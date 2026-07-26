//! smithay protocol-handler implementations for [`CompState`], plus the
//! `delegate_*!` macros that route each protocol's dispatch to them.
//!
//! Deliberately thin: these methods translate a protocol event into a call
//! on [`crate::state`] (window bookkeeping) or [`crate::input`] (focus), and
//! hold no logic of their own beyond what the protocol itself dictates. The
//! session-lock handler is the one exception, living in [`crate::lock`]
//! alongside the rest of that feature.
//!
//! Split one file per protocol handler, matching niri's own convention (see
//! docs/TODO.md's "module splits" entry) - [`buffer`] groups `ShmHandler`/
//! `BufferHandler`/`DmabufHandler` together since none has more than a
//! handful of lines, and [`misc`] groups the three purely-default-impl stub
//! handlers (`OutputHandler`/`TabletSeatHandler`/`FractionalScaleHandler`)
//! for the same reason; every other module is exactly one handler.

mod buffer;
mod compositor;
mod idle;
mod input_method;
mod layer_shell;
mod misc;
mod seat;
mod selection;
mod xdg_activation;
mod kde_decoration;
mod xdg_decoration;
mod xdg_shell;

use smithay::{
    delegate_compositor, delegate_cursor_shape, delegate_data_control, delegate_data_device, delegate_dmabuf,
    delegate_input_method_manager, delegate_layer_shell, delegate_output, delegate_primary_selection, delegate_seat,
    delegate_session_lock, delegate_shm, delegate_text_input_manager, delegate_virtual_keyboard_manager,
    delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
};

use crate::state::CompState;

delegate_compositor!(CompState);
delegate_xdg_shell!(CompState);
delegate_xdg_decoration!(CompState);
delegate_shm!(CompState);
delegate_dmabuf!(CompState);
delegate_xdg_activation!(CompState);
delegate_text_input_manager!(CompState);
delegate_input_method_manager!(CompState);
delegate_virtual_keyboard_manager!(CompState);
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
