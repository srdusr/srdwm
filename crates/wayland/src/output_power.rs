//! `zwlr_output_power_management_v1`: lets a client (a settings panel, an
//! idle daemon) put a specific output into DPMS off/on.
//!
//! Deliberately separate from `ext_idle_notify_v1`/`zwp_idle_inhibit_manager_v1`
//! (`protocols.rs`): that protocol only *tells* a client the seat has gone
//! idle, it has no way to blank a screen itself. A real "turn the display
//! off after N minutes idle" feature needs both - an idle daemon watches
//! `ext_idle_notify_v1` and calls this protocol's `set_mode` in response.
//! Neither protocol implies the other.
//!
//! DRM/udev backend only: there is no real display to power down when
//! nested under a host compositor (`winit.rs`) - the host owns the actual
//! screen, and turning off the *nested window* makes no sense. The global
//! is simply never created there (`CompState::_output_power_state` is
//! `None`), so a client sees the protocol as genuinely unsupported rather
//! than advertised-but-always-failing.
//!
//! No smithay helper exists for this protocol, so the `GlobalDispatch`/
//! `Dispatch` plumbing below is hand-written against the raw
//! `wayland-protocols-wlr` server bindings, the same pattern as
//! `screencopy.rs`/`foreign_toplevel.rs`.

use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};
use wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1};
use wayland_protocols_wlr::output_power_management::v1::server::zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1};

use crate::state::CompState;

/// The manager global. Held by `CompState` purely to keep the global alive
/// for the compositor's lifetime.
pub struct OutputPowerManagerState {
    _global: GlobalId,
}

impl OutputPowerManagerState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrOutputPowerManagerV1, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, ZwlrOutputPowerManagerV1, _>(1, ()) }
    }
}

/// Which `wl_output` a `zwlr_output_power_v1` object controls, resolved
/// once at creation. `set_mode` re-resolves it against the live output list
/// rather than trusting this stays valid - a client is free to hold the
/// object across a monitor unplug.
pub struct OutputPowerData {
    output: WlOutput,
}

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for CompState {
    fn bind(_state: &mut Self, _dh: &DisplayHandle, _client: &Client, manager: New<ZwlrOutputPowerManagerV1>, _data: &(), data_init: &mut DataInit<'_, Self>) {
        data_init.init(manager, ());
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for CompState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_power_manager_v1::Request;
        if let Request::GetOutputPower { id, output } = request {
            let power = data_init.init(id, OutputPowerData { output });
            // Per protocol: "sent immediately when the object is created
            // so the client is informed about the current power management
            // mode" - this compositor has no notion of a monitor starting
            // powered off, so always On at creation.
            power.mode(Mode::On);
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, OutputPowerData> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        data: &OutputPowerData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let zwlr_output_power_v1::Request::SetMode { mode } = request else { return };
        let Ok(mode) = mode.into_result() else { return };
        match state.set_output_power(&data.output, mode == Mode::On) {
            Some(()) => resource.mode(mode),
            None => resource.failed(),
        }
    }
}
