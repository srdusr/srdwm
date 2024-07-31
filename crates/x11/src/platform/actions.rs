use super::*;

impl X11Platform {

    pub(super) fn raise_and_focus(&mut self, id: WindowId) -> PlatformResult<()> {
        self.wm.borrow_mut().focus_window(id);
        if let Some(frame) = self.frame_for(id) {
            self.conn.configure_window(frame, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).map_err(err)?;
        }
        self.redraw_all_decorations()?;
        Ok(())
    }

    pub(super) fn request_close(&mut self, id: WindowId) -> PlatformResult<()> {
        let Some(frame) = self.frames.get(&id) else { return Ok(()) };
        if frame.supports_delete {
            let event = x11rb::protocol::xproto::ClientMessageEvent::new(
                32,
                frame.client,
                self.atoms.WM_PROTOCOLS,
                [self.atoms.WM_DELETE_WINDOW, x11rb::CURRENT_TIME, 0, 0, 0],
            );
            self.conn.send_event(false, frame.client, EventMask::NO_EVENT, event).map_err(err)?;
        } else {
            self.conn.destroy_window(frame.client).map_err(err)?;
        }
        Ok(())
    }

    pub(super) fn sync_geometry(&mut self, id: WindowId) -> PlatformResult<()> {
        let geom = self.wm.borrow().window(id).map(|w| w.geometry);
        if let Some(g) = geom {
            self.apply_geometry(id, g)?;
        }
        Ok(())
    }

    fn redraw_all_decorations(&mut self) -> PlatformResult<()> {
        let focused = self.wm.borrow().focused_id();
        let ids: Vec<WindowId> = self.frames.keys().copied().collect();
        for id in ids {
            let w = self.wm.borrow().window(id).cloned_for_render();
            if let Some(w) = w {
                self.redraw_decoration(id, &w, focused == Some(id))?;
            }
        }
        Ok(())
    }
}
