use super::*;

pub(super) fn handle_winit_event(state: &mut CompState, output: &Output, event: WinitEvent, closed: &mut bool) {
    match event {
        WinitEvent::CloseRequested => *closed = true,
        WinitEvent::Input(InputEvent::Keyboard { event }) => handle_keyboard_key_event(state, &event),
        WinitEvent::Input(InputEvent::PointerMotionAbsolute { event }) => {
            let size = output.current_mode().map(|m| m.size).unwrap_or_default().to_logical(1);
            let pos = event.position_transformed(size);
            handle_pointer_position(state, pos, event.time_msec());
        }
        WinitEvent::Input(InputEvent::PointerButton { event }) => {
            let pos = last_pointer_pos(state);
            let button = event.button_code();
            let pressed = event.state() == BackendButtonState::Pressed;
            handle_pointer_button(state, pos, button, pressed, event.time_msec());
        }
        // This backend had no scroll handling at all - `InputEvent::
        // PointerAxis` fell into the catch-all below and was silently
        // dropped, unconditionally, on every device. Same forwarding as
        // `udev.rs`'s equivalent (see its own comment for the `stop()`/
        // `v120()` reasoning); duplicated rather than shared since the two
        // backends' `InputEvent` generic parameters differ and there's no
        // shared event type to write one function against.
        WinitEvent::Input(InputEvent::PointerAxis { event }) => {
            if crate::input::handle_workspace_scroll(state, &event) {
                return;
            }
            let Some(pointer) = state.seat.get_pointer() else { return };
            let source = event.source();
            let mut frame = smithay::input::pointer::AxisFrame::new(event.time_msec()).source(source);
            for axis in [Axis::Horizontal, Axis::Vertical] {
                match event.amount(axis) {
                    Some(value) => frame = frame.value(axis, value),
                    None if source == AxisSource::Finger => frame = frame.stop(axis),
                    None => {}
                }
                if let Some(v120) = event.amount_v120(axis) {
                    frame = frame.v120(axis, v120 as i32);
                }
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }
        WinitEvent::Resized { .. } => {}
        _ => {}
    }
}
