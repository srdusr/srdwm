//! `wlr-foreign-toplevel-management-unstable-v1`: enumerates every managed
//! window (title, app_id, activated/maximized/minimized state) to any
//! client that binds it, and lets that client request activation, close,
//! maximize/minimize - what a dock or taskbar needs to be interactive
//! rather than just a launcher. Requested by AGS's dock (see
//! `docs/PANEL_SUPPORT_TODO.md`'s P1 list): without it there is no running-
//! app indicator, no click-to-focus in the dock, no alt-tab list.
//!
//! Chose this over the newer split `ext_foreign_toplevel_list_v1` (list
//! only) + a separate management protocol: the wlr protocol covers
//! enumeration *and* activate/close/maximize/minimize in one interface, and
//! it is what was actually asked for - smithay 0.7 has no built-in helper
//! for either shape, but has one for the `ext_` list-only half, which would
//! have meant standing up two protocols to get what this one already does
//! alone. Hand-written against the raw `wayland-protocols-wlr` server
//! bindings, same reasoning and the same crate as `screencopy.rs`.
//!
//! One `ZwlrForeignToplevelHandleV1` is created per (bound manager, window)
//! pair - a dock running in one client only ever sees the handles that
//! client's own manager object created, so `CompState` tracks a `Vec` of
//! live handles per `WindowId` to broadcast state changes (activation,
//! maximized, minimized, fullscreen) to every one of them, from whichever
//! call site actually changed that state - this module's own requests,
//! the pointer-driven titlebar handlers, or a client's own
//! `xdg_toplevel`/X11 request. See `update_activated`'s doc comment for
//! the one trigger that still doesn't re-broadcast.
//!
//! Override-redirect X11 windows are deliberately never announced here,
//! matching `_NET_CLIENT_LIST` and ICCCM: a dock has no business listing
//! menus/tooltips/drag images as if they were real applications.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::{self, State, ZwlrForeignToplevelHandleV1};
use wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1};

use srdwm_core::WindowId;

use crate::input::{close_dwindow, focus_window};
use crate::state::CompState;

const PROTOCOL_VERSION: u32 = 3;

pub struct ForeignToplevelState {
    _global: smithay::reexports::wayland_server::backend::GlobalId,
}

impl ForeignToplevelState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, ZwlrForeignToplevelManagerV1, _>(PROTOCOL_VERSION, ()) }
    }
}

/// Per-handle data: which window this specific `ZwlrForeignToplevelHandleV1`
/// (one of possibly several, one per bound manager) speaks for.
pub struct ToplevelHandleData {
    window: WindowId,
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for CompState {
    fn bind(state: &mut Self, dh: &DisplayHandle, client: &Client, manager: New<ZwlrForeignToplevelManagerV1>, _data: &(), data_init: &mut DataInit<'_, Self>) {
        let manager = data_init.init(manager, ());
        // Catch this client's dock/switcher up on every window that already
        // exists - without this, only windows opened *after* the client
        // bound the global would ever be announced.
        let existing: Vec<WindowId> = state.wm.borrow().windows().map(|w| w.id).collect();
        for id in existing {
            announce(state, &manager, id, client, dh);
        }
        state.foreign_toplevel_managers.push(manager);
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for CompState {
    fn request(state: &mut Self, _client: &Client, manager: &ZwlrForeignToplevelManagerV1, request: zwlr_foreign_toplevel_manager_v1::Request, _data: &(), _dh: &DisplayHandle, _data_init: &mut DataInit<'_, Self>) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            manager.finished();
            state.foreign_toplevel_managers.retain(|m| m != manager);
        }
    }

    fn destroyed(state: &mut Self, _client: smithay::reexports::wayland_server::backend::ClientId, manager: &ZwlrForeignToplevelManagerV1, _data: &()) {
        state.foreign_toplevel_managers.retain(|m| m != manager);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ToplevelHandleData> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _handle: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        data: &ToplevelHandleData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let id = data.window;
        // Every branch that actually changes state re-announces it
        // (`send_state`) rather than waiting for some other path to notice
        // - `activate`/`close` are the two the dock actually needs (per
        // `docs/PANEL_SUPPORT_TODO.md`), the rest are handled for protocol
        // completeness since a dock is free to send them (a maximize
        // context-menu entry, say) even if AGS's dock doesn't today.
        match request {
            zwlr_foreign_toplevel_handle_v1::Request::Activate { seat: _ } => {
                // No per-seat distinction - this compositor has exactly
                // one seat, same simplification `srd.window.focus()` and
                // every other single-window-target API already makes.
                focus_window(state, id);
            }
            zwlr_foreign_toplevel_handle_v1::Request::Close => {
                if let Some(w) = state.id_to_window.get(&id) {
                    close_dwindow(w);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Request::SetMaximized => set_maximized(state, id, true),
            zwlr_foreign_toplevel_handle_v1::Request::UnsetMaximized => set_maximized(state, id, false),
            zwlr_foreign_toplevel_handle_v1::Request::SetMinimized => set_minimized(state, id, true),
            zwlr_foreign_toplevel_handle_v1::Request::UnsetMinimized => set_minimized(state, id, false),
            // `output` is a hint only (per the protocol doc: "only a hint to
            // the compositor") - ignored, same as `fullscreen_request`'s
            // `_output` in `protocols.rs`, and for the same reason: single-
            // seat, single-fullscreen-path, matching every other fullscreen
            // entry point instead of introducing an output-aware one just
            // for this request.
            zwlr_foreign_toplevel_handle_v1::Request::SetFullscreen { output: _ } => set_fullscreen(state, id, true),
            zwlr_foreign_toplevel_handle_v1::Request::UnsetFullscreen => set_fullscreen(state, id, false),
            // The rectangle hint and Destroy: no srdwm-side effect. The
            // rectangle is an optional hint we're not obligated to act on;
            // `Destroy` just ends this protocol object, handled by
            // `destroyed` below.
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: smithay::reexports::wayland_server::backend::ClientId, handle: &ZwlrForeignToplevelHandleV1, data: &ToplevelHandleData) {
        if let Some(handles) = state.foreign_toplevel_handles.get_mut(&data.window) {
            handles.retain(|h| h != handle);
        }
    }
}

fn set_maximized(state: &mut CompState, id: WindowId, maximized: bool) {
    let already = state.wm.borrow().window(id).is_some_and(|w| w.maximized);
    if already != maximized {
        state.wm.borrow_mut().toggle_maximize(id);
        state.sync_geometry(id);
    }
    send_state(state, id);
}

fn set_minimized(state: &mut CompState, id: WindowId, minimized: bool) {
    let mut wm = state.wm.borrow_mut();
    if minimized {
        wm.minimize_window(id);
    } else {
        wm.restore_window(id);
    }
    drop(wm);
    send_state(state, id);
}

/// Same `redraw_decoration_buffer`-before-`sync_geometry` ordering as every
/// other fullscreen entry point (`protocols.rs`'s `fullscreen_request`,
/// `xwayland.rs`'s equivalent): `toggle_fullscreen` also flips
/// `Window.decorated`, and the decoration buffer needs to actually be
/// dropped, not just left stale for `sync_geometry`'s resize-only redraw
/// check to skip.
fn set_fullscreen(state: &mut CompState, id: WindowId, fullscreen: bool) {
    let already = state.wm.borrow().is_fullscreen(id);
    if already != fullscreen {
        state.wm.borrow_mut().toggle_fullscreen(id);
        state.redraw_decoration_buffer(id);
        state.sync_geometry(id);
    }
    send_state(state, id);
}

/// Creates and sends a new handle for `id` to `manager`, with its full
/// initial state (title, app_id, activated/maximized/minimized, done) --
/// the sequence `new_toplevel`'s own doc comment requires: "all initial
/// details... will be sent immediately after this event".
fn announce(state: &mut CompState, manager: &ZwlrForeignToplevelManagerV1, id: WindowId, client: &Client, dh: &DisplayHandle) {
    let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, ToplevelHandleData, CompState>(dh, manager.version(), ToplevelHandleData { window: id }) else {
        return;
    };
    manager.toplevel(&handle);
    state.foreign_toplevel_handles.entry(id).or_default().push(handle.clone());
    send_state_to(state, id, &[handle]);
}

/// Re-sends title/app_id/state/done to every handle a window currently has
/// - one per bound manager. Used both right after `announce` creates a
/// fresh handle and whenever state actually changes (`set_maximized`,
/// `update_activated`).
pub(crate) fn send_state(state: &mut CompState, id: WindowId) {
    let Some(handles) = state.foreign_toplevel_handles.get(&id).cloned() else { return };
    send_state_to(state, id, &handles);
}

fn send_state_to(state: &mut CompState, id: WindowId, handles: &[ZwlrForeignToplevelHandleV1]) {
    let Some(w) = state.wm.borrow().window(id).cloned() else { return };
    let focused = state.wm.borrow().focused_id() == Some(id);
    let fullscreen = state.wm.borrow().is_fullscreen(id);
    let bytes = state_flags(w.maximized, w.minimized, focused, fullscreen);

    for handle in handles {
        handle.title(w.title.clone());
        handle.app_id(w.app_id.clone());
        handle.state(bytes.clone());
        handle.done();
    }
}

/// Packs the flags a window currently has into the wire format
/// `zwlr_foreign_toplevel_handle_v1.state` expects: the raw bytes of
/// however many native-endian u32 enum values apply, back to back. Pulled
/// out of `send_state_to` as the one piece of this module that's pure
/// logic rather than protocol I/O, so it's unit-testable the way
/// `decoration.rs`'s rasterizers are.
fn state_flags(maximized: bool, minimized: bool, activated: bool, fullscreen: bool) -> Vec<u8> {
    let mut flags = Vec::new();
    if maximized {
        flags.push(State::Maximized as u32);
    }
    if minimized {
        flags.push(State::Minimized as u32);
    }
    if activated {
        flags.push(State::Activated as u32);
    }
    if fullscreen {
        flags.push(State::Fullscreen as u32);
    }
    let mut bytes = Vec::with_capacity(flags.len() * 4);
    for f in flags {
        bytes.extend_from_slice(&f.to_ne_bytes());
    }
    bytes
}

/// Called from `new_managed_window`/`finish_x11_window_setup`. Announces
/// the new window to every currently-bound manager - `GlobalDispatch::bind`
/// above only covers managers that bind *before* a window exists; this is
/// the other half, for ones that were already bound.
pub(crate) fn window_created(state: &mut CompState, id: WindowId) {
    let managers = state.foreign_toplevel_managers.clone();
    for manager in &managers {
        let Some(client) = manager.client() else { continue };
        let dh = state.dh.clone();
        announce(state, manager, id, &client, &dh);
    }
}

/// Called from the native/X11 `remove_window` paths. Every real Wayland
/// compositor's foreign-toplevel implementation must send `closed` before
/// a handle becomes unusable - skipping it would leave a dock's entry for
/// this window dangling with no signal to ever remove it.
pub(crate) fn window_closed(state: &mut CompState, id: WindowId) {
    if let Some(handles) = state.foreign_toplevel_handles.remove(&id) {
        for handle in handles {
            handle.closed();
        }
    }
}

/// Called from `set_keyboard_focus`, the single chokepoint every focus
/// change already goes through - re-announces state for whichever window
/// lost `Activated` and whichever gained it, so a dock's focused-app
/// highlight tracks real focus instead of only ever reflecting whatever was
/// focused at creation time.
///
/// Only `Activated` is kept live this way - maximized/minimized/fullscreen
/// changes are re-broadcast from their own call sites instead (the pointer-
/// driven titlebar handlers in `input.rs`, and the client-request handlers
/// in `protocols.rs`/`xwayland.rs`, all call `send_state` directly). The one
/// trigger that still doesn't re-broadcast is a *compositor keybinding*
/// (`srd.window.maximize()`/`.fullscreen()`/`.minimize()`): `crates/config`
/// only holds a `WindowManager` reference, not `CompState`, so it has no way
/// to reach this module. See `docs/PANEL_SUPPORT_TODO.md`'s P1 section.
pub(crate) fn update_activated(state: &mut CompState, old: Option<WlSurface>, new: Option<&WlSurface>) {
    let old_id = old.as_ref().and_then(|s| window_id_for_surface(state, s));
    let new_id = new.and_then(|s| window_id_for_surface(state, s));
    if old_id == new_id {
        return;
    }
    if let Some(id) = old_id {
        send_state(state, id);
    }
    if let Some(id) = new_id {
        send_state(state, id);
    }
}

fn window_id_for_surface(state: &CompState, surface: &WlSurface) -> Option<WindowId> {
    state.surface_to_id.get(surface).copied()
}

/// Diff-broadcasts maximized/minimized/fullscreen against what was last
/// sent for each window, called once per frame from `CompState::
/// tick_dirty_broadcasts` (same poll-loop cadence as `tick_animations`).
///
/// Every *known* trigger already re-broadcasts immediately at its own call
/// site: this module's own requests (`set_maximized`/`set_minimized`/
/// `set_fullscreen`), the pointer-driven titlebar handlers in `input.rs`,
/// and a client's own `xdg_toplevel`/X11 request in `protocols.rs`/
/// `xwayland.rs`. The one trigger documented as a real gap in
/// `docs/PANEL_SUPPORT_TODO.md` is a *compositor keybinding*
/// (`srd.window.maximize()`/`.fullscreen()`/`.minimize()`): `crates/config`
/// is the platform-agnostic scripting engine, shared with the X11/macOS
/// backends, and only ever holds a `WindowManager` reference - it cannot
/// reach this Wayland-protocol-specific module directly, so state it
/// changes went stale in every dock/panel until *something else*
/// coincidentally re-broadcast that window. Rather than thread a callback
/// through `WindowManager` for every future call site that might change
/// this state (and inevitably miss the next one), this diffs against
/// `WindowManager`'s live state once a frame and catches all of them
/// uniformly, including ones that don't exist yet.
///
/// One accepted redundancy: a brand-new window has no prior entry in
/// `last_broadcast_flags`, so its first tick after creation sends one
/// extra (identical) `send_state` on top of the one `window_created`
/// already sent - harmless per-window, one-time, and far simpler than
/// seeding the cache at creation just to suppress it.
pub(crate) fn broadcast_dirty_state(state: &mut CompState) {
    let current: Vec<(WindowId, bool, bool, bool)> =
        state.wm.borrow().windows().map(|w| (w.id, w.maximized, w.minimized, w.fullscreen)).collect();
    let mut changed = Vec::new();
    for (id, maximized, minimized, fullscreen) in &current {
        let key = (*maximized, *minimized, *fullscreen);
        if state.last_broadcast_flags.get(id) != Some(&key) {
            state.last_broadcast_flags.insert(*id, key);
            changed.push(*id);
        }
    }
    // Drops entries for windows that no longer exist - `window_closed`
    // already tells clients the handle itself is gone; this just stops the
    // map growing forever across the life of a session.
    let live: std::collections::HashSet<WindowId> = current.iter().map(|(id, ..)| *id).collect();
    state.last_broadcast_flags.retain(|id, _| live.contains(id));
    for id in changed {
        send_state(state, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_u32s(bytes: &[u8]) -> Vec<u32> {
        bytes.chunks_exact(4).map(|c| u32::from_ne_bytes(c.try_into().unwrap())).collect()
    }

    #[test]
    fn no_flags_is_an_empty_array() {
        assert_eq!(state_flags(false, false, false, false), Vec::<u8>::new());
    }

    #[test]
    fn each_flag_packs_its_own_enum_value() {
        assert_eq!(as_u32s(&state_flags(true, false, false, false)), vec![State::Maximized as u32]);
        assert_eq!(as_u32s(&state_flags(false, true, false, false)), vec![State::Minimized as u32]);
        assert_eq!(as_u32s(&state_flags(false, false, true, false)), vec![State::Activated as u32]);
        assert_eq!(as_u32s(&state_flags(false, false, false, true)), vec![State::Fullscreen as u32]);
    }

    #[test]
    fn all_flags_combine_in_one_array() {
        let values = as_u32s(&state_flags(true, true, true, true));
        assert_eq!(values.len(), 4);
        assert!(values.contains(&(State::Maximized as u32)));
        assert!(values.contains(&(State::Minimized as u32)));
        assert!(values.contains(&(State::Activated as u32)));
        assert!(values.contains(&(State::Fullscreen as u32)));
    }
}
