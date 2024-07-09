//! smithay protocol-handler implementations for [`CompState`], plus the
//! `delegate_*!` macros that route each protocol's dispatch to them.
//!
//! Deliberately thin: these methods translate a protocol event into a call
//! on [`crate::state`] (window bookkeeping) or [`crate::input`] (focus), and
//! hold no logic of their own beyond what the protocol itself dictates. The
//! session-lock handler is the one exception, living in [`crate::lock`]
//! alongside the rest of that feature.

use smithay::desktop::{find_popup_root_surface, layer_map_for_output, LayerSurface as DesktopLayerSurface, PopupKeyboardGrab, PopupKind, PopupPointerGrab};
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::Resource;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::ImportDma;
use smithay::utils::Serial;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};
use smithay::wayland::input_method::PopupSurface as ImePopupSurface;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::primary_selection::{PrimarySelectionHandler, PrimarySelectionState};
use smithay::wayland::selection::wlr_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::compositor::{add_pre_commit_hook, with_states};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, LayerSurfaceCachedState, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::{
    delegate_compositor, delegate_cursor_shape, delegate_data_control, delegate_data_device, delegate_dmabuf,
    delegate_layer_shell, delegate_output, delegate_primary_selection, delegate_seat, delegate_session_lock,
    delegate_shm, delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
    delegate_input_method_manager, delegate_text_input_manager,
};

use crate::state::{ClientState, CompState};

impl CompositorHandler for CompState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Two possible client kinds now: our own `ClientState` for regular
        // Wayland clients, or smithay's `XWaylandClientData` for the single
        // XWayland client (see `xwayland.rs`) - both carry a
        // `CompositorClientState`, just under different wrapper types.
        if let Some(state) = client.get_data::<ClientState>() {
            return &state.compositor_state;
        }
        &client.get_data::<smithay::xwayland::XWaylandClientData>().expect("client is neither ours nor XWayland's").compositor_state
    }

    /// Workaround for a real smithay bug (see docs/PANEL_SUPPORT_TODO.md and
    /// `layer_destroyed` below): destroying a `zwlr_layer_surface_v1` role
    /// resets the surface's `LayerSurfaceCachedState` to
    /// `Default::default()` (size 0x0, no anchor) rather than removing it,
    /// but the pre-commit hook smithay itself registers at
    /// `get_layer_surface` time keeps validating that state against every
    /// future commit regardless of whether the role still exists --
    /// tripping its own `width/height 0 requested without ... anchors`
    /// check and posting `invalid_size`, which kills the client's whole
    /// connection over what is protocol-legal (committing a now-roleless
    /// surface).
    ///
    /// Fixed by registering our own pre-commit hook here, in `new_surface`
    /// - called at `wl_compositor.create_surface`, strictly before any
    /// later `get_layer_surface` on the same surface could register
    /// smithay's own hook. Hooks run in registration order (`tree.rs`:
    /// `pre_commit_hooks` is a plain `Vec`, pushed and iterated in order),
    /// so ours always runs first and can neutralize the stale reset state
    /// before smithay's hook ever inspects it. This depends on that
    /// ordering guarantee holding in future smithay versions - it isn't
    /// documented as an API contract, just an implementation detail
    /// confirmed against 0.7.0's source - so re-check this file against
    /// whatever smithay version replaces it.
    ///
    /// Cost: one closure registered per `wl_surface` (not just layer
    /// surfaces, since we don't know in advance which ones will become
    /// one), each a no-op unless that exact surface is in
    /// `dead_layer_surfaces`.
    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<CompState, _>(surface, |state, _dh, surface| {
            if !state.dead_layer_surfaces.contains(surface) {
                return;
            }
            with_states(surface, |states| {
                let mut cached = states.cached_state.get::<LayerSurfaceCachedState>();
                let pending = cached.pending();
                if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
                    pending.size.w = 1;
                }
                if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
                    pending.size.h = 1;
                }
            });
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<CompState>(surface);
        // XWayland's association of an X11 window with this wl_surface can
        // arrive at any point relative to the map request (see
        // `xwayland.rs`'s module docs); `surface_associated` handles the
        // common ordering, this retries the surfaces still waiting on a
        // commit to actually make that association queryable.
        self.retry_pending_x11_windows();
        if let Some(&id) = self.surface_to_id.get(surface) {
            if let Some(w) = self.id_to_window.get(&id) {
                w.on_commit();
            }
            // See `content_epoch`'s doc comment: this is the only per-commit
            // signal the udev backend's rounded-corner mask cache has to
            // invalidate itself, since content can change every frame,
            // independent of the geometry-driven points `redraw_decoration_
            // buffer` already runs at.
            *self.content_epoch.entry(id).or_insert(0) += 1;
            crate::state::sync_toplevel_metadata(self, id, surface);
        }
        self.ensure_layer_initial_configure(surface);
        // Advances a just-created popup from unmapped to mapped (needed for
        // `PopupManager::popups_for_surface`, which `popup_render_elements`
        // reads at render time) and prunes dead ones. Cheap and only does
        // real work on a popup-role surface, so doing it on every commit
        // rather than throttling is not worth the extra bookkeeping.
        self.popups.commit(surface);
        self.popups.cleanup();
    }
}

impl XdgShellHandler for CompState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.new_managed_window(surface);
    }

    /// `move_request`/`resize_request` were also still smithay's default
    /// no-op implementations - a much larger gap than the five below:
    /// this is *how a client-side-decorated window gets dragged or resized
    /// by its own titlebar/edges at all*. A window we draw our own
    /// decoration for never needed this (`TitlebarHit::Drag`/`Resize` in
    /// `input.rs` detect the click directly, since we own those pixels),
    /// but a window that negotiated client-side decoration and draws its
    /// own titlebar - Firefox, and most GTK4 apps by default - handles
    /// the click itself and then asks the compositor to actually perform
    /// the move/resize via exactly these two requests. Left unimplemented,
    /// dragging or resizing any such window by its own chrome did
    /// nothing at all - the only way to reposition it was the
    /// modifier+drag-anywhere gesture (`bindm`), which most users have no
    /// reason to know exists and doesn't work for resize-from-a-specific-
    /// edge at all. Reuses the exact same `WindowManager::start_drag`/
    /// `start_resize` the pointer-driven titlebar handlers call --
    /// `handle_pointer_position`/`handle_pointer_button` already drive any
    /// in-progress drag/resize to completion on subsequent motion/release
    /// regardless of what started it, so no smithay pointer grab is
    /// needed here at all, just the same start call from a different
    /// trigger.
    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        if let Some(&id) = self.surface_to_id.get(surface.wl_surface()) {
            let pos = crate::input::last_pointer_pos(self);
            self.wm.borrow_mut().start_drag(id, pos.x as i32, pos.y as i32);
        }
    }

    fn resize_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        let Some(edge) = (match edges {
            xdg_toplevel::ResizeEdge::Top => Some(srdwm_core::ResizeEdge::Top),
            xdg_toplevel::ResizeEdge::Bottom => Some(srdwm_core::ResizeEdge::Bottom),
            xdg_toplevel::ResizeEdge::Left => Some(srdwm_core::ResizeEdge::Left),
            xdg_toplevel::ResizeEdge::Right => Some(srdwm_core::ResizeEdge::Right),
            xdg_toplevel::ResizeEdge::TopLeft => Some(srdwm_core::ResizeEdge::TopLeft),
            xdg_toplevel::ResizeEdge::TopRight => Some(srdwm_core::ResizeEdge::TopRight),
            xdg_toplevel::ResizeEdge::BottomLeft => Some(srdwm_core::ResizeEdge::BottomLeft),
            xdg_toplevel::ResizeEdge::BottomRight => Some(srdwm_core::ResizeEdge::BottomRight),
            // `None` is a valid protocol value (the client leaves the edge
            // unspecified) but `WindowManager::start_resize` needs one --
            // there's nothing sensible to default it to that wouldn't be a
            // guess, so this is a no-op rather than picking one.
            _ => None,
        }) else {
            return;
        };
        if let Some(&id) = self.surface_to_id.get(surface.wl_surface()) {
            let pos = crate::input::last_pointer_pos(self);
            self.wm.borrow_mut().start_resize(id, edge, pos.x as i32, pos.y as i32);
        }
    }

    /// `maximize_request`/`unmaximize_request`/`fullscreen_request`/
    /// `unfullscreen_request`/`minimize_request` were all still smithay's
    /// default no-op (or configure-only) implementations - found
    /// investigating the `toggle_fullscreen` decoration bug above, by
    /// checking what else routes through the same `WindowManager` calls
    /// the titlebar-button click handlers in `input.rs` already use.
    /// These five are the *client-initiated* equivalent of those clicks: a
    /// client's own window-menu "Maximize", pressing F11, an HTML5 video
    /// going fullscreen, or (for a client that negotiated client-side
    /// decoration and draws its own titlebar, like Firefox) that titlebar's
    /// own maximize button - all ask the compositor to actually perform
    /// the state change via these requests rather than the compositor
    /// noticing on its own. Left unimplemented, every one of them was a
    /// silent no-op: the client's button did nothing, with no error and
    /// nothing to suggest why, from any app that relies on this instead of
    /// (or in addition to) a compositor-side keybinding.
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_id.get(surface.wl_surface()) {
            if !self.wm.borrow().window(id).is_some_and(|w| w.maximized) {
                self.wm.borrow_mut().toggle_maximize(id);
                self.sync_geometry(id);
                crate::foreign_toplevel::send_state(self, id);
            }
        }
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_id.get(surface.wl_surface()) {
            if self.wm.borrow().window(id).is_some_and(|w| w.maximized) {
                self.wm.borrow_mut().toggle_maximize(id);
                self.sync_geometry(id);
                crate::foreign_toplevel::send_state(self, id);
            }
        }
        surface.send_configure();
    }

    /// `_output` (the client's requested target output) is ignored --
    /// single-seat, and every other fullscreen entry point (the titlebar
    /// button, `srd.window.fullscreen()`) already fullscreens on whatever
    /// monitor the window is already on, so this matches that instead of
    /// introducing an output-aware fullscreen path only this one request
    /// would use.
    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if let Some(&id) = self.surface_to_id.get(surface.wl_surface()) {
            if !self.wm.borrow().is_fullscreen(id) {
                // `redraw_decoration_buffer` first, same reason
                // `set_decorated_from_mode` calls it before `sync_geometry`:
                // fullscreen also flips `Window.decorated`, and dropping
                // the decoration needs the buffer actually removed, not
                // just left stale for `sync_geometry`'s own resize-only
                // redraw check to skip.
                self.wm.borrow_mut().toggle_fullscreen(id);
                self.redraw_decoration_buffer(id);
                self.sync_geometry(id);
                crate::foreign_toplevel::send_state(self, id);
            }
        }
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_id.get(surface.wl_surface()) {
            if self.wm.borrow().is_fullscreen(id) {
                self.wm.borrow_mut().toggle_fullscreen(id);
                self.redraw_decoration_buffer(id);
                self.sync_geometry(id);
                crate::foreign_toplevel::send_state(self, id);
            }
        }
        surface.send_configure();
    }

    /// No `send_configure` here, matching the pointer-driven
    /// `TitlebarHit::Minimize` handler in `input.rs`: minimizing doesn't
    /// change the window's own size, only whether it's currently shown, so
    /// there's nothing new to tell the client about its own geometry.
    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_id.get(surface.wl_surface()) {
            self.wm.borrow_mut().minimize_window(id);
            crate::foreign_toplevel::send_state(self, id);
        }
    }

    /// Was a bare no-op - no `send_configure` at all. Per xdg-shell,
    /// `xdg_surface.configure` is required before a popup's first commit;
    /// real toolkits (confirmed live: GTK4's Wayland backend) block that
    /// commit in a synchronous roundtrip waiting for it, so every popup
    /// hung its client forever. GTK4 implements tooltips *and*
    /// `Gtk.Popover` as `xdg_popup`, so this fired on hovering almost any
    /// widget with a tooltip - confirmed by a peer session's gdb backtrace
    /// (blocked in `wl_display_dispatch_queue` under `gtk_widget_show`)
    /// after AGS wedged.
    ///
    /// Geometry is `positioner.get_geometry()` un-constrained - no
    /// on-screen clamping yet (`PositionerState::get_unconstrained_geometry`
    /// needs a target rect in the parent's surface-local space, which is a
    /// real follow-up, not this fix); an occasional popup placed near a
    /// screen edge may render partly off it, which is cosmetic, not a hang.
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        if surface.send_configure().is_err() {
            return;
        }
        let _ = self.popups.track_popup(smithay::desktop::PopupKind::Xdg(surface));
    }

    /// Implicit grab + dismiss-on-outside-click. Previously believed
    /// blocked on `CompState`'s `SeatHandler` associated types not
    /// satisfying `PopupManager::grab_popup`'s `WaylandFocus +
    /// From<PopupKind>` bound - rechecked while implementing
    /// `move_request`/`resize_request` (same trait, adjacent methods) and
    /// it turns out they already do: `KeyboardFocus`/`PointerFocus` are
    /// both plain `WlSurface`, smithay provides `impl From<PopupKind> for
    /// WlSurface` itself, and `WlSurface: From<WlSurface>` trivially. No
    /// blocker ever existed by the time of this pass; the bound just
    /// hadn't been rechecked since being noted as unmet.
    ///
    /// `self.seat.clone()` rather than resolving `_seat` (the client's
    /// `wl_seat` resource) via `Seat::from_resource` - this compositor
    /// only ever has the one seat, matching how `move_request`/
    /// `resize_request` already ignore the same parameter.
    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let popup = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&popup) else { return };
        let seat = self.seat.clone();
        let Ok(grab) = self.popups.grab_popup(root, popup, &seat, serial) else { return };
        if let Some(keyboard) = seat.get_keyboard() {
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
    }
}

impl XdgDecorationHandler for CompState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
    }

    /// Honors whichever mode the client actually asked for, rather than
    /// always forcing server-side - and mirrors the result into our own
    /// `Window.decorated`, so a client drawing its own titlebar doesn't
    /// *also* get one drawn on top of it by us.
    ///
    /// Always forcing `ServerSide` (what this used to do) is why some
    /// clients ended up with two sets of window buttons: Firefox requests
    /// client-side decoration when its own "use system titlebar" setting
    /// is off, and draws its own close/minimize/maximize row regardless of
    /// what the compositor grants - so forcing server-side just added
    /// srdwm's row on top of the one Firefox was drawing anyway, instead
    /// of preventing it. Respecting the request means srdwm steps out of
    /// the way for exactly those clients, while everything that accepts
    /// (or has no preference and gets offered) server-side still gets our
    /// titlebar as before.
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        toplevel.send_configure();
        self.set_decorated_from_mode(toplevel.wl_surface(), mode == DecorationMode::ServerSide);
    }

    /// The client dropped its decoration-mode preference. `new_decoration`
    /// already offered `ServerSide` as the default the next configure will
    /// carry, so mirror that same default here rather than leaving
    /// whatever mode was negotiated before this - otherwise a client that
    /// requests `ClientSide`, then later unsets it expecting the default
    /// back, would stay undecorated by us forever.
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
        self.set_decorated_from_mode(toplevel.wl_surface(), true);
    }
}

impl ShmHandler for CompState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for CompState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl DmabufHandler for CompState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    /// Validates a client's dmabuf by actually importing it wherever a
    /// renderer is reachable from here, so a genuinely bad buffer (wrong
    /// modifier, format the renderer doesn't support) gets the protocol
    /// error instead of silently rendering garbage later.
    ///
    /// That's only the udev backend: its `PixmanRenderer` lives inside
    /// `self.udev` (`UdevState`), a field of this same struct.
    /// `PixmanRenderer` supports dmabuf import despite being a pure
    /// software renderer - `dmabuf_formats()` only advertises the Linear
    /// modifier, which it imports by mmap'ing the buffer and reading it
    /// directly as pixels, no GPU involved. This is what actually answers
    /// `docs/PANEL_SUPPORT_TODO.md`'s P0.3: GTK4 allocates via its own
    /// EGL/gbm path against the real DRM render node (untouched by this
    /// compositor either way) and hands the result here as a Linear-
    /// modifier dmabuf, which pixman can read straight off.
    ///
    /// The winit (nested/dev) backend's `GlesRenderer` lives on
    /// `WaylandPlatform`, a sibling of `CompState`, not reachable from a
    /// method on `CompState` itself. Accepted there without eager
    /// validation - the buffer still gets imported the same way every
    /// other buffer type already is, lazily, the first time it is actually
    /// rendered via `render_elements_from_surface_tree`. Real hardware,
    /// where P0.3 actually bites, always goes through the udev path.
    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        match self.udev.as_mut() {
            Some(udev) => match udev.renderer.import_dmabuf(&dmabuf, None) {
                Ok(_) => {
                    let _ = notifier.successful::<CompState>();
                }
                Err(e) => {
                    log::warn!("udev: rejecting dmabuf import: {e}");
                    notifier.failed();
                }
            },
            None => {
                let _ = notifier.successful::<CompState>();
            }
        }
    }
}

impl XdgActivationHandler for CompState {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// A launcher spawns an app after first getting a token
    /// (`get_activation_token`) and handing it to the new process (usually
    /// via `XDG_ACTIVATION_TOKEN`); the app's own first window then
    /// presents that same token back here via `activate`, asking to be
    /// raised. Without this, that request was silently ignored - the new
    /// window opened and just sat there unfocused behind everything,
    /// exactly the gap `docs/PANEL_SUPPORT_TODO.md`'s P1 flagged.
    ///
    /// No token bookkeeping of our own: `token_created`'s default already
    /// accepts every token (fine for a single-user session with no
    /// cross-client trust boundary to enforce), so all that's left is
    /// mapping the activating `surface` to a `WindowId` and reusing the
    /// exact same `focus_window` path a dock's "activate" request already
    /// goes through (`foreign_toplevel.rs`). If the surface isn't tracked
    /// yet - the activation raced ahead of this window's own mapping --
    /// there is nothing to focus yet, so this is a no-op rather than an
    /// error; the protocol doesn't require honoring every activation.
    fn request_activation(&mut self, _token: XdgActivationToken, _token_data: XdgActivationTokenData, surface: WlSurface) {
        if let Some(&id) = self.surface_to_id.get(&surface) {
            crate::input::focus_window(self, id);
        }
    }
}

/// `zwp_text_input_manager_v3` + `zwp_input_method_manager_v2`: lets a real
/// input method (fcitx5, ibus, any CJK/dead-key/emoji-picker IME) attach to
/// whichever surface has keyboard focus and draw its own candidate/
/// composition popup. Without these two globals a client that only speaks
/// text-input (most modern toolkits do, GTK4/Qt6 included) has no way to
/// tell the compositor "I have an editable text field, here is its cursor
/// rectangle" - every desktop app's search box, address bar, and chat
/// input silently loses IME support, not just an edge case.
///
/// Focus tracking needs *no* wiring here at all: `CompState::KeyboardFocus`
/// is a plain `WlSurface`, and smithay's own blanket `impl KeyboardTarget
/// for WlSurface` already calls `seat.text_input().set_focus/.enter()/
/// .leave()` and `seat.input_method().activate_input_method()/
/// deactivate_input_method()` from inside `enter`/`leave` - which
/// `set_keyboard_focus`'s existing `keyboard.set_focus(...)` call already
/// triggers on every real focus change. The only things actually missing
/// were the two manager globals and this handler for the popup surface
/// lifecycle.
impl smithay::wayland::input_method::InputMethodHandler for CompState {
    /// A candidate/composition window (an emoji picker, a CJK candidate
    /// list) just opened. Tracked as a regular [`PopupKind::InputMethod`]
    /// in the same [`PopupManager`](smithay::desktop::PopupManager) that
    /// already owns every `xdg_popup` - `elements::popup_render_elements`
    /// renders both kinds identically, so no separate render path is
    /// needed for this to actually become visible.
    fn new_popup(&mut self, surface: ImePopupSurface) {
        if let Err(e) = self.popups.track_popup(PopupKind::from(surface)) {
            log::warn!("input-method: failed to track popup: {e}");
        }
    }

    fn dismiss_popup(&mut self, surface: ImePopupSurface) {
        if let Some(parent) = surface.get_parent().map(|p| p.surface.clone()) {
            let _ = smithay::desktop::PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    /// The IME moved its own popup (e.g. following the text cursor as the
    /// user types) - `PopupSurface::location()` already reflects the new
    /// position; nothing else needs updating on this side, matching every
    /// other smithay-based compositor's own no-op here.
    fn popup_repositioned(&mut self, _surface: ImePopupSurface) {}

    /// Where the IME should anchor its popup, in the parent surface's own
    /// output-independent (logical, window-relative-origin) space - same
    /// geometry `elements::popup_targets` already computes for xdg popups,
    /// reused here rather than duplicated. A window not yet tracked (the
    /// activation raced ahead of its own mapping) gets a default/zero rect,
    /// same "no-op rather than an error" stance as `request_activation`
    /// above.
    fn parent_geometry(&self, parent: &WlSurface) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
        let Some(&id) = self.surface_to_id.get(parent) else {
            return smithay::utils::Rectangle::default();
        };
        let wm = self.wm.borrow();
        let Some(w) = wm.window(id) else {
            return smithay::utils::Rectangle::default();
        };
        let band = if w.decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
        smithay::utils::Rectangle::new((w.geometry.x, w.geometry.y + band).into(), (w.geometry.width as i32, w.geometry.height as i32).into())
    }
}

impl smithay::wayland::output::OutputHandler for CompState {}

/// `wp_cursor_shape_v1`: lets a client ask for a *named* cursor (text,
/// grab, resize edges, ...) instead of rendering and attaching its own
/// surface. Its requests route straight into `SeatHandler::cursor_image`
/// below, same as a client-drawn cursor surface does - no extra state on
/// our side. Without this global at all, a client that only speaks this
/// (increasingly the norm - recent GTK4/Firefox use it for most cursor
/// changes) has no way to tell us the pointer should look like anything
/// but whatever it last was, which reads as the cursor going stale, wrong,
/// or simply disappearing depending on what was showing when the client
/// gave up trying.
///
/// `TabletSeatHandler` is a supertrait bound of this protocol's `Dispatch`
/// impl (cursor-shape covers tablet tools too); srdwm has no tablet
/// support to speak of, so every method is left at its no-op default.
impl TabletSeatHandler for CompState {}

/// Fractional scaling. srdwm runs every output at scale 1, so there is
/// nothing to compute - but the global has to exist, because clients that
/// use it (notably wallpaper daemons) treat it as mandatory.
impl smithay::wayland::fractional_scale::FractionalScaleHandler for CompState {}

impl SeatHandler for CompState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    /// Clients set their own cursor (an I-beam over text, a hand over a
    /// link). Recorded here and drawn by the render paths - on a bare TTY
    /// nothing else would draw it. See `cursor.rs`.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }
}

impl WlrLayerShellHandler for CompState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, wl_output: Option<WlOutput>, _layer: Layer, namespace: String) {
        // Logged before anything else can early-return or panic: the
        // question this answers (see docs/PANEL_SUPPORT_TODO.md) is
        // whether this handler is reached AT ALL for a later
        // `get_layer_surface` request in a create -> commit -> destroy ->
        // commit-again -> create sequence, or whether the client's
        // dispatch is already dead by then and this never runs.
        log::debug!("layer-shell: new_layer_surface entered, surface={:?} namespace={namespace:?} output_named={}", surface.wl_surface().id(), wl_output.is_some());
        // A client may name the output it wants (a bar on a specific
        // monitor); if it doesn't, or names one we don't drive, it lands on
        // the primary output.
        let output = wl_output
            .as_ref()
            .and_then(|wl| self.output_for_wl(wl))
            .map(|e| e.output.clone())
            .or_else(|| self.primary_output().cloned());
        let Some(output) = output else {
            log::warn!("wayland: layer surface requested but no output exists yet");
            return;
        };
        // Paired with the debug log in `ensure_layer_initial_configure`'s
        // early return - see docs/PANEL_SUPPORT_TODO.md's P0. This is the
        // other half of "did map_layer actually succeed, and on which
        // output": logged unconditionally (not just on the error paths
        // that already existed) so a real reproduction shows both sides of
        // the handoff instead of just the failure.
        let surface_id = surface.wl_surface().id();
        let layer_surface = DesktopLayerSurface::new(surface, namespace);
        let result = layer_map_for_output(&output).map_layer(&layer_surface);
        match &result {
            Ok(()) => log::debug!("layer-shell: mapped surface {surface_id:?} onto output {}", output.name()),
            Err(e) => log::warn!("wayland: failed to map layer surface {surface_id:?}: {e}"),
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        // See the matching top-of-function log in `new_layer_surface`.
        log::debug!("layer-shell: layer_destroyed entered, surface={:?}", surface.wl_surface().id());
        // Marks this surface for the pre-commit-hook workaround in
        // `new_surface` - see that function's doc comment for the bug
        // this exists to route around.
        self.dead_layer_surfaces.insert(surface.wl_surface().clone());
        // The surface belongs to exactly one output's map, but which one is
        // the client's choice, so unmap from whichever holds it.
        for output in self.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(layer) = found {
                // Same zone-change recompute `ensure_layer_initial_configure`
                // already does on every commit that changes a layer's
                // exclusive zone (state.rs) - but this is the *only* place
                // that ever runs for a surface that goes away without one
                // last commit. `unmap_layer` alone doesn't trigger it:
                // reported live (by the AGS peer session) as a bar unmapping
                // for fullscreen yet `srd monitors` still reporting the
                // bar's old reserved_top for as long as fullscreen lasted --
                // harmless there only because fullscreen targets
                // `full_geometry`, which ignores the reservation anyway, but
                // wrong for anything that reads `usable`/`geometry` while a
                // bar is unmapped without exiting cleanly (a crash, not just
                // AGS's cooperative fullscreen hide).
                let zone_before = map.non_exclusive_zone();
                map.unmap_layer(&layer);
                let zone_after = map.non_exclusive_zone();
                if zone_after != zone_before {
                    self.pending.borrow_mut().push(srdwm_core::Event::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
                }
                break;
            }
        }
        // A lock/launcher surface holding exclusive keyboard focus just
        // vanished (crash, or a normal close) - don't leave focus dangling
        // on a dead surface.
        if self.seat.get_keyboard().and_then(|k| k.current_focus()).as_ref() == Some(surface.wl_surface()) {
            self.set_keyboard_focus(None);
        }
    }
}

/// Clipboard/primary-selection/drag-and-drop.
///
/// All three selection protocols below (`wl_data_device_manager`,
/// `zwp_primary_selection_v1`, `zwlr_data_control_manager_v1`) share
/// smithay's single `SelectionHandler`. Every transfer here is
/// *client-to-client*: one client owns the selection and writes the bytes
/// itself, and smithay wires the two ends together without the data passing
/// through us. `send_selection` is only ever called for a
/// **compositor-provided** selection (one this WM set itself via
/// `set_data_device_selection`), which srdwm never does - so it is
/// deliberately left unimplemented rather than faked.
impl SelectionHandler for CompState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for CompState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

// Drag-and-drop: the default trait methods already do the right thing for a
// compositor that doesn't draw its own drag icon or offer server-side drag
// sources - smithay runs the pointer grab and the offer/accept negotiation
// internally. Both are implemented empty (rather than skipped) because
// `DataDeviceHandler` requires them as supertraits.
impl ClientDndGrabHandler for CompState {}
impl ServerDndGrabHandler for CompState {}

impl PrimarySelectionHandler for CompState {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

/// `zwlr_data_control_manager_v1`: lets a client read/watch the selection
/// without ever holding keyboard focus. This is what `wl-paste --watch`
/// (and thus `cliphist store`, which the user's session autostarts) needs
/// - a focus-following clipboard manager is impossible without it.
impl DataControlHandler for CompState {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}

/// `ext_idle_notify_v1`. All the real logic (per-notification timers,
/// resetting them on activity, honouring inhibition) already lives in
/// smithay's own `IdleNotifierState` - this is just the getter it needs.
/// See `input.rs`'s `notify_idle_activity` for the other half: nothing
/// calls `notify_activity` on its own, that has to happen from every real
/// input path.
impl smithay::wayland::idle_notify::IdleNotifierHandler for CompState {
    fn idle_notifier_state(&mut self) -> &mut smithay::wayland::idle_notify::IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}

/// `zwp_idle_inhibit_manager_v1`. A video player (or anything else that
/// wants the screen to stay on/unlocked while it runs) creates one of
/// these tied to its own surface; as long as at least one is alive,
/// `IdleNotifierState::set_is_inhibited` stops idle timers from firing at
/// all - see `idle_inhibiting_surfaces`'s doc comment on `CompState` for
/// the one simplification (not workspace-visibility-aware) this takes.
impl smithay::wayland::idle_inhibit::IdleInhibitHandler for CompState {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibiting_surfaces.push(surface);
        self.idle_notifier_state.set_is_inhibited(true);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibiting_surfaces.retain(|s| s != &surface);
        self.idle_notifier_state.set_is_inhibited(!self.idle_inhibiting_surfaces.is_empty());
    }
}

delegate_compositor!(CompState);
delegate_xdg_shell!(CompState);
delegate_xdg_decoration!(CompState);
delegate_shm!(CompState);
delegate_dmabuf!(CompState);
delegate_xdg_activation!(CompState);
delegate_text_input_manager!(CompState);
delegate_input_method_manager!(CompState);
delegate_seat!(CompState);
delegate_output!(CompState);
delegate_layer_shell!(CompState);
delegate_data_device!(CompState);
delegate_primary_selection!(CompState);
delegate_data_control!(CompState);
delegate_session_lock!(CompState);
delegate_cursor_shape!(CompState);
smithay::delegate_viewporter!(CompState);
smithay::delegate_fractional_scale!(CompState);
smithay::delegate_idle_notify!(CompState);
smithay::delegate_idle_inhibit!(CompState);
