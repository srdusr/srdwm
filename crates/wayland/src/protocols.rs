//! smithay protocol-handler implementations for [`CompState`], plus the
//! `delegate_*!` macros that route each protocol's dispatch to them.
//!
//! Deliberately thin: these methods translate a protocol event into a call
//! on [`crate::state`] (window bookkeeping) or [`crate::input`] (focus), and
//! hold no logic of their own beyond what the protocol itself dictates. The
//! session-lock handler is the one exception, living in [`crate::lock`]
//! alongside the rest of that feature.

use smithay::desktop::{layer_map_for_output, LayerSurface as DesktopLayerSurface};
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Client;
use smithay::utils::Serial;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::primary_selection::{PrimarySelectionHandler, PrimarySelectionState};
use smithay::wayland::selection::wlr_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_data_control, delegate_data_device, delegate_layer_shell, delegate_output,
    delegate_primary_selection, delegate_seat, delegate_session_lock, delegate_shm, delegate_xdg_decoration,
    delegate_xdg_shell,
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
        }
        self.ensure_layer_initial_configure(surface);
    }
}

impl XdgShellHandler for CompState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.new_managed_window(surface);
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}

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

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {}
}

impl ShmHandler for CompState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for CompState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl smithay::wayland::output::OutputHandler for CompState {}

impl SeatHandler for CompState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}

impl WlrLayerShellHandler for CompState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, wl_output: Option<WlOutput>, _layer: Layer, namespace: String) {
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
        let layer_surface = DesktopLayerSurface::new(surface, namespace);
        let result = layer_map_for_output(&output).map_layer(&layer_surface);
        if let Err(e) = result {
            log::warn!("wayland: failed to map layer surface: {e}");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        // The surface belongs to exactly one output's map, but which one is
        // the client's choice, so unmap from whichever holds it.
        for output in self.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(layer) = found {
                map.unmap_layer(&layer);
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

delegate_compositor!(CompState);
delegate_xdg_shell!(CompState);
delegate_xdg_decoration!(CompState);
delegate_shm!(CompState);
delegate_seat!(CompState);
delegate_output!(CompState);
delegate_layer_shell!(CompState);
delegate_data_device!(CompState);
delegate_primary_selection!(CompState);
delegate_data_control!(CompState);
delegate_session_lock!(CompState);
