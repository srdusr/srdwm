//! `xdg_shell`: toplevel and popup lifecycle, and the client-initiated
//! move/resize/maximize/fullscreen/minimize requests a CSD client sends
//! instead of (or alongside) the pointer-driven titlebar handlers in
//! `input.rs`.

use smithay::desktop::{find_popup_root_surface, PopupKeyboardGrab, PopupKind, PopupPointerGrab};
use smithay::input::pointer::Focus;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState};

use crate::state::CompState;

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
        // Temporary: added to trace a live report that dragging a CSD
        // window (Firefox) by its own tab strip/header bar does nothing --
        // this is the only way to tell "the client never sent xdg_toplevel
        // ::move at all" apart from "it sent it and something downstream
        // of here didn't follow through." Remove once that's settled.
        match self.surface_to_id.get(surface.wl_surface()) {
            Some(&id) => {
                let pos = crate::input::last_pointer_pos(self);
                log::info!("move_request: window {id:?} at pointer {pos:?}");
                self.wm.borrow_mut().start_drag(id, pos.x as i32, pos.y as i32);
            }
            None => log::warn!("move_request: surface has no tracked window id"),
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
