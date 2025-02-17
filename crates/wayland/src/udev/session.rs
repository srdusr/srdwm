use super::*;

pub(crate) fn register_drm_fd(handle: &LoopHandle<'static, CompState>, card: &Rc<Card>) -> PlatformResult<()> {
    let raw = card.as_fd().as_raw_fd();
    // SAFETY: `FdWrapper` does not close `raw`; the owning `Card` lives in
    // `CompState::udev` for as long as this event source is registered.
    let wrapper = unsafe { FdWrapper::new(raw) };
    let source = Generic::new(wrapper, Interest::READ, CalloopMode::Level);
    handle
        .insert_source(source, move |_, _, data: &mut CompState| {
            let Some(udev) = data.udev.as_ref() else { return Ok(PostAction::Continue) };
            let card = udev.card.clone();
            match card.receive_events() {
                Ok(events) => {
                    // The event names the CRTC it came from, so with several
                    // monitors only that head advances - flipping all of
                    // them would desynchronise the others' buffers.
                    let mut flipped = false;
                    for event in events {
                        let DrmEvent::PageFlip(flip) = event else { continue };
                        if let Some(udev) = data.udev.as_mut() {
                            if let Some(head) = udev.heads.iter_mut().find(|h| h.crtc == flip.crtc) {
                                head.front = 1 - head.front;
                                head.flip_pending = false;
                                flipped = true;
                            }
                        }
                    }
                    if flipped {
                        data.render_udev_frame();
                    }
                }
                Err(e) => log::warn!("udev: receive_events failed: {e}"),
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| PlatformError::Other(format!("failed to register DRM fd: {e}")))?;
    Ok(())
}

pub(crate) fn register_libinput(handle: &LoopHandle<'static, CompState>, session: &LibSeatSession, seat_name: &str) -> PlatformResult<()> {
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context.udev_assign_seat(seat_name).map_err(|_| PlatformError::Other("udev: libinput udev_assign_seat failed".into()))?;
    let libinput_backend = LibinputInputBackend::new(libinput_context);

    handle
        .insert_source(libinput_backend, move |event, _, data: &mut CompState| {
            handle_libinput_event(data, event);
        })
        .map_err(|e| PlatformError::Other(format!("failed to register libinput backend: {e}")))?;
    Ok(())
}

pub(crate) fn register_session_notifier(handle: &LoopHandle<'static, CompState>, notifier: LibSeatSessionNotifier) -> PlatformResult<()> {
    handle
        .insert_source(notifier, move |event, &mut (), data: &mut CompState| {
            let Some(udev) = data.udev.as_mut() else { return };
            match event {
                SessionEvent::PauseSession => {
                    log::info!("udev: session paused (VT switch away)");
                    udev.active = false;
                }
                SessionEvent::ActivateSession => {
                    log::info!("udev: session resumed (VT switch back)");
                    udev.active = true;
                    let card = udev.card.clone();

                    // A flip issued right before the VT switch away may
                    // never have completed while inactive (nothing was
                    // scanning out), and its completion event can still be
                    // sitting undelivered on the DRM fd. The kernel refuses
                    // a new page flip on a CRTC with one already
                    // unacknowledged (EBUSY) - drain and apply any such
                    // events now, before reasserting crtcs, so a stale flip
                    // from before the switch can't collide with the fresh
                    // one `render_udev_frame` is about to issue below.
                    match card.receive_events() {
                        Ok(events) => {
                            for event in events {
                                let DrmEvent::PageFlip(flip) = event else { continue };
                                if let Some(head) = udev.heads.iter_mut().find(|h| h.crtc == flip.crtc) {
                                    head.front = 1 - head.front;
                                    head.flip_pending = false;
                                }
                            }
                        }
                        Err(e) => log::debug!("udev: no pending flip events to drain on resume: {e}"),
                    }

                    // Some drivers reset mode-setting state across a VT
                    // switch; reassert every head before rendering again.
                    for head in &mut udev.heads {
                        let fb = head.buffers[head.front].fb;
                        if let Err(e) = card.set_crtc(head.crtc, Some(fb), (0, 0), &[], None) {
                            log::warn!("udev: failed to reassert crtc on resume: {e}");
                        }
                        // Force a full repaint: contents are undefined after
                        // the VT switch (another VT's session may have
                        // scanned out something else entirely in between).
                        head.flip_pending = false;
                        head.ages = [0, 0];
                        head.flip_retry_after = None;
                    }
                    data.render_udev_frame();
                }
            }
        })
        .map_err(|e| PlatformError::Other(format!("failed to register session notifier: {e}")))?;
    Ok(())
}

/// Watches udev for DRM device changes. The kernel emits a `change` uevent
/// on the card when a connector is plugged or unplugged, which smithay
/// surfaces as [`UdevEvent::Changed`] - that is the hotplug signal.
///
/// `Added`/`Removed` refer to whole GPUs appearing or disappearing, which
/// this backend does not support (it binds one primary GPU at startup), so
/// they are logged and ignored rather than silently dropped.
pub(crate) fn register_udev_monitor(handle: &LoopHandle<'static, CompState>, seat_name: &str) -> PlatformResult<()> {
    let backend = UdevBackend::new(seat_name).map_err(err)?;
    handle
        .insert_source(backend, move |event, _, data: &mut CompState| match event {
            UdevEvent::Changed { .. } => {
                data.reprobe_outputs();
                data.render_udev_frame();
            }
            UdevEvent::Added { path, .. } => {
                log::info!("udev: new GPU {} appeared; multi-GPU is not supported, ignoring", path.display())
            }
            UdevEvent::Removed { .. } => log::info!("udev: a GPU was removed; multi-GPU is not supported, ignoring"),
        })
        .map_err(|e| PlatformError::Other(format!("failed to register udev monitor: {e}")))?;
    Ok(())
}

fn handle_libinput_event(state: &mut CompState, event: InputEvent<LibinputInputBackend>) {
    match event {
        InputEvent::Keyboard { event } => handle_keyboard_key_event(state, &event),
        InputEvent::PointerMotion { event } => {
            let Some(udev) = state.udev.as_mut() else { return };
            let delta = event.delta();
            // Clamped to the union of every head, so the pointer travels
            // between monitors instead of stopping at the first one's edge.
            let (w, h) = udev.bounds();
            udev.pointer_pos.x = (udev.pointer_pos.x + delta.x).clamp(0.0, (w - 1.0).max(0.0));
            udev.pointer_pos.y = (udev.pointer_pos.y + delta.y).clamp(0.0, (h - 1.0).max(0.0));
            let pos = udev.pointer_pos;
            handle_pointer_position(state, pos, event.time_msec());
        }
        InputEvent::PointerButton { event } => {
            let Some(pos) = state.udev.as_ref().map(|u| u.pointer_pos) else { return };
            let button = event.button_code();
            let pressed = event.state() == BackendButtonState::Pressed;
            handle_pointer_button(state, pos, button, pressed, event.time_msec());
        }
        // Laptop lid. libinput reports this as a switch toggle; without
        // handling it, closing the lid does nothing at all - no lock, no
        // suspend - which is a genuine problem on a laptop rather than a
        // missing nicety.
        InputEvent::SwitchToggle { event } => {
            // Fully qualified: libinput's own `Switch` is also in scope here.
            use smithay::backend::input::{SwitchState, SwitchToggleEvent};
            if matches!(event.switch(), Some(smithay::reexports::input::event::switch::Switch::Lid)) {
                let closed = event.state() == SwitchState::On;
                log::info!("lid {}", if closed { "closed" } else { "opened" });
                state.pending.borrow_mut().push(CoreEvent::LidSwitch { closed });
            }
        }
        InputEvent::PointerAxis { event } => {
            // Modifier+scroll switches workspace instead of reaching the
            // client - the `bind = SUPER, mouse_down/up, workspace, e+1/e-1`
            // gesture. Checked first so the client never sees these events;
            // forwarding them too would scroll the window under the cursor
            // as a side effect of changing workspace.
            if crate::input::handle_workspace_scroll(state, &event) {
                return;
            }
            // Otherwise: forwarded to the focused client via the pointer axis
            // frame, no WM-level handling.
            let Some(pointer) = state.seat.get_pointer() else { return };
            let source = event.source();
            let mut frame = AxisFrame::new(event.time_msec()).source(source);
            for axis in [Axis::Horizontal, Axis::Vertical] {
                match event.amount(axis) {
                    Some(value) => frame = frame.value(axis, value),
                    // `AxisSource::Finger` (a touchpad) *requires* a stop
                    // event on the frame where the finger lifts and the
                    // axis genuinely has no more motion - see `AxisFrame::
                    // source`'s own doc comment ("Using AxisSource::Finger
                    // requires a stop event to be sent, when the user lifts
                    // off the finger"). Never sending it left every
                    // two-finger scroll gesture with no way to tell Firefox/
                    // GTK it had actually ended, which is exactly the kind
                    // of thing that reads as "scrolling doesn't work" --
                    // not "no events arrive" (discrete wheel scrolling,
                    // which needs no stop event, was never affected) but
                    // kinetic/momentum scrolling and starting a fresh
                    // gesture right after a previous one never settling.
                    None if source == AxisSource::Finger => frame = frame.stop(axis),
                    None => {}
                }
                // Discrete wheel steps, additional to the pixel `value`
                // above - optional (`value` is the only event a client
                // strictly needs), but some clients use it to distinguish
                // "one physical click" from a smooth/high-resolution
                // scroll, so provide it whenever the device actually
                // reports one (real scroll wheels; never touchpads, which
                // have no discrete steps to report - `amount_v120` is
                // `None` for those, same guarantee `amount` gives the
                // other way around).
                if let Some(v120) = event.amount_v120(axis) {
                    frame = frame.v120(axis, v120 as i32);
                }
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }
        // 3+-finger swipe - claimed entirely for workspace switching, never
        // reaches a client. See `handle_gesture_swipe_end`'s doc comment.
        InputEvent::GestureSwipeBegin { event } => handle_gesture_swipe_begin(state, &event),
        InputEvent::GestureSwipeUpdate { event } => handle_gesture_swipe_update(state, &event),
        InputEvent::GestureSwipeEnd { event } => handle_gesture_swipe_end(state, &event),
        // Pinch/hold: no WM-level meaning, forwarded to the focused client
        // as-is (`wp_pointer_gestures`) - pinch-to-zoom in an image viewer
        // or PDF reader, the one real use either has. Same reasoning as the
        // `PointerAxis` forwarding above: nothing here should be silently
        // dropped just because this WM has no use for it itself.
        InputEvent::GesturePinchBegin { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let fingers = event.fingers();
            pointer.gesture_pinch_begin(state, &GesturePinchBeginEvent { serial: SERIAL_COUNTER.next_serial(), time: event.time_msec(), fingers });
        }
        InputEvent::GesturePinchUpdate { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let (delta, scale, rotation) = (event.delta(), event.scale(), event.rotation());
            pointer.gesture_pinch_update(state, &GesturePinchUpdateEvent { time: event.time_msec(), delta, scale, rotation });
        }
        InputEvent::GesturePinchEnd { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let cancelled = event.cancelled();
            pointer.gesture_pinch_end(state, &GesturePinchEndEvent { serial: SERIAL_COUNTER.next_serial(), time: event.time_msec(), cancelled });
        }
        InputEvent::GestureHoldBegin { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let fingers = event.fingers();
            pointer.gesture_hold_begin(state, &GestureHoldBeginEvent { serial: SERIAL_COUNTER.next_serial(), time: event.time_msec(), fingers });
        }
        InputEvent::GestureHoldEnd { event } => {
            let Some(pointer) = state.seat.get_pointer() else { return };
            let cancelled = event.cancelled();
            pointer.gesture_hold_end(state, &GestureHoldEndEvent { serial: SERIAL_COUNTER.next_serial(), time: event.time_msec(), cancelled });
        }
        _ => {}
    }
}

