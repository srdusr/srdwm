//! `zwlr_output_management_v1`: lets a settings panel enumerate every
//! output srdwm drives - name, description, physical size, supported
//! modes, and current position/scale/transform/enabled state - and
//! request changes. smithay 0.7 has no built-in helper for this protocol;
//! hand-written against `wayland-protocols-wlr`'s raw server bindings,
//! same pattern as `foreign_toplevel.rs`/`workspace.rs`. Requested by the
//! field-survey comparison against sway/Hyprland/river (`docs/
//! IMPLEMENTATION_STATUS.md`): every one of them treats this as core, not
//! optional, once external monitors, scaling, or a settings panel are in
//! the picture.
//!
//! **Deliberately conservative on `apply`/`test`**: a configuration can
//! change a head's position, scale and transform - all straightforward
//! via `Output::change_current_state`, which already exists and is
//! already exercised (hotplug, the nested backend's own resize handling).
//! Two things a real configuration payload can also ask for are
//! deliberately *not* attempted, and fail honestly (`failed`, not a
//! silent no-op) rather than accept a request this compositor can't
//! actually carry out:
//!
//! - **Disabling/enabling a head.** srdwm has no concept of a head that
//!   exists but isn't mapped into the global compositor space - every
//!   output it advertises is always enabled. A request to actually
//!   change that state (not just re-`enable_head` an already-enabled one,
//!   which every real settings panel does on every apply regardless of
//!   what actually changed) fails.
//! - **Switching resolution or refresh rate.** Real DRM mode-setting means
//!   finding the matching connector mode and reprogramming the CRTC --
//!   substantial, hardware-dependent work this pass didn't attempt, and
//!   this development environment has no multi-mode hardware to verify it
//!   against even if it had. A `set_mode`/`set_custom_mode` that matches
//!   the head's *already-current* mode (the common case: a panel echoes
//!   back existing state alongside a real position/scale change) is
//!   accepted as a no-op; a genuinely different one fails.
//!
//! Both are real, scoped-out gaps, not oversights - worth closing in a
//! later pass once there's hardware to verify a mode-setting change
//! against.


use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use smithay::output::{Mode as OutputMode, Output, Scale};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum};
use smithay::utils::{Point, Transform};

use wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1};
use wayland_protocols_wlr::output_management::v1::server::zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1};
use wayland_protocols_wlr::output_management::v1::server::zwlr_output_head_v1::{self, ZwlrOutputHeadV1};
use wayland_protocols_wlr::output_management::v1::server::zwlr_output_manager_v1::{self, ZwlrOutputManagerV1};
use wayland_protocols_wlr::output_management::v1::server::zwlr_output_mode_v1::{self, ZwlrOutputModeV1};

use crate::state::CompState;

const PROTOCOL_VERSION: u32 = 4;

pub struct OutputManagementState {
    _global: GlobalId,
}

impl OutputManagementState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrOutputManagerV1, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, ZwlrOutputManagerV1, _>(PROTOCOL_VERSION, ()) }
    }
}

pub struct HeadData {
    name: String,
}

#[derive(Clone, Copy, PartialEq)]
pub struct ModeData {
    mode: OutputMode,
}

/// What a client has asked for one head within a single in-flight
/// `zwlr_output_configuration_v1` - built up by `enable_head`/
/// `disable_head` and the requests on the `zwlr_output_configuration_head_v1`
/// object that creates, then read back all at once by `apply`/`test`.
enum HeadRequest {
    Disabled,
    Enabled { position: Option<(i32, i32)>, scale: Option<f64>, transform: Option<Transform>, mode: Option<ModeRequest> },
}

enum ModeRequest {
    Existing(OutputMode),
    Custom { width: i32, height: i32, refresh: i32 },
}

/// Shared between a `ZwlrOutputConfigurationV1` and every
/// `ZwlrOutputConfigurationHeadV1` it creates, keyed by output name (the
/// one stable identity a head has - see `HeadData`'s doc comment on why
/// `Output` itself isn't used as the key).
type ConfigMap = Arc<Mutex<HashMap<String, HeadRequest>>>;

pub struct ConfigurationData {
    serial: u32,
    heads: ConfigMap,
}

pub struct ConfigurationHeadData {
    head_name: String,
    heads: ConfigMap,
}

impl GlobalDispatch<ZwlrOutputManagerV1, ()> for CompState {
    fn bind(state: &mut Self, dh: &DisplayHandle, client: &Client, manager: New<ZwlrOutputManagerV1>, _data: &(), data_init: &mut DataInit<'_, Self>) {
        let manager = data_init.init(manager, ());
        let outputs: Vec<Output> = state.outputs().cloned().collect();
        for output in &outputs {
            announce_head(state, &manager, output, client, dh);
        }
        manager.done(state.output_serial);
        state.output_managers.push(manager);
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for CompState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        manager: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                data_init.init(id, ConfigurationData { serial, heads: Arc::new(Mutex::new(HashMap::new())) });
            }
            zwlr_output_manager_v1::Request::Stop => {
                manager.finished();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, manager: &ZwlrOutputManagerV1, _data: &()) {
        state.output_managers.retain(|m| m != manager);
    }
}

impl Dispatch<ZwlrOutputHeadV1, HeadData> for CompState {
    fn request(_state: &mut Self, _client: &Client, _head: &ZwlrOutputHeadV1, _request: zwlr_output_head_v1::Request, _data: &HeadData, _dh: &DisplayHandle, _data_init: &mut DataInit<'_, Self>) {
        // Only request is `release` (a destructor); `destroyed` below
        // handles the cleanup for both that and a client disconnecting
        // outright.
    }

    fn destroyed(state: &mut Self, _client: ClientId, head: &ZwlrOutputHeadV1, data: &HeadData) {
        if let Some(handles) = state.output_heads.get_mut(&data.name) {
            handles.retain(|h| h != head);
        }
    }
}

impl Dispatch<ZwlrOutputModeV1, ModeData> for CompState {
    fn request(_state: &mut Self, _client: &Client, _mode: &ZwlrOutputModeV1, _request: zwlr_output_mode_v1::Request, _data: &ModeData, _dh: &DisplayHandle, _data_init: &mut DataInit<'_, Self>) {}

    fn destroyed(state: &mut Self, _client: ClientId, mode: &ZwlrOutputModeV1, _data: &ModeData) {
        for handles in state.output_modes.values_mut() {
            handles.retain(|m| m != mode);
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ConfigurationData> for CompState {
    fn request(
        state: &mut Self,
        client: &Client,
        config: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        data: &ConfigurationData,
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_configuration_v1::Request::EnableHead { id, head } => {
                let Some(name) = head.data::<HeadData>().map(|d| d.name.clone()) else { return };
                data.heads.lock().unwrap().insert(name.clone(), HeadRequest::Enabled { position: None, scale: None, transform: None, mode: None });
                data_init.init(id, ConfigurationHeadData { head_name: name, heads: data.heads.clone() });
            }
            zwlr_output_configuration_v1::Request::DisableHead { head } => {
                let Some(name) = head.data::<HeadData>().map(|d| d.name.clone()) else { return };
                data.heads.lock().unwrap().insert(name, HeadRequest::Disabled);
            }
            zwlr_output_configuration_v1::Request::Apply => apply_or_test(state, config, data, client, dh, true),
            zwlr_output_configuration_v1::Request::Test => apply_or_test(state, config, data, client, dh, false),
            zwlr_output_configuration_v1::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(_state: &mut Self, _client: ClientId, _config: &ZwlrOutputConfigurationV1, _data: &ConfigurationData) {}
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ConfigurationHeadData> for CompState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _config_head: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        data: &ConfigurationHeadData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let mut heads = data.heads.lock().unwrap();
        let Some(HeadRequest::Enabled { position, scale, transform, mode }) = heads.get_mut(&data.head_name) else { return };
        match request {
            zwlr_output_configuration_head_v1::Request::SetPosition { x, y } => *position = Some((x, y)),
            zwlr_output_configuration_head_v1::Request::SetScale { scale: s } => *scale = Some(s),
            zwlr_output_configuration_head_v1::Request::SetTransform { transform: WEnum::Value(t) } => {
                *transform = Some(wl_transform_to_smithay(t));
            }
            zwlr_output_configuration_head_v1::Request::SetMode { mode: mode_obj } => {
                if let Some(m) = mode_obj.data::<ModeData>() {
                    *mode = Some(ModeRequest::Existing(m.mode));
                }
            }
            zwlr_output_configuration_head_v1::Request::SetCustomMode { width, height, refresh } => {
                *mode = Some(ModeRequest::Custom { width, height, refresh });
            }
            _ => {}
        }
    }

    fn destroyed(_state: &mut Self, _client: ClientId, _config_head: &ZwlrOutputConfigurationHeadV1, _data: &ConfigurationHeadData) {}
}

/// `WEnum<wl_output::Transform>` has no built-in conversion to smithay's
/// own `Transform` (the reverse direction, smithay `Transform` -> wire, is
/// provided by smithay itself and already used elsewhere in this crate --
/// this is the one direction it doesn't cover, needed here because this is
/// the first place in this codebase that has to *decode* a client-sent
/// transform rather than only ever sending one).
fn wl_transform_to_smithay(t: smithay::reexports::wayland_server::protocol::wl_output::Transform) -> Transform {
    use smithay::reexports::wayland_server::protocol::wl_output::Transform as Wl;
    match t {
        Wl::Normal => Transform::Normal,
        Wl::_90 => Transform::_90,
        Wl::_180 => Transform::_180,
        Wl::_270 => Transform::_270,
        Wl::Flipped => Transform::Flipped,
        Wl::Flipped90 => Transform::Flipped90,
        Wl::Flipped180 => Transform::Flipped180,
        Wl::Flipped270 => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

/// Whether a client's requested mode is a no-op relative to what's already
/// active - the one `set_mode`/`set_custom_mode` case this module accepts
/// (see its own doc comment on why a genuinely *different* mode doesn't).
/// A custom mode's `refresh` of `0` means "unspecified", so it matches any
/// refresh rate the current mode happens to have.
fn mode_request_matches_current(req: &ModeRequest, current: Option<OutputMode>) -> bool {
    match req {
        ModeRequest::Existing(m) => current == Some(*m),
        ModeRequest::Custom { width, height, refresh } => current.is_some_and(|m| m.size.w == *width && m.size.h == *height && (*refresh == 0 || m.refresh == *refresh)),
    }
}

/// Validates every head in `data.heads` against live state, then - only
/// for `apply`, never `test` - actually changes it. All-or-nothing: if
/// any head's request can't be honoured, nothing is applied and `failed`
/// is sent, matching the protocol's own framing of a configuration as one
/// atomic unit. See the module doc comment for exactly what "can't be
/// honoured" covers.
fn apply_or_test(state: &mut CompState, config: &ZwlrOutputConfigurationV1, data: &ConfigurationData, _client: &Client, _dh: &DisplayHandle, apply: bool) {
    if data.serial != state.output_serial {
        config.cancelled();
        return;
    }
    let heads = data.heads.lock().unwrap();
    for (name, req) in heads.iter() {
        let Some(output) = state.outputs().find(|o| &o.name() == name).cloned() else {
            config.failed();
            return;
        };
        match req {
            HeadRequest::Disabled => {
                config.failed();
                return;
            }
            HeadRequest::Enabled { mode, .. } => {
                let Some(mode_req) = mode else { continue };
                if !mode_request_matches_current(mode_req, output.current_mode()) {
                    config.failed();
                    return;
                }
            }
        }
    }

    if apply {
        for (name, req) in heads.iter() {
            let HeadRequest::Enabled { position, scale, transform, .. } = req else { continue };
            let Some(output) = state.outputs().find(|o| &o.name() == name).cloned() else { continue };
            if let Some(t) = transform {
                output.change_current_state(None, Some(*t), None, None);
            }
            if let Some(s) = scale {
                output.change_current_state(None, None, Some(Scale::Fractional(*s)), None);
            }
            if let Some((x, y)) = position {
                // A real `wlr-output-management-v1` client's own position
                // request is logical, by protocol convention - `apply_
                // output_position` wants physical (see its own doc
                // comment), so it gets converted here rather than assumed.
                let output_scale = output.current_scale().fractional_scale();
                let physical: Point<i32, smithay::utils::Logical> =
                    (((*x as f64) * output_scale).round() as i32, ((*y as f64) * output_scale).round() as i32).into();
                apply_output_position(state, &output, physical);
            }
        }
    }
    drop(heads);

    config.succeeded();
    if apply {
        broadcast_dirty_outputs(state);
    }
}

/// Moves an output and keeps every place its position is separately
/// cached in step - `Output` itself, `CompState::outputs` (used for hit-
/// testing/`output_at`) and, on the udev backend, `UdevHead::location`
/// (used to translate render geometry into head-local space) each keep
/// their own copy for reasons documented on their own fields, and would
/// otherwise silently drift from what `Output` now reports.
///
/// `new_location` is *physical* pixels - the same space `Platform::
/// monitors()` reports `full_x`/`full_y` in (see that function's own doc
/// comment for why), which is what `entry.location`/`head.location` are
/// everywhere else in this compositor (render geometry, damage tracking,
/// cursor clamping). `output.change_current_state`'s own position
/// parameter is a real Wayland-protocol value, and `wl_output`/
/// `xdg_output` report position to clients in *logical* points - always,
/// not a choice this compositor makes - so it gets a separately scaled
/// copy here rather than the raw physical value. Storing the unconverted
/// physical value in `change_current_state` too (what this used to do)
/// looked harmless locally but told every real Wayland client the wrong
/// logical position for any output whose scale isn't exactly `1.0`,
/// reported from the AGS peer session as a dead gap between two monitors'
/// desktop space wide enough to drop a window or a pointer into: their
/// own arrangement math chains outputs by the physical width `srd
/// monitors` reports, correctly, but a `1.25`-scale output being told a
/// `1920`-logical-point position when its real logical width was `1536`
/// opened exactly that gap.
///
/// Callers already holding a *logical* position (a real `wlr-output-
/// management-v1` client's own request, which is logical by protocol
/// convention) must convert to physical before calling this - see this
/// function's own call site in `handle_apply_or_test` for that
/// conversion.
pub(crate) fn apply_output_position(state: &mut CompState, output: &Output, new_location_physical: Point<i32, smithay::utils::Logical>) {
    let scale = output.current_scale().fractional_scale();
    let logical: Point<i32, smithay::utils::Logical> =
        ((new_location_physical.x as f64 / scale).round() as i32, (new_location_physical.y as f64 / scale).round() as i32).into();
    output.change_current_state(None, None, None, Some(logical));
    if let Some(entry) = state.outputs.iter_mut().find(|e| &e.output == output) {
        entry.location = new_location_physical;
    }
    if let Some(udev) = state.udev.as_mut() {
        if let Some(head) = udev.heads.iter_mut().find(|h| &h.output == output) {
            head.location = new_location_physical;
        }
    }
    // Remembered for next startup - see `monitor_layout`'s own module doc
    // comment for why this compositor persists its own layout rather than
    // leaving that to whichever panel happens to be running.
    crate::monitor_layout::save_output(&output.name(), crate::monitor_layout::PersistedOutput { x: new_location_physical.x, y: new_location_physical.y, enabled: true });
}

fn announce_head(state: &mut CompState, manager: &ZwlrOutputManagerV1, output: &Output, client: &Client, dh: &DisplayHandle) {
    let Ok(head) = client.create_resource::<ZwlrOutputHeadV1, HeadData, CompState>(dh, manager.version(), HeadData { name: output.name() }) else {
        return;
    };
    manager.head(&head);

    head.name(output.name());
    head.description(output.description());
    let phys = output.physical_properties();
    if phys.size.w > 0 && phys.size.h > 0 {
        head.physical_size(phys.size.w, phys.size.h);
    }
    if !phys.make.is_empty() {
        head.make(phys.make.clone());
    }
    if !phys.model.is_empty() {
        head.model(phys.model.clone());
    }

    let modes = output.modes();
    let preferred = output.preferred_mode();
    let current = output.current_mode();
    let mut current_mode_handle = None;
    for m in &modes {
        let Ok(mode_handle) = client.create_resource::<ZwlrOutputModeV1, ModeData, CompState>(dh, manager.version(), ModeData { mode: *m }) else {
            continue;
        };
        head.mode(&mode_handle);
        mode_handle.size(m.size.w, m.size.h);
        if m.refresh > 0 {
            mode_handle.refresh(m.refresh);
        }
        if Some(*m) == preferred {
            mode_handle.preferred();
        }
        if Some(*m) == current {
            current_mode_handle = Some(mode_handle.clone());
        }
        state.output_modes.entry(output.name()).or_default().push(mode_handle);
    }

    // srdwm has no disabled-output concept - see the module doc comment --
    // so every head it ever advertises is always enabled.
    head.enabled(1);
    if let Some(mode_handle) = &current_mode_handle {
        head.current_mode(mode_handle);
    }
    let loc = output.current_location();
    head.position(loc.x, loc.y);
    head.transform(output.current_transform().into());
    head.scale(output.current_scale().fractional_scale());

    state.output_heads.entry(output.name()).or_default().push(head);
}

/// One output's state as far as this protocol cares, for the cheap
/// per-frame equality check `broadcast_dirty_outputs` gates on - see
/// `foreign_toplevel::broadcast_dirty_state`/`workspace::
/// broadcast_dirty_active`'s doc comments for the same "diff once a tick,
/// only do real work on an actual change" shape used throughout this
/// crate's dock/panel-facing protocols. Scale is compared as milli-units
/// (`* 1000.0` rounded) rather than the raw `f64`, so this can derive a
/// plain `PartialEq` instead of needing a fuzzy float comparison.
#[derive(Clone, PartialEq)]
pub(crate) struct OutputSnapshot {
    name: String,
    x: i32,
    y: i32,
    scale_millis: i64,
    transform: Transform,
    mode: Option<(i32, i32, i32)>,
}

fn snapshot(output: &Output) -> OutputSnapshot {
    let loc = output.current_location();
    let mode = output.current_mode().map(|m| (m.size.w, m.size.h, m.refresh));
    OutputSnapshot {
        name: output.name(),
        x: loc.x,
        y: loc.y,
        scale_millis: (output.current_scale().fractional_scale() * 1000.0).round() as i64,
        transform: output.current_transform(),
        mode,
    }
}

/// Called once a frame from `CompState::tick_dirty_broadcasts`. Diffs the
/// live output set/state against what was last broadcast and, only on a
/// real change, creates/destroys head objects for any output that
/// appeared/disappeared and re-sends current state (enabled/current_mode/
/// position/transform/scale) on every other - covers both real hotplug
/// and an `apply()` that just went through, without either needing its own
/// separate notification path.
pub(crate) fn broadcast_dirty_outputs(state: &mut CompState) {
    let current: Vec<OutputSnapshot> = state.outputs().map(snapshot).collect();
    if current == state.last_broadcast_outputs {
        return;
    }
    let current_names: HashSet<String> = current.iter().map(|s| s.name.clone()).collect();
    let previous_names: HashSet<String> = state.last_broadcast_outputs.iter().map(|s| s.name.clone()).collect();
    state.last_broadcast_outputs = current;

    for name in previous_names.difference(&current_names) {
        if let Some(handles) = state.output_heads.remove(name) {
            for h in handles {
                h.finished();
            }
        }
        state.output_modes.remove(name);
    }

    let added: Vec<String> = current_names.difference(&previous_names).cloned().collect();
    if !added.is_empty() {
        let managers = state.output_managers.clone();
        for manager in &managers {
            let Some(client) = manager.client() else { continue };
            let dh = state.dh.clone();
            for name in &added {
                let found: Option<Output> = state.outputs().find(|o| &o.name() == name).cloned();
                if let Some(output) = found {
                    announce_head(state, manager, &output, &client, &dh);
                }
            }
        }
    }

    let outputs: Vec<Output> = state.outputs().cloned().collect();
    for output in &outputs {
        let Some(handles) = state.output_heads.get(&output.name()).cloned() else { continue };
        let mode_handles = state.output_modes.get(&output.name()).cloned().unwrap_or_default();
        let current_mode = output.current_mode();
        let matching_mode_handle = current_mode.and_then(|m| mode_handles.iter().find(|mh| mh.data::<ModeData>().is_some_and(|d| d.mode == m)).cloned());
        let loc = output.current_location();
        for head in &handles {
            head.enabled(1);
            if let Some(mh) = &matching_mode_handle {
                head.current_mode(mh);
            }
            head.position(loc.x, loc.y);
            head.transform(output.current_transform().into());
            head.scale(output.current_scale().fractional_scale());
        }
    }

    state.output_serial += 1;
    let serial = state.output_serial;
    for manager in state.output_managers.clone() {
        manager.done(serial);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: i32, h: i32, refresh: i32) -> OutputMode {
        OutputMode { size: (w, h).into(), refresh }
    }

    #[test]
    fn existing_mode_request_matches_only_the_identical_mode() {
        let m = mode(1920, 1080, 60000);
        assert!(mode_request_matches_current(&ModeRequest::Existing(m), Some(m)));
        assert!(!mode_request_matches_current(&ModeRequest::Existing(m), Some(mode(1280, 720, 60000))));
        assert!(!mode_request_matches_current(&ModeRequest::Existing(m), None));
    }

    #[test]
    fn custom_mode_request_matches_same_size_and_refresh() {
        let req = ModeRequest::Custom { width: 1920, height: 1080, refresh: 60000 };
        assert!(mode_request_matches_current(&req, Some(mode(1920, 1080, 60000))));
        assert!(!mode_request_matches_current(&req, Some(mode(1920, 1080, 59940))));
        assert!(!mode_request_matches_current(&req, Some(mode(1280, 720, 60000))));
    }

    #[test]
    fn custom_mode_request_with_zero_refresh_matches_any_refresh_at_that_size() {
        let req = ModeRequest::Custom { width: 1920, height: 1080, refresh: 0 };
        assert!(mode_request_matches_current(&req, Some(mode(1920, 1080, 60000))));
        assert!(mode_request_matches_current(&req, Some(mode(1920, 1080, 59940))));
        assert!(!mode_request_matches_current(&req, Some(mode(1280, 720, 60000))));
    }

    #[test]
    fn snapshot_equality_ignores_float_noise_below_a_thousandth() {
        let a = OutputSnapshot { name: "HDMI-A-1".into(), x: 0, y: 0, scale_millis: 1500, transform: Transform::Normal, mode: Some((1920, 1080, 60000)) };
        let b = OutputSnapshot { name: "HDMI-A-1".into(), x: 0, y: 0, scale_millis: 1500, transform: Transform::Normal, mode: Some((1920, 1080, 60000)) };
        assert!(a == b);
    }

    #[test]
    fn snapshot_inequality_catches_a_position_change() {
        let a = OutputSnapshot { name: "HDMI-A-1".into(), x: 0, y: 0, scale_millis: 1000, transform: Transform::Normal, mode: None };
        let b = OutputSnapshot { name: "HDMI-A-1".into(), x: 1920, y: 0, scale_millis: 1000, transform: Transform::Normal, mode: None };
        assert!(a != b);
    }

    #[test]
    fn wl_transform_round_trips_through_smithay_and_back() {
        use smithay::reexports::wayland_server::protocol::wl_output::Transform as Wl;
        for (wl, expected) in [
            (Wl::Normal, Transform::Normal),
            (Wl::_90, Transform::_90),
            (Wl::_180, Transform::_180),
            (Wl::_270, Transform::_270),
            (Wl::Flipped, Transform::Flipped),
            (Wl::Flipped90, Transform::Flipped90),
            (Wl::Flipped180, Transform::Flipped180),
            (Wl::Flipped270, Transform::Flipped270),
        ] {
            assert_eq!(wl_transform_to_smithay(wl), expected);
            let back: Wl = expected.into();
            assert_eq!(back, wl);
        }
    }
}
