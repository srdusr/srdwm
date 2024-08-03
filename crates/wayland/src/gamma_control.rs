//! `zwlr_gamma_control_manager_v1`: lets a client (a night-light daemon like
//! `gammastep`/`wlsunset`, or a settings panel) set a per-output gamma
//! ramp - the mechanism behind "reduce blue light in the evening" features.
//!
//! DRM/udev backend only: there is no real CRTC gamma table to adjust when
//! nested under a host compositor, same reasoning as `output_power.rs`. The
//! global is genuinely not created there (`CompState::_gamma_control_state`
//! is `Option`, `None` for `winit`) rather than advertised-and-always-
//! failing.
//!
//! No smithay helper exists for this protocol, so the `GlobalDispatch`/
//! `Dispatch` plumbing below is hand-written against the raw
//! `wayland-protocols-wlr` server bindings, the same pattern as
//! `screencopy.rs`/`output_power.rs`.

use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};
use wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_manager_v1::{self, ZwlrGammaControlManagerV1};
use wayland_protocols_wlr::gamma_control::v1::server::zwlr_gamma_control_v1::{self, ZwlrGammaControlV1};

use crate::state::CompState;

/// The manager global. Held by `CompState` purely to keep the global alive
/// for the compositor's lifetime.
pub struct GammaControlManagerState {
    _global: GlobalId,
}

impl GammaControlManagerState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrGammaControlManagerV1, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, ZwlrGammaControlManagerV1, _>(1, ()) }
    }
}

/// Which `wl_output` a `zwlr_gamma_control_v1` object controls, resolved
/// once at creation - same reasoning as `output_power::OutputPowerData`.
pub struct GammaControlData {
    output: WlOutput,
}

impl GlobalDispatch<ZwlrGammaControlManagerV1, ()> for CompState {
    fn bind(_state: &mut Self, _dh: &DisplayHandle, _client: &Client, manager: New<ZwlrGammaControlManagerV1>, _data: &(), data_init: &mut DataInit<'_, Self>) {
        data_init.init(manager, ());
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrGammaControlManagerV1,
        request: zwlr_gamma_control_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_gamma_control_manager_v1::Request;
        let Request::GetGammaControl { id, output } = request else { return };
        let control = data_init.init(id, GammaControlData { output: output.clone() });
        // Required "sent immediately when the gamma control object is
        // created" - `failed()` covers an output that doesn't resolve or
        // has no gamma ramp at all (headless/virtual outputs), rather than
        // sending a nonsensical size.
        match state.gamma_ramp_size(&output) {
            Some(size) => control.gamma_size(size),
            None => control.failed(),
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, GammaControlData> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrGammaControlV1,
        request: zwlr_gamma_control_v1::Request,
        data: &GammaControlData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let zwlr_gamma_control_v1::Request::SetGamma { fd } = request else { return };
        if state.set_gamma_ramp(&data.output, fd).is_none() {
            resource.failed();
        }
    }
}
