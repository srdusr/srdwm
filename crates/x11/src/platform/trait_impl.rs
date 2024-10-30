use super::*;

impl Platform for X11Platform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::X11
    }

    /// Was `wait_for_event()` (blocks indefinitely for the first event,
    /// only draining any backlog after that), which left `srd`'s IPC socket
    /// - polled at the end of this method - unresponsive for as long as
    /// nothing happened on the X11 connection at all: no keypress, no mouse
    /// motion, nothing. A script sitting on `srd clients` while the user's
    /// hands were off the keyboard for a few seconds would just hang for
    /// exactly that long. Replaced with a bounded `poll(2)` on the
    /// connection's own fd (`~16ms`, matching the Wayland backends' own
    /// frame-ish cadence) so this method always returns roughly that often
    /// regardless of X11 activity, draining whatever's actually arrived
    /// (zero or more events) each time rather than requiring at least one.
    fn poll_events(&mut self) -> PlatformResult<Vec<Event>> {
        self.conn.flush().map_err(err)?;
        let fd = self.conn.stream().as_raw_fd();
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // Safety: `pfd` is a valid, live `pollfd` for the duration of this
        // call, and `poll` writes only into `revents`, which is never read
        // here - the return value alone (ready vs. timed out) is what
        // matters, so a spurious wake or a timeout are both fine outcomes.
        unsafe {
            libc::poll(&mut pfd, 1, 16);
        }

        let mut out = Vec::new();
        while let Some(ev) = self.conn.poll_for_event().map_err(err)? {
            if let Some(e) = self.handle_event(ev)? {
                out.push(e);
            }
        }
        self.apply_registrar_events();
        if let Some(ipc) = self.ipc.as_mut() {
            if ipc.poll(&self.wm) {
                out.push(Event::WorkspaceChanged);
            }
        }
        Ok(out)
    }

    fn monitors(&mut self) -> PlatformResult<Vec<Monitor>> {
        let resources = self.conn.randr_get_screen_resources_current(self.root).map_err(err)?.reply().map_err(err)?;
        let mut monitors = Vec::new();
        for (i, &output) in resources.outputs.iter().enumerate() {
            let info = self.conn.randr_get_output_info(output, resources.config_timestamp).map_err(err)?.reply().map_err(err)?;
            if info.crtc == 0 {
                continue;
            }
            let crtc = self.conn.randr_get_crtc_info(info.crtc, resources.config_timestamp).map_err(err)?.reply().map_err(err)?;
            if crtc.width == 0 || crtc.height == 0 {
                continue;
            }
            let name = String::from_utf8_lossy(&info.name).to_string();
            let mut m = Monitor::new(i as u32, name, Rect::new(crtc.x as i32, crtc.y as i32, crtc.width as u32, crtc.height as u32));
            m.primary = i == 0;
            monitors.push(m);
        }
        if monitors.is_empty() {
            let screen = &self.conn.setup().roots[0];
            monitors.push({
                let mut m = Monitor::new(0, "default", Rect::new(0, 0, screen.width_in_pixels as u32, screen.height_in_pixels as u32));
                m.primary = true;
                m
            });
        }
        Ok(monitors)
    }

    fn apply_geometry(&mut self, window: WindowId, geometry: Rect) -> PlatformResult<()> {
        let Some(frame) = self.frames.get(&window) else { return Ok(()) };
        let (frame_id, client_id) = (frame.frame, frame.client);
        // The titlebar band is only actually reserved when the window is
        // decorated - e.g. a `srd.rule(...)` that sets `decorated = false`
        // - otherwise the client keeps getting offset down by, and
        // shrunk by, a titlebar that `redraw_decoration` (below) is
        // correctly not drawing at all, leaving a blank strip and the
        // frame visibly not matching what's inside it.
        let decorated = self.wm.borrow().window(window).map(|w| w.decorated).unwrap_or(true);
        let band = if decorated { TITLEBAR_HEIGHT } else { 0 };
        self.conn
            .configure_window(
                frame_id,
                &ConfigureWindowAux::new().x(geometry.x).y(geometry.y).width(geometry.width).height(geometry.height),
            )
            .map_err(err)?;
        self.conn
            .configure_window(
                client_id,
                &ConfigureWindowAux::new().x(0).y(band as i32).width(geometry.width).height(geometry.height.saturating_sub(band)),
            )
            .map_err(err)?;
        self.conn.flush().map_err(err)?;
        Ok(())
    }

    fn set_title(&mut self, window: WindowId, title: &str) -> PlatformResult<()> {
        if let Some(frame) = self.frames.get(&window) {
            self.conn.change_property8(x11rb::protocol::xproto::PropMode::REPLACE, frame.client, x11rb::protocol::xproto::AtomEnum::WM_NAME, x11rb::protocol::xproto::AtomEnum::STRING, title.as_bytes()).map_err(err)?;
        }
        Ok(())
    }

    fn focus(&mut self, window: WindowId) -> PlatformResult<()> {
        if let Some(frame) = self.frames.get(&window) {
            let client = frame.client;
            self.conn.set_input_focus(x11rb::protocol::xproto::InputFocus::POINTER_ROOT, client, x11rb::CURRENT_TIME).map_err(err)?;
            self.conn.change_property32(x11rb::protocol::xproto::PropMode::REPLACE, self.root, self.atoms._NET_ACTIVE_WINDOW, x11rb::protocol::xproto::AtomEnum::WINDOW, &[client]).map_err(err)?;
            self.conn.flush().map_err(err)?;
            self.refresh_focused_global_menu(window, client);
        }
        Ok(())
    }

    fn minimize(&mut self, window: WindowId) -> PlatformResult<()> {
        if let Some(frame) = self.frames.get(&window) {
            self.conn.unmap_window(frame.frame).map_err(err)?;
            self.conn.flush().map_err(err)?;
        }
        Ok(())
    }

    fn restore(&mut self, window: WindowId) -> PlatformResult<()> {
        if let Some(frame) = self.frames.get(&window) {
            self.conn.map_window(frame.frame).map_err(err)?;
            self.conn.flush().map_err(err)?;
        }
        Ok(())
    }

    fn close(&mut self, window: WindowId) -> PlatformResult<()> {
        self.request_close(window)
    }

    fn set_decorated(&mut self, window: WindowId, decorated: bool) -> PlatformResult<()> {
        if let Some(w) = self.wm.borrow_mut().window_mut(window) {
            w.decorated = decorated;
        }
        Ok(())
    }

    fn set_border_color(&mut self, window: WindowId, rgb: (u8, u8, u8)) -> PlatformResult<()> {
        if let Some(w) = self.wm.borrow_mut().window_mut(window) {
            w.border_color = rgb;
        }
        if let Some(frame) = self.frame_for(window) {
            self.conn.change_window_attributes(frame, &ChangeWindowAttributesAux::new().border_pixel(rgb_to_pixel(rgb))).map_err(err)?;
            self.conn.flush().map_err(err)?;
        }
        Ok(())
    }

    fn set_border_width(&mut self, window: WindowId, width: u32) -> PlatformResult<()> {
        if let Some(w) = self.wm.borrow_mut().window_mut(window) {
            w.border_width = width;
        }
        if let Some(frame) = self.frame_for(window) {
            self.conn.configure_window(frame, &ConfigureWindowAux::new().border_width(width)).map_err(err)?;
            self.conn.flush().map_err(err)?;
        }
        Ok(())
    }

    fn redraw_decoration(&mut self, window: WindowId, win: &CoreWindow, focused: bool) -> PlatformResult<()> {
        if !win.decorated {
            return Ok(());
        }
        let Some(frame) = self.frame_for(window) else { return Ok(()) };
        let theme = self.wm.borrow().theme;
        let bg = rgb_to_pixel(theme.titlebar_bg);
        let fg = rgb_to_pixel(if focused { theme.titlebar_fg_focused } else { theme.titlebar_fg_unfocused });

        self.conn.change_gc(self.gc, &x11rb::protocol::xproto::ChangeGCAux::new().foreground(bg)).map_err(err)?;
        self.conn
            .poly_fill_rectangle(frame, self.gc, &[Rectangle { x: 0, y: 0, width: win.geometry.width as u16, height: TITLEBAR_HEIGHT as u16 }])
            .map_err(err)?;

        self.conn.change_gc(self.gc, &x11rb::protocol::xproto::ChangeGCAux::new().foreground(fg).font(self.font)).map_err(err)?;
        self.conn.image_text8(frame, self.gc, 6, 20, win.title.as_bytes()).map_err(err)?;

        // Minimize / maximize / close buttons, right-aligned, matching
        // srdwm_core::window::ResizeEdge::hit_test's button layout.
        let btn = TITLEBAR_HEIGHT as i16;
        let right = win.geometry.width as i16;
        let min_x = right - btn * 3;
        let max_x = right - btn * 2;
        let close_x = right - btn;

        self.conn.poly_line(x11rb::protocol::xproto::CoordMode::ORIGIN, frame, self.gc, &[
            x11rb::protocol::xproto::Point { x: min_x + 8, y: 22 },
            x11rb::protocol::xproto::Point { x: min_x + 20, y: 22 },
        ]).map_err(err)?;
        self.conn.poly_rectangle(frame, self.gc, &[Rectangle { x: max_x + 9, y: 9, width: 11, height: 11 }]).map_err(err)?;
        self.conn.poly_line(x11rb::protocol::xproto::CoordMode::ORIGIN, frame, self.gc, &[
            x11rb::protocol::xproto::Point { x: close_x + 8, y: 8 },
            x11rb::protocol::xproto::Point { x: close_x + 22, y: 22 },
        ]).map_err(err)?;
        self.conn.poly_line(x11rb::protocol::xproto::CoordMode::ORIGIN, frame, self.gc, &[
            x11rb::protocol::xproto::Point { x: close_x + 22, y: 8 },
            x11rb::protocol::xproto::Point { x: close_x + 8, y: 22 },
        ]).map_err(err)?;

        self.conn.flush().map_err(err)?;
        Ok(())
    }

    fn grab_keyboard(&mut self) -> PlatformResult<()> {
        // Global bindings are grabbed individually via `grab_keybindings`
        // once the config's key list is known, rather than a blanket
        // keyboard grab (which would also block clients from receiving
        // any keys at all).
        Ok(())
    }

    fn ungrab_keyboard(&mut self) -> PlatformResult<()> {
        self.conn.ungrab_key(0, self.root, ModMask::ANY).map_err(err)?;
        Ok(())
    }

    fn keyboard_layout(&mut self) -> PlatformResult<String> {
        Err(PlatformError::Unsupported("keyboard_layout"))
    }

    fn cycle_keyboard_layout(&mut self) -> PlatformResult<String> {
        Err(PlatformError::Unsupported("cycle_keyboard_layout"))
    }
}
