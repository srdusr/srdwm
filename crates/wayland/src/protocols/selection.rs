//! Clipboard/primary-selection/drag-and-drop.
//!
//! All three selection protocols below (`wl_data_device_manager`,
//! `zwp_primary_selection_v1`, `zwlr_data_control_manager_v1`) share
//! smithay's single `SelectionHandler`. Every transfer here is
//! *client-to-client*: one client owns the selection and writes the bytes
//! itself, and smithay wires the two ends together without the data passing
//! through us. `send_selection` is only ever called for a
//! **compositor-provided** selection (one this WM set itself via
//! `set_data_device_selection`), which srdwm never does - so it is
//! deliberately left unimplemented rather than faked.

use smithay::wayland::selection::data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler};
use smithay::wayland::selection::primary_selection::{PrimarySelectionHandler, PrimarySelectionState};
use smithay::wayland::selection::wlr_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::selection::SelectionHandler;

use crate::state::CompState;

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
