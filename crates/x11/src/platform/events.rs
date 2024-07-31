use super::*;

impl X11Platform {

    pub(super) fn handle_event(&mut self, event: XEvent) -> PlatformResult<Option<Event>> {
        match event {
            XEvent::MapRequest(ev) => self.manage_new_window(ev.window),
            XEvent::ConfigureRequest(ev) => {
                if self.xid_to_core.contains_key(&ev.window) {
                    // We own layout for managed clients; just ack with a
                    // synthetic ConfigureNotify carrying real geometry.
                    let geom = self.conn.get_geometry(ev.window).map_err(err)?.reply().map_err(err)?;
                    let notify = x11rb::protocol::xproto::ConfigureNotifyEvent {
                        response_type: x11rb::protocol::xproto::CONFIGURE_NOTIFY_EVENT,
                        sequence: 0,
                        event: ev.window,
                        window: ev.window,
                        above_sibling: x11rb::NONE,
                        x: geom.x,
                        y: geom.y,
                        width: geom.width,
                        height: geom.height,
                        border_width: 0,
                        override_redirect: false,
                    };
                    self.conn.send_event(false, ev.window, EventMask::STRUCTURE_NOTIFY, notify).map_err(err)?;
                } else {
                    let aux = ConfigureWindowAux::from_configure_request(&ev);
                    self.conn.configure_window(ev.window, &aux).map_err(err)?;
                }
                self.conn.flush().map_err(err)?;
                Ok(None)
            }
            XEvent::UnmapNotify(ev) => Ok(self.unmanage(ev.window)),
            XEvent::DestroyNotify(ev) => Ok(self.unmanage(ev.window)),
            XEvent::ButtonPress(ev) => {
                let (x, y) = (ev.root_x as i32, ev.root_y as i32);
                let hit = self.wm.borrow().hit_test(x, y);
                if let Some((id, hit)) = hit {
                    self.raise_and_focus(id)?;
                    match hit {
                        TitlebarHit::Drag => self.wm.borrow_mut().start_drag(id, x, y),
                        TitlebarHit::Close => self.request_close(id)?,
                        TitlebarHit::Maximize => {
                            self.wm.borrow_mut().toggle_maximize(id);
                            self.sync_geometry(id)?;
                        }
                        TitlebarHit::Minimize => {
                            self.wm.borrow_mut().minimize_window(id);
                            if let Some(frame) = self.frame_for(id) {
                                self.conn.unmap_window(frame).map_err(err)?;
                            }
                        }
                        TitlebarHit::Resize(edge) => self.wm.borrow_mut().start_resize(id, edge, x, y),
                    }
                    self.conn.flush().map_err(err)?;
                }
                // Let the click through to the client (we grabbed it SYNC).
                self.conn.allow_events(x11rb::protocol::xproto::Allow::REPLAY_POINTER, ev.time).map_err(err)?;
                self.conn.flush().map_err(err)?;
                Ok(Some(Event::MouseButtonPress { button: MouseButton::Left, x, y }))
            }
            XEvent::ButtonRelease(ev) => {
                let mut wm = self.wm.borrow_mut();
                let was_dragging = wm.is_dragging();
                let was_resizing = wm.is_resizing();
                let dragged_id = wm.focused_id();
                if was_dragging {
                    wm.end_drag();
                } else if was_resizing {
                    wm.end_resize();
                }
                drop(wm);
                if was_dragging || was_resizing {
                    if let Some(id) = dragged_id {
                        self.sync_geometry(id)?;
                    }
                }
                Ok(Some(Event::MouseButtonRelease { button: MouseButton::Left, x: ev.root_x as i32, y: ev.root_y as i32 }))
            }
            XEvent::MotionNotify(ev) => {
                let (x, y) = (ev.root_x as i32, ev.root_y as i32);
                let mut wm = self.wm.borrow_mut();
                let id = wm.focused_id();
                if wm.is_dragging() {
                    wm.update_drag(x, y);
                } else if wm.is_resizing() {
                    wm.update_resize(x, y);
                } else {
                    return Ok(Some(Event::MouseMotion { x, y }));
                }
                drop(wm);
                if let Some(id) = id {
                    self.sync_geometry(id)?;
                }
                Ok(Some(Event::MouseMotion { x, y }))
            }
            XEvent::KeyPress(ev) => {
                let keysym = self.keycode_to_keysym(ev.detail);
                let Some(key_name) = keysyms::keysym_to_name(keysym) else { return Ok(None) };
                Ok(Some(Event::KeyPress { key_name, modifiers: Self::modifiers_from_state(ev.state.into()) }))
            }
            XEvent::KeyRelease(ev) => {
                let keysym = self.keycode_to_keysym(ev.detail);
                let Some(key_name) = keysyms::keysym_to_name(keysym) else { return Ok(None) };
                Ok(Some(Event::KeyRelease { key_name, modifiers: Self::modifiers_from_state(ev.state.into()) }))
            }
            XEvent::Expose(ev) => {
                let target = self.frames.iter().find(|(_, f)| f.frame == ev.window).map(|(&id, _)| id);
                if let Some(id) = target {
                    let w = self.wm.borrow().window(id).cloned_for_render();
                    if let Some(w) = w {
                        let focused = self.wm.borrow().focused_id() == Some(id);
                        let _ = self.redraw_decoration(id, &w, focused);
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}
