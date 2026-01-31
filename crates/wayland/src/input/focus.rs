//! Focusing, raising, and closing a window - the small set of helpers
//! every other input-handling module needs regardless of what triggered
//! the focus change (a click, a keybinding, an IPC call, a closed window's
//! fallback).

use smithay::desktop::Window as DWindow;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use srdwm_core::{Event as CoreEvent, WindowId};

use crate::state::CompState;

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
pub(super) fn dwindow_is_visible(state: &CompState, w: &DWindow) -> bool {
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
    // Real desktop convention (Windows/GNOME/macOS all do this): a
    // selected desktop icon stays highlighted only until something else
    // takes focus. Reported live as "highlighted desktop icons don't
    // become not highlighted anymore" when clicking a window - `select_
    // desktop_icon(None)` (clearing selection) was only ever called from
    // `start_desktop_marquee` (a new marquee-select on bare desktop), never
    // from the one place every focus path already funnels through
    // regardless of how it got triggered (a click, Alt-Tab, a dock's IPC
    // focus dispatch, scratchpad show, ...) - see this function's own
    // doc comment on why that funnel already exists for raising. A no-op,
    // cheap, when nothing was selected to begin with.
    state.select_desktop_icon(None);
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
    raise_in_space(state, id);
    state.pending.borrow_mut().push(CoreEvent::WindowFocused(id));
    let surface = state.id_to_window.get(&id).and_then(dwindow_wl_surface);
    // Routed through `set_keyboard_focus` (rather than calling
    // `KeyboardHandle::set_focus` directly) so clipboard/primary-selection
    // focus follows window focus too - see that method's doc comment.
    state.set_keyboard_focus(surface);
}

/// Raises `id` to the top of smithay's own `Space` stacking order (see
/// `focus_window`'s doc comment for why `Space`'s own order, separate from
/// core's, has to be kept in sync) without touching core's focus or
/// workspace state at all.
///
/// Split out of `focus_window` specifically so the udev/winit backends'
/// post-IPC-mutation re-sync (`crate::input::focus_window`'s doc comment
/// on *that* call site explains why it exists at all: an IPC-only focus
/// change needs `Space` to catch up too) can re-raise the already-focused
/// window without going through `WindowManager::focus_window` a second
/// time. That core method has its own side effect of switching to the
/// target's workspace if it differs from the current one - correct for a
/// real focus change, but wrong here: calling it on a window that is
/// already focused (just re-raising it for `Space`'s benefit) compared the
/// still-current, still-on-its-old-workspace window against whatever
/// `current_workspace` had just been set to by the same IPC mutation this
/// re-sync is reacting to, and silently switched it right back --
/// confirmed live as `srd dispatch activate workspace <id>` (and, by
/// extension, any AGS workspace-switcher click going through the same IPC
/// path) visibly changing `current_workspace` for a moment and then
/// reverting within milliseconds, every time, unless the same IPC call
/// also happened to change which window was focused. Exactly the same bug
/// `main.rs`'s `sync()` was already fixed for (see its own doc comment) --
/// this is a second call site with the identical unconditional-`focus_
/// window`-reassertion shape, never fixed at the same time.
pub(crate) fn raise_in_space(state: &mut CompState, id: WindowId) {
    if let Some(w) = state.id_to_window.get(&id).cloned() {
        state.space.raise_element(&w, true);
        state.raise_pinned();
    }
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
