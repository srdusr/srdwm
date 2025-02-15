//! `wl_seat`: keyboard/pointer/touch focus types and the client-set cursor
//! image.

use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::CompState;

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
        // The client has now explicitly claimed the cursor - see
        // `decoration_cursor_active`'s own doc comment and `input.rs::
        // update_cursor_shape` for why this has to be tracked separately
        // from just overwriting `cursor_status`.
        self.decoration_cursor_active = false;
    }
}
