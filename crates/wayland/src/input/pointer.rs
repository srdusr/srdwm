//! Pointer motion, button presses, and cursor-shape resolution.

use smithay::backend::input::ButtonState as BackendButtonState;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::shell::wlr_layer::KeyboardInteractivity;

use srdwm_core::{TitlebarHit, WindowId};

use crate::state::CompState;

use super::focus::{close_dwindow, dwindow_is_visible, dwindow_wl_surface, focus_window};
use super::keyboard::core_modifiers_from_xkb;
use super::layers::{background_layer_surface_under, layer_surface_under};
use super::{notify_idle_activity, DRAG_MODIFIER};

/// Minimum gap between `redraw_decoration_buffer` calls fired from an
/// active resize drag's own pointer-motion events - see `CompState::
/// resize_redraw_at`'s doc comment. 60Hz: fast enough that the border
/// visibly tracks the drag, far below the per-motion-event rate a real
/// mouse or touchpad can produce.
pub(crate) const RESIZE_REDRAW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1000 / 60);

/// `WindowManager::hit_test`, but substituting each window's currently
/// *animated* rect (if it has one active in `state.window_anims`) for its
/// final `geometry` - see `hit_test_with`'s own doc comment in
/// `crates/core/src/manager/hittest.rs` for why plain `hit_test` alone gets
/// this wrong during a maximize/fullscreen/snap toggle or a new window's
/// open-slide. Every decoration hit-test call site in this module goes
/// through this now instead of calling `hit_test` on the borrowed
/// `WindowManager` directly.
fn hit_test_animated(state: &CompState, x: i32, y: i32) -> Option<(WindowId, TitlebarHit)> {
    state.wm.borrow().hit_test_with(x, y, |id, geometry| {
        let animated = state.window_anims.get(&id).map(crate::state::WindowAnim::current_rect).unwrap_or(geometry);
        // Also corrects for a client whose real committed size differs
        // from what was requested (a terminal's cell-quantized size, most
        // commonly) - see `effective_frame`'s own doc comment. Without
        // this, the resize-margin/border hit-test zone stayed sized to the
        // *requested* rect even after the border itself moved to match the
        // real one, so the clickable edge and the visible edge disagreed
        // again, just like the border and the desktop background used to.
        state.effective_frame(id, animated)
    })
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
fn refresh_pointer_focus(
    state: &mut CompState,
    pos: Point<f64, Logical>,
    time: u32,
) -> (Option<(WindowId, TitlebarHit)>, bool, bool, Option<WindowId>, Option<(WlSurface, Point<f64, Logical>)>) {
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
    let hit = hit_test_animated(state, pos.x as i32, pos.y as i32);
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

    let Some(pointer) = state.seat.get_pointer() else { return (hit, over_layer_surface, over_content, hovered_id, None) };
    // Freshly resolved target from ordinary hit-testing - overridden below
    // by `pointer_button_grab` when a button is held, per its own doc
    // comment (the Wayland implicit-grab rule).
    let resolved: Option<(WlSurface, Point<f64, Logical>)> = if let Some((surface, loc)) = popup_hit {
        Some((surface, loc.to_f64()))
    } else if let Some((surface, loc)) = layer_hit {
        Some((surface, loc.to_f64()))
    } else if hit.is_some() {
        None // Over our own decoration - no client focus.
    } else if let Some((window, loc)) = &under {
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
        //
        // `loc` (from `Space`) is the window's raw *buffer*-origin in screen
        // space, NOT its visible top-left - see `sync_geometry`'s own doc
        // comment (`state/geometry.rs`), which positions every window via
        // `map_element(w, (geom.x - content_offset.x, ...))` *specifically*
        // so that `pos - loc` alone already lands in the buffer-local
        // coordinates `Window::surface_under` expects (confirmed against
        // smithay 0.7.0's own source: the ordinary toplevel branch hands
        // `point` straight through with a hardcoded `(0, 0)` offset, unlike
        // its sibling popup branch a few lines above, which does add
        // `self.geometry().loc` - so a toplevel's `point` has to already
        // be buffer-local, and `map_element`'s own placement is what makes
        // `pos - loc` be exactly that with no further adjustment needed).
        //
        // A previous version of this line added `content_offset` back a
        // *second* time (`pos - loc + content_offset`), reasoning that
        // `loc` was the visible position and needed shifting back to
        // buffer-local - but `sync_geometry` had already done that
        // shifting into `loc` itself, so this double-applied it: every
        // click on a CSD window with a nonzero shadow margin (GTK4 clients
        // - Firefox concretely, on its own titlebar/tab-strip buttons
        // specifically, since that's real content on an undecorated
        // window, not srdwm's own decoration) landed `content_offset`
        // *past* whatever was actually clicked, in the opposite direction
        // from the original (pre-any-fix) bug. Both versions were wrong in
        // opposite directions; plain `pos - loc` is what `sync_geometry`'s
        // own contract actually calls for.
        let win_relative = pos - loc.to_f64();
        window.surface_under(win_relative, WindowSurfaceType::ALL).map(|(surface, offset)| (surface, (*loc + offset).to_f64()))
    } else {
        // Bare desktop, no window there either - last chance for a
        // `Bottom`/`Background` layer surface (see
        // `layer_surface_under_layers`'s doc comment) before giving up.
        background_layer_surface_under(state, pos).map(|(surface, loc)| (surface, loc.to_f64()))
    };
    // `pointer_button_grab`'s own lock is deliberately skipped whenever a
    // real Wayland-level grab is active (`pointer.is_grabbed()`) - most
    // concretely, a client-initiated `wl_data_device` drag-and-drop
    // (`DnDGrab`, installed the moment a client calls `start_drag`, e.g. a
    // browser tab being torn out into another window). smithay's own
    // `DnDGrab::motion` ignores this call's `focus` argument for the
    // client-facing side (it explicitly calls `handle.motion(data, None,
    // event)`, since no client gets ordinary pointer focus mid-drag) but
    // *does* feed the same `focus` straight into `update_focus`, which is
    // what actually decides the current drop target as the cursor moves.
    // Keeping the origin-surface lock active here as well meant that value
    // stayed pinned to whichever window the drag *started* over for the
    // entire gesture, so `update_focus` could never see a second window as
    // the drop target no matter where the cursor actually went - reported
    // live as not being able to drag a tab from one window onto another.
    // The lock's own reason for existing (a GTK drag recognizer treating a
    // mid-gesture `leave` as "abort", see this field's own doc comment)
    // only applies to *ordinary* pointer motion, which is exactly the case
    // `is_grabbed()` being false identifies - once a real grab has taken
    // over, that grab's own implementation is already responsible for
    // routing enter/leave correctly, and needs the true, freshly-resolved
    // surface to do it, not a stale one.
    let delivery = if pointer.is_grabbed() { resolved.clone() } else { state.pointer_button_grab.clone().or_else(|| resolved.clone()) };
    if let Some((surface, origin)) = delivery {
        // `MotionEvent.location` is documented on `smithay::input::pointer::
        // MotionEvent` itself as "Location of the pointer in compositor
        // space" - i.e. global, the same space `pos` is already in.
        // `PointerHandle::motion`'s own `focus` parameter carries `origin`
        // specifically so smithay can compute the surface-relative
        // coordinate *itself* (`event.location - loc`, see `PointerInternal
        // ::motion` in smithay's `input/pointer/mod.rs`) before handing it
        // to the client and storing the *global* value in its own internal
        // `self.location` (what `PointerHandle::current_location()` later
        // returns). Subtracting `origin` here as well, before this call,
        // fed the client `pos - origin - origin` - doubly-offset, and
        // wrong in a way that grows with a window's distance from the
        // screen origin - while also corrupting smithay's own idea of
        // "where is the pointer" for anything else that reads
        // `current_location()`. `pos` unmodified, letting smithay subtract
        // `origin` exactly once, is what every other call site in this
        // file (and this same function's own `None`/lock-surface branches)
        // already does correctly.
        pointer.motion(state, Some((surface, origin)), &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    } else {
        pointer.motion(state, None, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    }
    (hit, over_layer_surface, over_content, hovered_id, resolved)
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

    // A no-op whenever no desktop icon is currently being dragged - see
    // `CompState::update_desktop_icon_drag`'s own doc comment.
    state.update_desktop_icon_drag((pos.x as i32, pos.y as i32));
    // Likewise a no-op whenever no marquee selection is in progress.
    state.update_desktop_marquee((pos.x as i32, pos.y as i32));

    // Tells core which monitor the pointer is physically over right now --
    // core has no pointer of its own to know this (see `pointer_monitor`'s
    // own doc comment), and `add_window`'s target-monitor fallback needs
    // it for the one case the *focused* window's monitor can't answer:
    // nothing focused on whichever monitor the user is actually at when
    // launching something new. `full_geometry`, not the bar-shrunk
    // `geometry` - this is "which physical screen is this pixel on", not
    // a work-area question. `None` if the pointer is somehow outside every
    // known monitor (shouldn't happen given `UdevState::bounds()` already
    // clamps to their union, but a real `None` here is honest rather than
    // guessing).
    {
        let mut wm = state.wm.borrow_mut();
        let current = wm.monitors().iter().find(|m| m.full_geometry.contains_point(pos.x as i32, pos.y as i32)).map(|m| m.id);
        wm.set_pointer_monitor(current);
    }
    let (hit, over_layer_surface, over_content, hovered_id, _) = refresh_pointer_focus(state, pos, time);
    // The only pointer-position telemetry this compositor exposes to
    // anything outside itself - kept at `trace` (off by default, `RUST_LOG`
    // enables it same as any other target here) rather than removed
    // outright: a peer session building against this compositor over IPC
    // pointed out that without *some* "where is the pointer right now" oracle,
    // a synthetic-input tool has no way to tell whether it moved the pointer
    // at all versus landed somewhere unexpected, short of corner-clamping (4
    // fixed points) or hover feedback (binary, only over a reactive widget).
    // Every earlier version of this line ran at `warn`, unconditionally on --
    // see `docs/TODO.md`'s matching cleanup entry for why that was too loud
    // for daily use, not for why the telemetry itself was ever the problem.
    log::trace!("pointer motion pos={:?} hit={hit:?}", (pos.x, pos.y));
    // Titlebar button hover highlighting (explicitly requested, see
    // docs/TODO.md) - `Drag`/`Resize` aren't buttons, so only the three
    // real ones count. Compared against the previous value rather than
    // set unconditionally so an unchanged hover (the overwhelmingly common
    // case: most motion events land on the same button, or on none at all)
    // doesn't force a redraw every single pointer-motion event.
    let new_hover = hit.and_then(|(id, h)| matches!(h, srdwm_core::TitlebarHit::Close | srdwm_core::TitlebarHit::Minimize | srdwm_core::TitlebarHit::Maximize).then_some((id, h)));
    // Compared as just `(id, hit)`, ignoring the `Instant` already stored
    // - the field itself carries a timestamp, but "is this the same hover
    // as before" must not depend on it, or every motion event within the
    // same button would read as a *new* hover and keep resetting the
    // glyph-reveal animation's own start time back to zero.
    let currently_hovering = state.hovered_titlebar_button.map(|(id, h, _)| (id, h));
    if new_hover != currently_hovering {
        let old = state.hovered_titlebar_button.take();
        state.hovered_titlebar_button = new_hover.map(|(id, h)| (id, h, std::time::Instant::now()));
        // Both windows need a fresh signature check: the newly-hovered one
        // (to actually draw the highlight) and the previously-hovered one,
        // if it's a *different* window, to clear its own highlight again.
        if let Some((id, _, _)) = old {
            state.redraw_decoration_buffer(id);
        }
        if let Some((id, _)) = new_hover {
            state.redraw_decoration_buffer(id);
        }
    }
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
    // Captured now, while `wm` is already borrowed, and acted on further
    // down after `drop(wm)` - `redraw_decoration_buffer` needs `&mut
    // state` as a whole, which can't happen while `state.wm`'s own
    // `RefMut` is still alive.
    let resizing_id = wm.resizing_window();
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
    // Keeps the border/titlebar bitmap tracking an active resize drag's own
    // live geometry (see `state::geometry::effective_frame_of`'s doc
    // comment) instead of only catching up once the drag ends - throttled
    // against `RESIZE_REDRAW_INTERVAL` since motion events can arrive far
    // faster than a redraw is worth paying for. Reset to `None` on every
    // tick that isn't resizing this exact window, so a later resize's first
    // motion event always redraws immediately rather than inheriting a
    // stale timestamp from a previous drag (or from dragging, which shares
    // `dragging_or_resizing` above but never touches `resizing_id`).
    if let Some(id) = resizing_id {
        let now = std::time::Instant::now();
        let due = state.resize_redraw_at.is_none_or(|t| now.duration_since(t) >= RESIZE_REDRAW_INTERVAL);
        if due {
            state.redraw_decoration_buffer(id);
            state.resize_redraw_at = Some(now);
        }
    } else {
        state.resize_redraw_at = None;
    }
}

/// Sets the pointer to a resize-direction shape while hovering (or
/// actively dragging) one of our own decoration's resize edges, and back
/// to the default arrow when leaving our decoration for anything else.
///
/// Only ever *forces* `cursor_status` for our own decoration - never while
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
///
/// `state.decoration_cursor_active` is what makes the "leave it alone"
/// branch below safe rather than sticky: reported live as "the resize icon
/// stays on screen long after the pointer is nowhere near an edge." Moving
/// from a decoration edge onto plain content sets no new `wl_pointer` focus
/// (an undecorated/CSD window's edge and its content are the same surface,
/// just different bands of it - see `hit_test`'s `UNDECORATED_TOP_RESIZE_
/// MARGIN`), so the client never gets an `enter` event to react to, and most
/// toolkits only re-call `set_cursor` when *their own* idea of which widget
/// is hovered changes - which it hasn't, from their point of view, since
/// they were never told the pointer was ever over a resize edge to begin
/// with. The resize icon we forced while hovering that edge was therefore
/// never going to be overwritten by anything, ever, without this: the very
/// first content tick after leaving a decoration/resize hover resets to the
/// plain arrow *once*, and only if we're the one who last set it - a
/// client that has since claimed the cursor itself (tracked by `cursor_
/// image` in `protocols.rs` clearing this same flag) is left alone on every
/// following tick, so this can't fight a legitimate client cursor that
/// isn't changing simply because the pointer kept moving.
fn update_cursor_shape(state: &mut CompState, hit: Option<(WindowId, TitlebarHit)>, over_layer_surface: bool, over_content: bool) {
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};

    if over_layer_surface {
        return;
    }
    let edge = match hit {
        Some((_, TitlebarHit::Resize(edge))) => Some(edge),
        _ => state.wm.borrow().resize_edge(),
    };
    // A live "forcing the cursor here has no visible effect" report earlier
    // this session turned out to be a real *design* gap, not a rendering
    // bug: hovering a titlebar button used to fall into the same
    // `CursorIcon::Default` branch as the plain drag area below it --
    // indistinguishable from "not hovering anything special" (the desktop's
    // own baseline cursor is also `Default`), so there was never any visible
    // change to notice in the first place. `Pointer` (a real hand/finger
    // cursor - see `cursor.rs`'s own `pointer_bitmap`) is what every
    // mainstream desktop shows over a clickable titlebar button instead.
    let icon = match edge {
        Some(edge) => resize_cursor_icon(edge),
        // A real button (Close/Maximize/Minimize), not the plain drag
        // area - a hand cursor, matching every mainstream desktop's own
        // convention for a clickable titlebar control.
        None if matches!(hit, Some((_, TitlebarHit::Close | TitlebarHit::Maximize | TitlebarHit::Minimize))) => CursorIcon::Pointer,
        // Hovering our own decoration but not an edge or a button (the
        // drag area) and not actively resizing: back to the plain arrow.
        None if hit.is_some() => CursorIcon::Default,
        // Over a client's own content: leave `cursor_status` alone once the
        // client has claimed it - but if we're still showing whatever we
        // last forced (a resize icon from the edge just left), reset it
        // back to the plain arrow this one time rather than leaving it
        // stuck, since nothing else is ever going to.
        None if over_content => {
            if state.decoration_cursor_active {
                state.cursor_status = CursorImageStatus::Named(CursorIcon::Default);
                state.decoration_cursor_active = false;
            }
            return;
        }
        // Bare desktop: nothing else will ever reset this, so we have to.
        None => CursorIcon::Default,
    };
    state.cursor_status = CursorImageStatus::Named(icon);
    state.decoration_cursor_active = true;
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

pub(crate) fn handle_pointer_button(state: &mut CompState, pos: Point<f64, Logical>, button: u32, pressed: bool, time: u32) {
    notify_idle_activity(state);
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;
    const BTN_MIDDLE: u32 = 0x112;
    let serial = SERIAL_COUNTER.next_serial();

    // Locked: srdwm's own native lock UI has no real `wl_surface` to
    // dispatch a pointer event to - its on-screen keyboard is hit-tested
    // directly instead, on a left-button press, before falling through to
    // the generic forward-to-lock-surface path below (which still applies
    // for an external `LockSurface`-based locker, or a native lock with
    // the keyboard hidden/absent).
    if state.lock.locked {
        if pressed && button == BTN_LEFT && state.lock.native.is_some() && state.native_lock_click(pos) {
            return;
        }
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
                // A separator or section-header row occupies real space
                // (`row_at` resolves a click on it same as any other) but
                // isn't a real action - same "click does nothing, menu
                // stays open" convention any native menu's own divider/
                // caption follows, rather than either running a no-op
                // action or dismissing the whole menu on what was very
                // possibly a slightly-off click at a real item just
                // above/below it.
                if !action.is_interactive() {
                    state.context_menu = Some(menu);
                    return;
                }
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
        // Same rule again for the desktop-icon/bare-desktop menu.
        if let Some(menu) = state.desktop_menu.take() {
            if let Some(row) = menu.row_at(pos.x as i32, pos.y as i32) {
                let (_, action) = menu.items[row].clone();
                if matches!(action, crate::desktop_menu::DesktopMenuAction::Separator) {
                    state.desktop_menu = Some(menu);
                    return;
                }
                state.close_desktop_menu();
                state.run_desktop_menu_action(action);
            } else {
                state.close_desktop_menu();
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
        let hit = if layer_hit.is_some() { None } else { hit_test_animated(state, pos.x as i32, pos.y as i32) };
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
            } else {
                // Genuinely bare desktop (or below every window, which
                // only ever means bare desktop - icons render below every
                // window, see `desktop_icons.rs`'s own module doc
                // comment): a desktop icon here, single- or double-click
                // per `general.desktop_icon_single_click`, otherwise clear
                // whatever was selected.
                let icon_hit = state
                    .desktop_icons
                    .as_ref()
                    .and_then(|icons| icons.icon_at(pos.x as i32, pos.y as i32).map(|(i, origin)| (icons.icons[i].id.clone(), origin)));
                match icon_hit {
                    Some((id, origin)) => {
                        let single_click_opens = state.wm.borrow().desktop_icon_single_click;
                        if single_click_opens || state.is_double_click_icon(&id, time) {
                            state.select_desktop_icon(Some(&id));
                            state.open_desktop_icon(&id);
                        } else {
                            // Don't collapse an existing multi-selection
                            // just because the drag grabbed one of its own
                            // members - `start_desktop_icon_drag` itself
                            // carries every currently-selected icon along
                            // when the one grabbed is already selected
                            // (see its own doc comment), the same "drag one
                            // of several selected files, they all move"
                            // convention every real desktop uses. Grabbing
                            // an icon *outside* the current selection still
                            // replaces it, same as before.
                            let already_selected = state.desktop_icons.as_ref().is_some_and(|icons| icons.icons.iter().any(|i| i.id == id && i.selected));
                            if !already_selected {
                                state.select_desktop_icon(Some(&id));
                            }
                            state.start_desktop_icon_drag(&id, origin, (pos.x as i32, pos.y as i32));
                        }
                    }
                    // Genuinely bare desktop, not just "no icon under the
                    // pointer" - starts a rubber-band selection instead of
                    // only clearing whatever was selected before. The one
                    // "click and drag" desktop interaction this compositor
                    // never had (reported live next to "missing click and
                    // drag stuff like from windows").
                    None => state.start_desktop_marquee((pos.x as i32, pos.y as i32)),
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
        let hit = hit_test_animated(state, pos.x as i32, pos.y as i32);
        match (button, hit) {
            (BTN_RIGHT, Some((id, TitlebarHit::Drag))) => state.open_context_menu(id, (pos.x as i32, pos.y as i32)),
            (BTN_MIDDLE, Some((id, TitlebarHit::Drag))) => state.wm.borrow_mut().lower_window(id),
            // Right-click the maximize button itself: the Snap-Layouts
            // flyout (pick a half/quarter position for this window)
            // instead of the window menu - a plain left-click there still
            // just toggles maximize, unchanged.
            (BTN_RIGHT, Some((id, TitlebarHit::Maximize))) => state.open_snap_flyout(id, (pos.x as i32, pos.y as i32)),
            // Right-click bare desktop (no titlebar/border hit, no window
            // content, no bar/dock layer surface - same "genuinely bare"
            // definition the left-click branch above uses): a desktop
            // icon's own menu if the click landed on one, otherwise the
            // "New Folder"/"Refresh" desktop menu. Previously a true
            // no-op, the actual gap this whole feature exists to close --
            // see this session's own TODO.md entry.
            (BTN_RIGHT, None)
                if layer_surface_under(state, pos).is_none() && !state.space.element_under(pos).is_some_and(|(w, _)| dwindow_is_visible(state, w)) =>
            {
                let icon_hit = state.desktop_icons.as_ref().and_then(|icons| icons.icon_at(pos.x as i32, pos.y as i32).map(|(i, _origin)| icons.icons[i].id.clone()));
                match icon_hit {
                    Some(id) => {
                        state.select_desktop_icon(Some(&id));
                        state.open_desktop_icon_menu(&id, (pos.x as i32, pos.y as i32));
                    }
                    None => state.open_desktop_menu((pos.x as i32, pos.y as i32)),
                }
            }
            _ => {}
        }
    } else if !pressed {
        // A no-op via `Option::take()` when no icon drag was active --
        // always checked on release, same as `was_dragging`/`was_resizing`
        // below, just for a desktop icon instead of a window.
        state.end_desktop_icon_drag();
        state.end_desktop_marquee();
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
        // Persists whatever `end_drag`/`end_resize` just updated in
        // `remembered_geometry` - a real user action (button released
        // after a drag/resize), not a per-frame event, so writing the
        // whole small table each time is cheap and needs no separate
        // dirty-tracking. See `window_memory.rs`'s own doc comment.
        if was_dragging || was_resizing {
            crate::window_memory::save_all(wm.all_remembered_geometry());
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
    //
    // Also where `pointer_button_grab` starts and ends - see its own doc
    // comment. Only the 0->1 transition captures a new grab target (a
    // second button going down mid-gesture keeps whatever the first press
    // already locked in); only the ->0 transition releases it, and not
    // before this press/release's own `pointer.button()` below still goes
    // out under the (still-active) grab.
    let (.., resolved) = refresh_pointer_focus(state, pos, time);
    if pressed {
        if state.pointer_buttons_held == 0 {
            state.pointer_button_grab = resolved;
        }
        state.pointer_buttons_held += 1;
    } else {
        state.pointer_buttons_held = state.pointer_buttons_held.saturating_sub(1);
    }
    if let Some(pointer) = state.seat.get_pointer() {
        let button_state = if pressed { BackendButtonState::Pressed } else { BackendButtonState::Released };
        pointer.button(state, &ButtonEvent { serial, time, button, state: button_state });
        // See the matching comment in `handle_pointer_position`: `button`
        // alone never tells the client the event is ready to act on, only
        // `frame` does.
        pointer.frame(state);
    }
    if !pressed && state.pointer_buttons_held == 0 {
        state.pointer_button_grab = None;
    }
}
