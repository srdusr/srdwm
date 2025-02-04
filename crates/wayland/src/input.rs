//! Input routing: keyboard, pointer, and what "focus" means.
//!
//! Shared by both backends - smithay delivers keyboard/pointer events
//! through generic `InputBackend` traits, so the precise keybinding matching
//! and titlebar hit-testing exist once here and are called from the winit
//! backend ([`crate::winit`]) and the libinput/udev one ([`crate::udev`])
//! alike.
//!
//! Every function that routes an event checks the session lock first: while
//! locked, input goes to the lock surface and nowhere else. See
//! [`crate::lock`].

use smithay::backend::input::{ButtonState as BackendButtonState, KeyState as BackendKeyState, KeyboardKeyEvent};
use smithay::backend::session::Session as _;
use smithay::desktop::{layer_map_for_output, Window as DWindow, WindowSurfaceType};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{Anchor, ExclusiveZone, KeyboardInteractivity, Layer, LayerSurfaceCachedState};
use std::time::{Duration, Instant};

use srdwm_core::{Event as CoreEvent, Modifiers, TitlebarHit, WindowId};

/// Modifier that turns a drag anywhere in a window into move/resize.
/// Matches the `SUPER` the shipped and ported configs use for
/// `bindm ... movewindow` / `resizewindow`.
const DRAG_MODIFIER: Modifiers = Modifiers::SUPER;

use crate::state::CompState;

pub(crate) fn last_pointer_pos(state: &CompState) -> Point<f64, Logical> {
    state.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default()
}

/// Topmost layer-shell surface (if any) under `pos`, checked in the same
/// above-everything-else stacking order `space_render_elements` renders
/// `Overlay`/`Top` layers in (bars, launchers, notifications, lock UIs).
/// `Background`/`Bottom` layers (wallpapers) deliberately aren't checked
/// here: nothing in scope for the daily-driver gate needs pointer input
/// routed to them, and space windows should stay clickable over a
/// wallpaper.
/// `pos` is in the global space; layer geometry is relative to its own
/// output, so the pointer is translated into output-local coordinates
/// before hit-testing and the result translated back out.
/// Only checked for `Overlay`/`Top` before a window hit-test, and again for
/// `Bottom`/`Background` after one comes up empty - see the two call
/// sites in `handle_pointer_button`/`handle_pointer_position` for why it's
/// split rather than one four-layer loop here. A `Bottom`/`Background`
/// surface (a desktop-icons layer, a wallpaper daemon that wants clicks) is
/// meant to sit *behind* normal windows, so a window covering that point
/// should still get the click; `Overlay`/`Top` (an on-screen keyboard, a
/// bar, a dock) are meant to sit in front of everything, windows included.
///
/// Was `Overlay`/`Top` only, full stop - a `Bottom`-layer surface was
/// silently unclickable no matter what, since nothing else in
/// `handle_pointer_button` ever checked layers at all. Not the cause of
/// the live "clicking the dock does nothing" report (confirmed: that dock
/// uses `Layer::Top`, which was already checked), but a real, separate gap
/// found while chasing it - worth closing regardless of whether anything
/// currently deployed sits at `Bottom`/`Background` yet.
pub(crate) fn layer_surface_under_layers(state: &CompState, pos: Point<f64, Logical>, layers: [Layer; 2]) -> Option<(WlSurface, Point<i32, Logical>)> {
    let entry = state.output_at(pos)?;
    let origin = entry.location;
    let local = pos - origin.to_f64();
    let map = layer_map_for_output(&entry.output);
    for layer_kind in layers {
        // Not `map.layer_under(layer_kind, local)` - that hands back only
        // the single topmost surface whose *bounding box* contains `local`,
        // and if that one surface's own input region excludes the point
        // (its `surface_under` below returns `None`), the old code gave up
        // on this whole layer-kind rather than trying whatever real,
        // clickable surface is stacked underneath it. A bbox-only pick is
        // exactly wrong the moment two surfaces on the same layer-kind
        // overlap - a transparent, mapped-but-mostly-empty surface (a
        // backdrop-dismiss popup, concretely: `Overview`'s own bbox-wide
        // fallback region was exactly this shape before it was fixed
        // AGS-side) sitting in front of a real one in z-order would
        // silently swallow every click and even every hover/motion event
        // meant for the surface underneath, with no way to reach it at
        // all. Walking every candidate on this layer-kind, topmost first
        // (`.rev()`, matching `layer_under`'s own z-order convention), and
        // falling through to the next when a candidate's real input region
        // doesn't cover the point, is what `layer_under` alone can't do.
        for layer in map.layers_on(layer_kind).rev() {
            let Some(geo) = map.layer_geometry(layer) else { continue };
            if !geo.to_f64().contains(local) {
                continue;
            }
            if let Some((surface, surface_loc)) = layer.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL) {
                return Some((surface, origin + geo.loc + surface_loc));
            }
        }
    }
    None
}

pub(crate) fn layer_surface_under(state: &CompState, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
    layer_surface_under_layers(state, pos, [Layer::Overlay, Layer::Top])
}

/// The `Bottom`/`Background` half of the same lookup - see
/// `layer_surface_under_layers`'s doc comment for the ordering rationale.
pub(crate) fn background_layer_surface_under(state: &CompState, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
    layer_surface_under_layers(state, pos, [Layer::Bottom, Layer::Background])
}

/// `full` with only a top-anchored layer surface's exclusive zone (a menu
/// bar) subtracted back out - see `Monitor::maximize_geometry`'s own doc
/// comment for why maximize needs this third rect, distinct from both
/// `geometry` (every zone subtracted) and `full_geometry` (none). Shared by
/// both backends' `monitors()`, same as everything else in this module.
/// Deliberately re-derived from the layer list rather than reusing
/// `non_exclusive_zone()`: that smithay helper folds every anchor
/// together, with no way to ask it to skip a bottom-anchored dock while
/// still respecting a top-anchored bar.
pub(crate) fn maximize_geometry_for(output: &Output, full: srdwm_core::Rect) -> srdwm_core::Rect {
    let mut rect = full;
    for layer in layer_map_for_output(output).layers() {
        let data = with_states(layer.wl_surface(), |states| *states.cached_state.get::<LayerSurfaceCachedState>().current());
        let ExclusiveZone::Exclusive(amount) = data.exclusive_zone else { continue };
        if data.anchor.contains(Anchor::TOP) && !data.anchor.contains(Anchor::BOTTOM) {
            let shrink = (amount as i32 + data.margin.top).max(0);
            rect.y += shrink;
            rect.height = rect.height.saturating_sub(shrink as u32);
        }
    }
    rect
}

/// `ext_idle_notify_v1`'s whole job is answering "has the user touched an
/// input device recently" - `IdleNotifierState` does the actual timer
/// bookkeeping (see its own doc comment), this just has to be called from
/// every real input path, deliberately including while the session is
/// locked: idle activity is about the seat, not about which surface (if
/// any) an event ends up delivered to, and a lock daemon watching this
/// protocol to decide when to re-dim/re-lock still needs to see real
/// movement even though nothing else happens with it at a locked screen.
///
/// Throttled to once per 250ms: `notify_activity` removes and re-inserts a
/// calloop timer for every live notification, every call, with no
/// throttling of its own - fine at keypress/click frequency, but pointer
/// motion can fire far more often than that during a drag, and idle
/// timeouts are measured in minutes, not milliseconds, so nothing about
/// idle detection needs - or can even perceive - finer resolution than
/// this. The same class of hot-path-on-every-motion-event cost that made
/// this session's earlier diagnostic logging a real, measured regression
/// (see `docs/IMPLEMENTATION_STATUS.md`), just cheap enough here (in-memory
/// bookkeeping, not synchronous I/O) that throttling rather than removing
/// it outright is the right amount of caution.
fn notify_idle_activity(state: &mut CompState) {
    const THROTTLE: Duration = Duration::from_millis(250);
    let now = Instant::now();
    if state.last_idle_notify.is_some_and(|last| now.duration_since(last) < THROTTLE) {
        return;
    }
    state.last_idle_notify = Some(now);
    let seat = state.seat.clone();
    state.idle_notifier_state.notify_activity(&seat);
}

/// Re-resolves and re-asserts real Wayland pointer focus at `pos` - i.e.
/// re-runs the exact same layer-shell/decoration/content/background
/// hit-testing `handle_pointer_position` always did, and calls
/// `pointer.motion()` with whatever it finds, but *without* sending
/// `wl_pointer.frame` (callers decide when their own batch of events is
/// done) and without any of `handle_pointer_position`'s other side effects
/// (cursor shape, focus-follows-mouse, drag/resize updates) - those only
/// make sense on an actual motion event, not a button press.
///
/// Extracted so [`handle_pointer_button`] can call this immediately before
/// delivering a click, rather than only ever trusting whatever the *last*
/// real motion event happened to leave `PointerHandle`'s own focus at.
/// Those can disagree: confirmed live via a temporary diagnostic (since
/// removed) that `space.element_under(pos)` - srdwm's own, freshly
/// computed on every click - and
/// `PointerHandle::current_focus()` - Wayland's, last set by whichever
/// motion event happened to run before this click - disagreed on a real
/// user's real clicks, inconsistently, sometimes on the very same window.
/// A click landing on stale/no Wayland focus reads exactly like "clicking
/// doesn't work" or "the cursor isn't where clicking happens," even though
/// srdwm's own idea of what's under the pointer was correct the whole
/// time. Calling this right before every button event closes that gap
/// regardless of why focus went stale, rather than chasing the exact
/// staleness trigger (rapid clicks, a tap-to-click event with no
/// intervening motion delta, etc.) one cause at a time.
#[allow(clippy::type_complexity)]
fn refresh_pointer_focus(state: &mut CompState, pos: Point<f64, Logical>, time: u32) -> (Option<(WindowId, TitlebarHit)>, bool, bool, Option<WindowId>) {
    // Checked before literally everything else, including layer-shell --
    // see `elements::popup_surface_under`'s own doc comment for why: a
    // popup (tooltip, dropdown, right-click menu) always renders on top of
    // everything else, popups on their own parent's content and layer-shell
    // bars/docks alike, and hit-testing has to match that same priority or
    // a click/scroll over an open popup silently lands on whatever's
    // underneath it instead.
    let popup_hit = crate::elements::popup_surface_under(state, pos);
    let layer_hit = layer_surface_under(state, pos);
    // Broadened, not just layer-shell: both a layer surface and an open
    // popup are transient client UI that should suppress WM-level
    // decoration-cursor guessing and focus-follows-mouse the same way (see
    // both call sites below) - hovering a dropdown menu must not refocus
    // whatever window happens to sit underneath it.
    let over_layer_surface = layer_hit.is_some() || popup_hit.is_some();
    let hit = state.wm.borrow().hit_test(pos.x as i32, pos.y as i32);
    let under = state
        .space
        .element_under(pos)
        .filter(|(w, _)| dwindow_is_visible(state, w))
        .map(|(w, loc)| (w.clone(), loc));
    let over_content = under.is_some();
    // Whichever core window the pointer is over right now, decoration or
    // content - `None` while over a layer-shell surface or bare desktop.
    // Only `handle_pointer_position` actually uses this (focus-follows-
    // mouse), but it needs `under` before that's consumed by the match
    // below, so it's computed here rather than recomputed by the caller.
    let hovered_id = hit
        .map(|(id, _)| id)
        .or_else(|| under.as_ref().and_then(|(window, _)| dwindow_wl_surface(window)).and_then(|s| state.surface_to_id.get(&s).copied()));

    let Some(pointer) = state.seat.get_pointer() else { return (hit, over_layer_surface, over_content, hovered_id) };
    if let Some((surface, loc)) = popup_hit {
        let surface_loc = pos - loc.to_f64();
        pointer.motion(state, Some((surface, loc.to_f64())), &MotionEvent { location: surface_loc, serial: SERIAL_COUNTER.next_serial(), time });
    } else if let Some((surface, loc)) = layer_hit {
        let surface_loc = pos - loc.to_f64();
        pointer.motion(state, Some((surface, loc.to_f64())), &MotionEvent { location: surface_loc, serial: SERIAL_COUNTER.next_serial(), time });
    } else if hit.is_some() {
        // Over our own decoration - no client focus.
        pointer.motion(state, None, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    } else if let Some((window, loc)) = under {
        // `window.toplevel()` is only ever `Some` for a native xdg-shell
        // surface - it's `None` for every XWayland window, and even for a
        // plain xdg-shell one it's always the *root* surface regardless of
        // which subsurface the pointer is actually over (video/GL overlays,
        // some GTK/Electron popups). Either way that meant pointer focus
        // landed on the wrong surface - or no surface at all, for X11
        // clients - and the click coordinates were relative to the window
        // root rather than whatever was actually under the cursor.
        // `Window::surface_under` is smithay's own hit-test for this: it
        // walks the real surface tree (subsurfaces and popups included) and
        // unifies the xdg-shell/X11 cases the way `dwindow_wl_surface` does
        // elsewhere in this module.
        let win_relative = pos - loc.to_f64();
        if let Some((surface, offset)) = window.surface_under(win_relative, WindowSurfaceType::ALL) {
            let surface_loc = win_relative - offset.to_f64();
            let surface_origin = (loc + offset).to_f64();
            pointer.motion(state, Some((surface, surface_origin)), &MotionEvent { location: surface_loc, serial: SERIAL_COUNTER.next_serial(), time });
        }
    } else if let Some((surface, loc)) = background_layer_surface_under(state, pos) {
        // Bare desktop, no window there either - last chance for a
        // `Bottom`/`Background` layer surface (see
        // `layer_surface_under_layers`'s doc comment) before giving up.
        let surface_loc = pos - loc.to_f64();
        pointer.motion(state, Some((surface, loc.to_f64())), &MotionEvent { location: surface_loc, serial: SERIAL_COUNTER.next_serial(), time });
    } else {
        pointer.motion(state, None, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    }
    (hit, over_layer_surface, over_content, hovered_id)
}

pub(crate) fn handle_pointer_position(state: &mut CompState, pos: Point<f64, Logical>, time: u32) {
    notify_idle_activity(state);
    // Locked: pointer motion goes to the lock surface only. No hit-testing
    // against windows/decorations, so no hover, no drag, no resize.
    if state.lock.locked {
        let surface = state.any_lock_surface().cloned();
        if let Some(pointer) = state.seat.get_pointer() {
            let focus = surface.map(|s| (s, Point::from((0, 0)).to_f64()));
            pointer.motion(state, focus, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
            pointer.frame(state);
        }
        return;
    }

    let (hit, over_layer_surface, over_content, hovered_id) = refresh_pointer_focus(state, pos, time);
    let Some(pointer) = state.seat.get_pointer() else { return };
    // `PointerHandle::motion`/`button`/`axis` only queue the event with the
    // active grab - nothing sends `wl_pointer.frame` on its own (confirmed
    // reading smithay's `DefaultGrab`: its `motion`/`button` impls call
    // straight through to the handle and never call `frame`). `frame` is
    // what tells a client "the events since the last frame are one atomic
    // update, process them now" - required by the protocol since
    // `wl_pointer` version 5, and this compositor advertises v9. Without
    // it, any client that correctly waits for `frame` before acting on
    // motion/button state (most modern toolkits, confirmed live: neither
    // Firefox nor wezterm registered a click or a drag-selection, in both
    // cases with the cursor sitting squarely on the target) never actually
    // processes what it was sent, even though every event up to this point
    // was individually correct. This is likely the real root cause behind
    // this whole session's "clicking/scrolling doesn't work" reports --
    // every fix so far (subsurface routing, decoration geometry, app_id)
    // was real and necessary, but none of them could have mattered if the
    // client was never told to look at what it received.
    pointer.frame(state);

    update_cursor_shape(state, hit, over_layer_surface, over_content);

    let mut wm = state.wm.borrow_mut();
    let dragging_or_resizing = wm.is_dragging() || wm.is_resizing();
    if wm.is_dragging() {
        wm.update_drag(pos.x as i32, pos.y as i32);
    } else if wm.is_resizing() {
        wm.update_resize(pos.x as i32, pos.y as i32);
    }
    let focused = wm.focused_id();
    // `general.focus_follows_mouse`: hovering a *different* window focuses
    // it, no click needed - classic X11 sloppy focus. Gated on `hit`/
    // `under` actually landing on a window (not a layer surface or bare
    // desktop) and on not already being mid-drag/resize, where the pointer
    // sweeps over unrelated windows constantly and none of that should
    // steal focus from whatever's actually being dragged. `hovered_id !=
    // focused` both skips redundant work on every one of the many motion
    // events a stationary pointer over an already-focused window still
    // generates, and is what makes `auto_raise` (below) only fire on an
    // actual focus change rather than every motion tick too.
    let focus_follow_target =
        (wm.focus_follows_mouse && !dragging_or_resizing && !over_layer_surface).then_some(hovered_id).flatten().filter(|id| Some(*id) != focused);
    if let Some(id) = focus_follow_target {
        if wm.auto_raise {
            // `raise_window` alone here, not `focus_window` - the actual
            // core + real Wayland/X11 keyboard focus change happens once,
            // below, through the same `focus_window` free function every
            // click-driven focus change already goes through (sets real
            // keyboard focus too, which `WindowManager::focus_window`
            // alone does not).
            wm.raise_window(id);
        }
    }
    drop(wm);
    if let Some(id) = focus_follow_target {
        focus_window(state, id);
    }
    if dragging_or_resizing {
        if let Some(id) = focused {
            state.sync_geometry(id);
        }
    }
}

/// Sets the pointer to a resize-direction shape while hovering (or
/// actively dragging) one of our own decoration's resize edges, and back
/// to the default arrow when leaving our decoration for anything else.
///
/// Only ever touches `cursor_status` for our own decoration - never while
/// `layer_hit`/client content has focus, since a client surface drives its
/// own cursor via `wl_pointer.set_cursor` once it starts receiving
/// `pointer.motion()`/`enter` (already sent above, by the time this runs),
/// and stomping on that here would fight the client for control of its own
/// cursor rather than just leaving it alone.
///
/// Without this, `cursor_status` was only ever set by client requests --
/// nothing on the compositor's own side ever asked for a resize cursor at
/// all, so hovering or dragging one of our own decoration's edges never
/// looked any different from hovering plain content, regardless of what
/// shapes `cursor.rs` can actually render.
///
/// `over_content` distinguishes "over a client surface that will drive its
/// own cursor" from "over the bare desktop, where nothing ever will" --
/// without it, dragging off one of our decoration's resize edges straight
/// onto empty desktop left `cursor_status` stuck on that resize icon
/// forever: there is no client there to ever call `set_cursor` and reset
/// it, and this function's own early-return (for the "let the client drive
/// it" case) doesn't distinguish an *absent* client from a slow one.
fn update_cursor_shape(state: &mut CompState, hit: Option<(WindowId, TitlebarHit)>, over_layer_surface: bool, over_content: bool) {
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};

    if over_layer_surface {
        return;
    }
    let edge = match hit {
        Some((_, TitlebarHit::Resize(edge))) => Some(edge),
        _ => state.wm.borrow().resize_edge(),
    };
    let icon = match edge {
        Some(edge) => resize_cursor_icon(edge),
        // Hovering our own decoration but not an edge (the drag area, a
        // button) and not actively resizing: back to the plain arrow.
        None if hit.is_some() => CursorIcon::Default,
        // Over a client's own content: leave `cursor_status` alone, per the
        // doc comment above - the client drives it.
        None if over_content => return,
        // Bare desktop: nothing else will ever reset this, so we have to.
        None => CursorIcon::Default,
    };
    state.cursor_status = CursorImageStatus::Named(icon);
}

fn resize_cursor_icon(edge: srdwm_core::ResizeEdge) -> smithay::input::pointer::CursorIcon {
    use smithay::input::pointer::CursorIcon;
    use srdwm_core::ResizeEdge;
    match edge {
        ResizeEdge::Left | ResizeEdge::Right => CursorIcon::EwResize,
        ResizeEdge::Top | ResizeEdge::Bottom => CursorIcon::NsResize,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorIcon::NwseResize,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorIcon::NeswResize,
    }
}

/// The underlying `wl_surface` for a mapped window, regardless of whether
/// it's a native `xdg-shell` toplevel or an XWayland `X11Surface` --
/// `desktop::Window` exposes these as two separate accessors with no
/// shared one.
pub(crate) fn dwindow_wl_surface(w: &DWindow) -> Option<WlSurface> {
    if let Some(top) = w.toplevel() {
        return Some(top.wl_surface().clone());
    }
    w.x11_surface().and_then(|x| x.wl_surface())
}

/// Whether `w` is actually visible right now - on the current workspace and
/// not minimized - matching `WindowManager::visible_windows`'s own filter.
///
/// `state.space` (smithay's `Space`) is not workspace-aware: a window stays
/// mapped in it, and so stays hit-testable by `Space::element_under`, from
/// the moment it's created until it's explicitly minimized or destroyed --
/// switching workspace never unmaps anything (see `minimize` in
/// `udev::platform`, the only other place that calls `unmap_elem`, and the
/// absence of any workspace-switch handler that touches `self.space` at
/// all). Without this check, `element_under` freely returns a window sitting
/// on a workspace that isn't even shown, and a click "through" empty desktop
/// on the current workspace silently focuses/raises/moves motion onto that
/// invisible window instead of whatever (if anything) is really there.
fn dwindow_is_visible(state: &CompState, w: &DWindow) -> bool {
    let Some(id) = dwindow_wl_surface(w).and_then(|s| state.surface_to_id.get(&s).copied()) else { return false };
    let wm = state.wm.borrow();
    wm.window(id).is_some_and(|win| !win.minimized && win.workspace == wm.current_workspace())
}

/// Requests a client close its window, whichever kind it is.
pub(crate) fn close_dwindow(w: &DWindow) {
    if let Some(top) = w.toplevel() {
        top.send_close();
    } else if let Some(x11) = w.x11_surface() {
        let _ = x11.close();
    }
}

/// Focuses `id` in our own `WindowManager` *and* gives its surface real
/// Wayland/X11 keyboard focus - without this, a window can be raised and
/// tiled correctly yet never receive a single keystroke.
pub(crate) fn focus_window(state: &mut CompState, id: WindowId) {
    state.wm.borrow_mut().focus_window(id);
    // Raises the window in smithay's own `Space` too, not just core's
    // `order` - `Space` keeps a completely independent stacking order of
    // its own, which is what actually renders on top *and* what
    // `space.element_under` hit-tests against; `WindowManager::order`
    // (which `focus_window` above already updates) has no effect on
    // either. Without this, any focus path that doesn't also happen to
    // raise `Space` manually (Alt-Tab, a dock's IPC "focus" dispatch,
    // scratchpad show, the Snap-Layouts flyout, ...) left a window
    // genuinely focused - keyboard input, core's own idea of "topmost"
    // both correct - while it kept rendering *underneath* whatever was
    // already on top, and a click on the visible (stale-topmost) window
    // silently reached that one instead. "Focus doesn't bring a window to
    // the front" and "clicking through a window that's fully covering
    // another" are the same root cause, not two bugs. Previously only the
    // plain-content-click branch in `handle_pointer_button` did this,
    // manually, immediately before calling this function - every other
    // caller went through unraised. Cheap even when the window is already
    // topmost (`raise_element` on an already-last element is a no-op
    // reinsertion), so unconditional here rather than gated on whether
    // focus is actually changing.
    if let Some(w) = state.id_to_window.get(&id).cloned() {
        state.space.raise_element(&w, true);
        state.raise_pinned();
    }
    state.pending.borrow_mut().push(CoreEvent::WindowFocused(id));
    let surface = state.id_to_window.get(&id).and_then(dwindow_wl_surface);
    // Routed through `set_keyboard_focus` (rather than calling
    // `KeyboardHandle::set_focus` directly) so clipboard/primary-selection
    // focus follows window focus too - see that method's doc comment.
    state.set_keyboard_focus(surface);
}

/// Re-syncs real Wayland/X11 keyboard focus to whatever `WindowManager`
/// already considers focused, without changing what that is.
///
/// For callers where core's own focus already moved on its own --
/// specifically `WindowManager::remove_window`'s fallback to
/// `self.order.last()` when the just-closed window was the focused one --
/// and only the Wayland/X11 side needs to catch up to it. Without this, the
/// window core now considers focused (and renders as such) never actually
/// receives a keystroke until it's clicked, since nothing told
/// `set_keyboard_focus` focus had moved.
///
/// `focus_window` above is for the opposite direction: driving core's
/// focus deliberately (a click, a keybinding) and syncing outward from
/// that. This is "core already decided, catch the rest of the compositor
/// up" - `wm.focus_window` must not be called again here, since the id
/// core picked (or `None`, if nothing is left) is exactly what should win.
pub(crate) fn sync_keyboard_focus(state: &mut CompState) {
    let focused = state.wm.borrow().focused_id();
    let surface = focused.and_then(|id| state.id_to_window.get(&id)).and_then(dwindow_wl_surface);
    state.set_keyboard_focus(surface);
}

pub(crate) fn handle_pointer_button(state: &mut CompState, pos: Point<f64, Logical>, button: u32, pressed: bool, time: u32) {
    notify_idle_activity(state);
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;
    const BTN_MIDDLE: u32 = 0x112;
    let serial = SERIAL_COUNTER.next_serial();

    // Locked: forward the click to the lock surface (it may have a button or
    // a text field) but never let it focus, raise, drag, or close a window.
    if state.lock.locked {
        if let Some(pointer) = state.seat.get_pointer() {
            let button_state = if pressed { BackendButtonState::Pressed } else { BackendButtonState::Released };
            pointer.button(state, &ButtonEvent { serial, time, button, state: button_state });
            pointer.frame(state);
        }
        return;
    }

    // The context menu, if open, captures every press: a click inside
    // resolves whichever row it landed on, a click anywhere else just
    // dismisses it. Neither case falls through to the normal handling
    // below - opening the menu and then clicking a window underneath it
    // should not *also* focus/raise/drag that window on the same click,
    // the same "one click, one action" rule every native window menu
    // follows.
    if pressed {
        if let Some(menu) = state.context_menu.take() {
            if let Some(row) = menu.row_at(pos.x as i32, pos.y as i32) {
                let (_, action) = menu.items[row];
                state.close_context_menu();
                state.run_context_menu_action(menu.window, action);
            } else {
                state.close_context_menu();
            }
            return;
        }
        // Same "one click, one action" rule as the context menu above --
        // a click inside the Snap-Layouts flyout applies that zone, a click
        // anywhere else just dismisses it.
        if let Some(flyout) = state.snap_flyout.take() {
            if let Some(zone) = flyout.zone_at(pos.x as i32, pos.y as i32) {
                state.close_snap_flyout();
                state.run_snap_flyout_action(flyout.window, zone);
            } else {
                state.close_snap_flyout();
            }
            return;
        }
    }

    // Modifier+drag: with the modifier held, dragging *anywhere* in a window
    // moves it (left button) or resizes it from the nearest corner (right
    // button) - the `bindm SUPER, mouse:272/273` gesture. Without this a
    // window can only be moved by its titlebar, which is useless for
    // windows that have none (fullscreen, CSD apps, layer surfaces).
    //
    // Checked before the titlebar hit-test so the modifier wins over the
    // decoration: holding the modifier and grabbing the titlebar should
    // still move, not press a titlebar button.
    if pressed && (button == BTN_LEFT || button == BTN_RIGHT) {
        let mods = state.seat.get_keyboard().map(|k| core_modifiers_from_xkb(&k.modifier_state()));
        if mods.is_some_and(|m| m.contains(DRAG_MODIFIER)) {
            let target = state.wm.borrow().window_at(pos.x as i32, pos.y as i32);
            if let Some(id) = target {
                focus_window(state, id);
                let mut wm = state.wm.borrow_mut();
                if button == BTN_LEFT {
                    wm.start_drag(id, pos.x as i32, pos.y as i32);
                } else {
                    let edge = wm.nearest_corner(id, pos.x as i32, pos.y as i32);
                    wm.start_resize(id, edge, pos.x as i32, pos.y as i32);
                }
                return;
            }
        }
    }

    if pressed && button == BTN_LEFT {
        let layer_hit = layer_surface_under(state, pos);
        if let Some((surface, _)) = &layer_hit {
            // Look the surface up on whichever output actually holds it.
            let on_demand = state
                .outputs()
                .find_map(|output| {
                    layer_map_for_output(output)
                        .layer_for_surface(surface, WindowSurfaceType::ALL)
                        .map(|l| {
                            l.can_receive_keyboard_focus()
                                && l.cached_state().keyboard_interactivity != KeyboardInteractivity::Exclusive
                        })
                })
                .unwrap_or(false);
            // `Exclusive` layers (lock screens, exclusive launchers) already
            // hold focus from `ensure_layer_initial_configure` and keep it
            // regardless of where else is clicked; only `OnDemand` layers
            // (e.g. a bar's search field) claim it on click.
            if on_demand {
                state.set_keyboard_focus(Some(surface.clone()));
            }
        }
        let hit = if layer_hit.is_some() { None } else { state.wm.borrow().hit_test(pos.x as i32, pos.y as i32) };
        if let Some((id, hit)) = hit {
            focus_window(state, id);
            match hit {
                TitlebarHit::Drag => {
                    // Double-click the titlebar to maximise, as every other
                    // desktop does - one of the few window operations that
                    // otherwise needs the keyboard or a precise button hit.
                    if state.is_double_click(id, time) {
                        state.wm.borrow_mut().toggle_maximize(id);
                        state.sync_geometry(id);
                        crate::foreign_toplevel::send_state(state, id);
                    } else {
                        state.wm.borrow_mut().start_drag(id, pos.x as i32, pos.y as i32)
                    }
                }
                TitlebarHit::Close => {
                    if let Some(w) = state.id_to_window.get(&id) {
                        close_dwindow(w);
                    }
                }
                TitlebarHit::Maximize => {
                    state.wm.borrow_mut().toggle_maximize(id);
                    state.sync_geometry(id);
                    crate::foreign_toplevel::send_state(state, id);
                }
                TitlebarHit::Minimize => {
                    state.wm.borrow_mut().minimize_window(id);
                    crate::foreign_toplevel::send_state(state, id);
                }
                TitlebarHit::Resize(edge) => state.wm.borrow_mut().start_resize(id, edge, pos.x as i32, pos.y as i32),
            }
        } else if layer_hit.is_none() {
            if let Some((window, _loc)) = state.space.element_under(pos).filter(|(w, _)| dwindow_is_visible(state, w)) {
                let window = window.clone();
                // `focus_window` itself raises both `Space` and pinned
                // windows now - see its own doc comment. No longer done
                // manually here first.
                if let Some(&id) = dwindow_wl_surface(&window).and_then(|s| state.surface_to_id.get(&s)) {
                    focus_window(state, id);
                }
            }
        }
    } else if pressed && (button == BTN_RIGHT || button == BTN_MIDDLE) {
        // Right-click a titlebar: open the window menu (minimize/maximize/
        // pin/close) - previously nothing at all, since the only
        // right-button behaviour anywhere was the SUPER+right-drag resize
        // gesture above, which needs the modifier held. Middle-click:
        // lower the window instead, the convention several X11 WMs
        // (twm, fvwm, IceWM) have always had. Both only fire on the
        // titlebar's plain drag area - a resize edge or one of the three
        // buttons keeps its own single meaning regardless of which button
        // was pressed, so a right-click on the close button, say, doesn't
        // do something else entirely.
        let hit = state.wm.borrow().hit_test(pos.x as i32, pos.y as i32);
        match (button, hit) {
            (BTN_RIGHT, Some((id, TitlebarHit::Drag))) => state.open_context_menu(id, (pos.x as i32, pos.y as i32)),
            (BTN_MIDDLE, Some((id, TitlebarHit::Drag))) => state.wm.borrow_mut().lower_window(id),
            // Right-click the maximize button itself: the Snap-Layouts
            // flyout (pick a half/quarter position for this window)
            // instead of the window menu - a plain left-click there still
            // just toggles maximize, unchanged.
            (BTN_RIGHT, Some((id, TitlebarHit::Maximize))) => state.open_snap_flyout(id, (pos.x as i32, pos.y as i32)),
            _ => {}
        }
    } else if !pressed {
        let mut wm = state.wm.borrow_mut();
        let was_dragging = wm.is_dragging();
        let was_resizing = wm.is_resizing();
        // `start_drag`/`start_resize` both focus the window they grab, and
        // nothing else can change focus while a grab is active (the pointer
        // is captured by the drag, not routed elsewhere) - so `focused_id`
        // is reliably the window `end_drag`/`end_resize` are about to
        // finish, without `WindowManager` needing to hand the id back
        // itself.
        let id = wm.focused_id();
        if was_dragging {
            wm.end_drag();
        } else if was_resizing {
            wm.end_resize();
        }
        drop(wm);
        // `end_drag` can snap the geometry one more time (edge/top-of-
        // screen snapping, `SmartPlacement::snap_zone`) *after* the last
        // `update_drag` already moved the window - without this, that
        // final snap only ever reached `Window.geometry`. The border and
        // titlebar redraw fresh from live geometry every frame, so they'd
        // jump to the snapped rect immediately, while the client's actual
        // mapped surface (driven only by `sync_geometry`'s
        // `space.map_element`/`xdg_toplevel.configure`) stayed wherever the
        // drag physically stopped - decoration visibly detached from its
        // own window's content. Click routing desynced the same way:
        // `hit_test`/`window_at` read the now-snapped `Window.geometry`
        // while `space.element_under` still read the stale pre-snap
        // position, so clicks in the visually-snapped zone resolved
        // against the wrong rect. The X11 backend already gets this right
        // (`crates/x11/src/lib.rs`'s `ButtonRelease` handler); this was the
        // one call site in the module doc'd as "shared by both backends"
        // that never got the same fix.
        if was_dragging || was_resizing {
            if let Some(id) = id {
                state.sync_geometry(id);
            }
        }
    }

    // Re-assert real Wayland pointer focus at `pos` immediately before the
    // actual click - see `refresh_pointer_focus`'s own doc comment for why
    // this can't just trust whatever the last motion event left focus at.
    // A no-op from the client's perspective when focus was already correct
    // (an idempotent motion event at the same surface-local coordinates it
    // already has), so this costs nothing in the common case.
    refresh_pointer_focus(state, pos, time);
    if let Some(pointer) = state.seat.get_pointer() {
        let button_state = if pressed { BackendButtonState::Pressed } else { BackendButtonState::Released };
        pointer.button(state, &ButtonEvent { serial, time, button, state: button_state });
        // See the matching comment in `handle_pointer_position`: `button`
        // alone never tells the client the event is ready to act on, only
        // `frame` does.
        pointer.frame(state);
    }
}

/// Shared between the winit (nested) and udev (bare-TTY) backends: both
/// deliver keyboard events through smithay's generic `KeyboardKeyEvent`
/// trait, so the precise-keybinding-matching logic (see the module docs)
/// only needs to exist once.
pub(crate) fn handle_keyboard_key_event<B: smithay::backend::input::InputBackend, E: KeyboardKeyEvent<B>>(state: &mut CompState, event: &E) {
    notify_idle_activity(state);
    let keycode = event.key_code();
    let key_state = event.state();
    let time = event.time_msec();
    let serial = SERIAL_COUNTER.next_serial();
    let Some(keyboard) = state.seat.get_keyboard() else { return };

    // While the session is locked, every key goes to the lock surface and
    // *nothing* is treated as a WM keybinding. Skipping this would leave the
    // lock trivially bypassable - the config binds spawn commands
    // (`Mod4+Return` opens a terminal), so honouring bindings here would let
    // anyone at a locked screen run arbitrary programs.
    if state.lock.locked {
        // A native lock (`crate::native_lock`) has no external client
        // surface to forward to at all - srdwm is its own locker, so
        // every keystroke feeds the password buffer directly instead.
        // Only on press: a character is typed on key-down, matching
        // ordinary text input, and password/BackSpace/Return/Escape
        // handling only make sense once per physical keystroke, not once
        // per press *and* release.
        if state.lock.native.is_some() {
            if key_state == BackendKeyState::Pressed {
                keyboard.input::<(), _>(state, keycode, key_state, serial, time, |data, mods, handle| {
                    // `keysym_to_utf8` on the already-resolved keysym
                    // (rather than the state-aware `xkb_state_key_get_
                    // utf8` xkbcommon's own docs recommend) is a
                    // deliberate simplification: correct for plain
                    // ASCII/shifted-symbol passwords, which is the
                    // overwhelming common case; the gap is dead-key/
                    // compose sequences spanning more than one keypress,
                    // which would just make that one character not match
                    // rather than ever falsely succeed - a usability
                    // rough edge, not a security one. Computed before
                    // `keysym_name_for` below, which takes `handle` by
                    // value.
                    let utf8 = xkbcommon::xkb::keysym_to_utf8(handle.modified_sym());
                    let name = keysym_name_for(handle).unwrap_or_default();
                    data.native_lock_key(&name, &utf8, mods.caps_lock);
                    FilterResult::Intercept(())
                });
            } else {
                keyboard.input::<(), _>(state, keycode, key_state, serial, time, |_, _, _| FilterResult::Intercept(()));
            }
            return;
        }
        keyboard.input::<(), _>(state, keycode, key_state, serial, time, |_, _, _| FilterResult::Forward);
        return;
    }

    let bound_keys = state.bound_keys.clone();
    let matched: Option<(String, Modifiers)> =
        keyboard.input(state, keycode, key_state, serial, time, move |data, mods, handle| {
            let modifiers = core_modifiers_from_xkb(mods);
            // `Ctrl+Alt+F1`..`F12` (xkb emits these as the `XF86Switch_VT_1`..
            // `_12` keysyms, not a plain function-key + modifier combo) --
            // handled here, by raw keysym *value* rather than name, since
            // matching a name string wrong fails silently and looks
            // identical to this never having been implemented at all (it
            // wasn't, until now: reported live, the user had to leave the
            // graphical session entirely and log in on a different TTY to
            // get a shell back after srdwm went down, because nothing ever
            // told the session to switch away). Values are contiguous
            // (0x1008FE01..=0x1008FE0C, xkbcommon's `keysyms.rs`), so `raw -
            // KEY_XF86SWITCH_VT_1 + 1` is the target VT. Udev/bare-TTY
            // backend only - `data.udev` is `None` under the nested winit
            // backend, where VT switching is meaningless, so this is a
            // no-op there rather than an error, same as every other
            // udev-only feature in this module.
            const KEY_XF86SWITCH_VT_1: u32 = 0x1008_FE01;
            const KEY_XF86SWITCH_VT_12: u32 = 0x1008_FE0C;
            let raw = handle.modified_sym().raw();
            if (KEY_XF86SWITCH_VT_1..=KEY_XF86SWITCH_VT_12).contains(&raw) {
                if key_state == BackendKeyState::Pressed {
                    if let Some(udev) = data.udev.as_mut() {
                        let vt = (raw - KEY_XF86SWITCH_VT_1 + 1) as i32;
                        if let Err(e) = udev.session.change_vt(vt) {
                            log::warn!("udev: change_vt({vt}) failed: {e}");
                        }
                    }
                }
                return FilterResult::Intercept((String::new(), modifiers));
            }
            match keysym_name_for(handle) {
                Some(name) if bound_keys.contains(&srdwm_core::key_combo_string(modifiers, &name)) => {
                    FilterResult::Intercept((name, modifiers))
                }
                _ => FilterResult::Forward,
            }
        });

    match key_state {
        BackendKeyState::Pressed => {
            // An empty `key_name` is the VT-switch case above, already
            // fully handled inside the closure - it isn't a real
            // keybinding and must not start a repeat timer or fire a
            // `CoreEvent::KeyPress` (`Lua` config has nothing bound to `""`,
            // so this would be harmless either way, but skipping it is both
            // cheaper and clearer than relying on that).
            if let Some((key_name, modifiers)) = matched {
                if !key_name.is_empty() {
                    state.begin_repeat(keycode, &key_name, modifiers);
                    state.pending.borrow_mut().push(CoreEvent::KeyPress { key_name, modifiers });
                }
            }
        }
        // Any release ends a repeat of *that* key; releasing an unrelated
        // key must not stop it.
        BackendKeyState::Released => state.end_repeat(keycode),
    }
    // Unmatched keys were already forwarded to the focused client by
    // `FilterResult::Forward` inside the closure above.
}

/// Translates the effective xkb keysym for this keypress into the same
/// `"Return"`/`"a"`/`"F5"`-style name `srdwm_core::keysyms` uses, so a
/// binding written once in Lua resolves identically on X11 and Wayland.
pub(crate) fn keysym_name_for(handle: smithay::input::keyboard::KeysymHandle<'_>) -> Option<String> {
    srdwm_core::keysyms::keysym_to_name(handle.modified_sym().raw())
}

pub(crate) fn core_modifiers_from_xkb(mods: &smithay::input::keyboard::ModifiersState) -> Modifiers {
    let mut m = Modifiers::empty();
    if mods.shift {
        m |= Modifiers::SHIFT;
    }
    if mods.ctrl {
        m |= Modifiers::CTRL;
    }
    if mods.alt {
        m |= Modifiers::ALT;
    }
    if mods.logo {
        m |= Modifiers::SUPER;
    }
    m
}

/// Modifier+scroll cycles workspaces, consuming the event.
///
/// Returns `true` if it handled the scroll, in which case the caller must
/// *not* also forward it to the client. Generic over the input backend for
/// the same reason the keyboard handler is: both backends deliver scroll
/// through smithay's `PointerAxisEvent` trait.
pub(crate) fn handle_workspace_scroll<B, E>(state: &mut CompState, event: &E) -> bool
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::PointerAxisEvent<B>,
{
    use smithay::backend::input::Axis;

    notify_idle_activity(state);
    if state.lock.locked {
        return false;
    }
    let mods = state.seat.get_keyboard().map(|k| core_modifiers_from_xkb(&k.modifier_state()));
    if !mods.is_some_and(|m| m.contains(DRAG_MODIFIER)) {
        return false;
    }
    let Some(v) = event.amount(Axis::Vertical).filter(|v| *v != 0.0) else { return false };
    // Scrolling down (positive) advances, matching `workspace, e+1`.
    switch_workspace_relative(state, v > 0.0)
}

/// Switches to the next (`forward`) or previous workspace in id order,
/// wrapping around, and fires the two follow-up broadcasts a plain
/// `WindowManager::switch_workspace` call alone doesn't cover. The shared
/// body behind every *relative* workspace switch - `SUPER`+scroll above,
/// and a 3+-finger touchpad swipe (`handle_gesture_swipe_end` below) --
/// pulled out here rather than duplicated a second time: both gaps below
/// were found missing for the scroll gesture specifically during this same
/// session, and nothing about either is scroll-only, so a second call site
/// copy-pasting the same steps would have been one missed broadcast away
/// from reintroducing the exact bug that was just fixed once already.
/// Returns `false` (and does nothing) if there are no workspaces at all.
fn switch_workspace_relative(state: &mut CompState, forward: bool) -> bool {
    let mut wm = state.wm.borrow_mut();
    let ids: Vec<_> = wm.workspaces().iter().map(|w| w.id).collect();
    if ids.is_empty() {
        return false;
    }
    let current = ids.iter().position(|&id| id == wm.current_workspace()).unwrap_or(0);
    let next = if forward { (current + 1) % ids.len() } else { (current + ids.len() - 1) % ids.len() };
    wm.switch_workspace(ids[next]);
    drop(wm);
    // Without this, the switch above is invisible: nothing shows or hides
    // a single window for the new workspace until `main.rs`'s `sync()`
    // runs, which only happens when a polled event sets `dirty` - see
    // `srdwm_core::Event::WorkspaceChanged`'s doc comment. Found live-
    // testing the unrelated `ext_workspace_v1` protocol's own `activate`
    // request, which has the identical problem; the scroll gesture had the
    // exact same bug already, just never one anyone traced back this far.
    state.pending.borrow_mut().push(srdwm_core::Event::WorkspaceChanged);
    // Same reasoning as `foreign_toplevel::send_state`'s call sites: without
    // this, a dock's workspace pill only ever tracked switches driven
    // through `ext_workspace_handle_v1.activate` itself, going stale the
    // moment a gesture (or any other non-protocol trigger) changed the
    // active workspace instead.
    crate::workspace::broadcast_active_workspace(state);
    true
}

/// A 3+-finger touchpad swipe just started - resets the running horizontal
/// offset `handle_gesture_swipe_update` accumulates into, or leaves it
/// `None` while the session is locked so a swipe over the lock screen does
/// nothing (matching every other pointer/keyboard path's "locked: no normal
/// handling" rule - see this module's own doc comment).
pub(crate) fn handle_gesture_swipe_begin<B, E>(state: &mut CompState, event: &E)
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::GestureBeginEvent<B>,
{
    notify_idle_activity(state);
    state.gesture_swipe = if state.lock.locked { None } else { Some((event.fingers(), 0.0)) };
}

/// Accumulates one update's worth of horizontal motion into the swipe
/// started by `handle_gesture_swipe_begin` - `delta_x` is relative to the
/// *previous* update, not a running total (see `gesture_swipe`'s own doc
/// comment on `CompState`), so summing here is the only way to know the
/// swipe's real total distance once it ends.
pub(crate) fn handle_gesture_swipe_update<B, E>(state: &mut CompState, event: &E)
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::GestureSwipeUpdateEvent<B>,
{
    if let Some((_, total_dx)) = state.gesture_swipe.as_mut() {
        *total_dx += event.delta_x();
    }
}

/// A touchpad swipe just ended - switches workspace if it was a genuine
/// 3+-finger swipe past `SWIPE_THRESHOLD` and wasn't cancelled (a libinput
/// gesture is marked cancelled when it doesn't resolve to a clean single
/// direction, e.g. the fingers moved back and forth). Below the threshold
/// or below 3 fingers, this does nothing - the same "did you mean it"
/// floor a mis-clicked drag gets elsewhere in this file, and 2-finger
/// motion is already handled as ordinary scroll (`PointerAxis`) rather
/// than reaching here at all on a correctly configured touchpad.
///
/// Deliberately claimed entirely by the compositor rather than forwarded to
/// the focused client, unlike pinch/hold (forwarded as-is in
/// `udev::session`): `wp_pointer_gestures` swipe is specifically the
/// 3/4-finger overview-style gesture, and the handful of desktops that
/// support it at all (GNOME, sway, Hyprland) all reserve it for workspace
/// switching the same way - there is no real client-side consumer to lose
/// by not forwarding it. Swipe left (negative `total_dx`) advances to the
/// next workspace, right goes back, matching macOS's own convention for
/// swiping between spaces.
const SWIPE_THRESHOLD: f64 = 60.0;

pub(crate) fn handle_gesture_swipe_end<B, E>(state: &mut CompState, event: &E)
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::GestureEndEvent<B>,
{
    let Some((fingers, total_dx)) = state.gesture_swipe.take() else { return };
    if event.cancelled() || fingers < 3 || total_dx.abs() < SWIPE_THRESHOLD {
        return;
    }
    switch_workspace_relative(state, total_dx < 0.0);
}
