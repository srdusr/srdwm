use super::*;

impl X11Platform {

    pub(super) fn manage_new_window(&mut self, client: XWindow) -> PlatformResult<Option<Event>> {
        let geom = self.conn.get_geometry(client).map_err(err)?.reply().map_err(err)?;
        let title = self.window_title(client).unwrap_or_default();
        let (instance, class) = self.window_class(client);
        let supports_delete = self.supports_wm_delete(client);

        let id = {
            let mut wm = self.wm.borrow_mut();
            let id = wm.alloc_window_id();
            let mut w = CoreWindow::new(id, title);
            w.app_id = class;
            w.instance = instance;
            w.geometry = Rect::new(geom.x as i32, geom.y as i32, geom.width as u32, geom.height as u32 + TITLEBAR_HEIGHT);
            wm.add_window(w);
            id
        };
        let placed = self.wm.borrow().window(id).map(|w| w.geometry).unwrap_or(Rect::new(0, 0, 640, 480));

        let frame = self.conn.generate_id().map_err(err)?;
        let aux = CreateWindowAux::new()
            .event_mask(
                EventMask::SUBSTRUCTURE_REDIRECT
                    | EventMask::SUBSTRUCTURE_NOTIFY
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::EXPOSURE,
            )
            .background_pixel(self.conn.setup().roots[0].white_pixel);
        // `Window.border_color`/`border_width` were tracked in
        // `srdwm_core::Window` and settable via `srd.window.set_border_*`,
        // but nothing ever actually drew a border with them on this
        // backend - `set_border_color`/`set_border_width` below only
        // updated the stored struct field. X11 windows have a native
        // server-drawn border (`border_pixel`/the `create_window`
        // `border-width` parameter, both unconditionally 0 here before),
        // so this uses that rather than hand-rendering one - the X server
        // draws it, no extra composite work needed.
        let border_color = self.wm.borrow().window(id).map(|w| w.border_color).unwrap_or((0x31, 0x32, 0x44));
        let border_width = self.wm.borrow().window(id).map(|w| w.border_width).unwrap_or(0);
        let aux = aux.border_pixel(rgb_to_pixel(border_color));
        self.conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                frame,
                self.root,
                placed.x as i16,
                placed.y as i16,
                placed.width as u16,
                placed.height as u16,
                border_width as u16,
                WindowClass::INPUT_OUTPUT,
                0,
                &aux,
            )
            .map_err(err)?;

        self.conn.reparent_window(client, frame, 0, TITLEBAR_HEIGHT as i16).map_err(err)?;
        self.conn
            .configure_window(client, &ConfigureWindowAux::new().width(placed.width).height(placed.height.saturating_sub(TITLEBAR_HEIGHT)))
            .map_err(err)?;

        // Passive-grab button1 on the client so our first click focuses/raises
        // it, then replay the click through to the app - the standard
        // click-to-focus pattern used by dwm/openbox/etc.
        self.conn
            .grab_button(
                false,
                client,
                EventMask::BUTTON_PRESS,
                GrabMode::SYNC,
                GrabMode::ASYNC,
                x11rb::NONE,
                x11rb::NONE,
                ButtonIndex::M1,
                ModMask::ANY,
            )
            .map_err(err)?;

        self.conn.map_window(client).map_err(err)?;
        self.conn.map_window(frame).map_err(err)?;
        self.conn
            .change_property32(x11rb::protocol::xproto::PropMode::APPEND, self.root, self.atoms._NET_CLIENT_LIST, x11rb::protocol::xproto::AtomEnum::WINDOW, &[client])
            .map_err(err)?;
        self.conn.flush().map_err(err)?;

        self.xid_to_core.insert(client, id);
        self.frames.insert(id, Frame { frame, client, supports_delete });

        let w = self.wm.borrow().window(id).cloned_for_render();
        if let Some(w) = w {
            let _ = self.redraw_decoration(id, &w, true);
        }

        Ok(Some(Event::WindowCreated(id)))
    }

    fn window_title(&self, client: XWindow) -> Option<String> {
        let reply = self
            .conn
            .get_property(false, client, self.atoms._NET_WM_NAME, self.atoms.UTF8_STRING, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        if reply.value_len > 0 {
            return String::from_utf8(reply.value).ok();
        }
        let reply = self
            .conn
            .get_property(false, client, x11rb::protocol::xproto::AtomEnum::WM_NAME, x11rb::protocol::xproto::AtomEnum::STRING, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        String::from_utf8(reply.value).ok()
    }

    /// Reads `WM_CLASS` and splits it into `(instance, class)` - the
    /// property is two NUL-terminated strings back to back, instance first
    /// (ICCCM 4.1.2.5). Was never read at all before this: `manage_new_window`
    /// only ever set `Window::title`, leaving `app_id` permanently empty on
    /// every X11 window - meaning every `srd.rule({ class = ... }, ...)`
    /// silently failed to match anything on this backend, the same root
    /// cause `with_toplevel_app_id`'s doc comment describes already having
    /// been found and fixed for native Wayland windows earlier. Returns
    /// `("", "")` if the property is missing or malformed rather than an
    /// `Option`, since both halves are used unconditionally either way.
    fn window_class(&self, client: XWindow) -> (String, String) {
        let Ok(cookie) = self.conn.get_property(false, client, x11rb::protocol::xproto::AtomEnum::WM_CLASS, x11rb::protocol::xproto::AtomEnum::STRING, 0, 1024)
        else {
            return (String::new(), String::new());
        };
        let Ok(reply) = cookie.reply() else { return (String::new(), String::new()) };
        let mut parts = reply.value.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned());
        let instance = parts.next().unwrap_or_default();
        let class = parts.next().unwrap_or_default();
        (instance, class)
    }

    fn supports_wm_delete(&self, client: XWindow) -> bool {
        let Ok(cookie) = self.conn.get_property(false, client, self.atoms.WM_PROTOCOLS, x11rb::protocol::xproto::AtomEnum::ATOM, 0, 32) else {
            return false;
        };
        let Ok(reply) = cookie.reply() else { return false };
        reply
            .value32()
            .map(|mut it| it.any(|a| a == self.atoms.WM_DELETE_WINDOW))
            .unwrap_or(false)
    }

    pub(super) fn unmanage(&mut self, client: XWindow) -> Option<Event> {
        let id = self.xid_to_core.remove(&client)?;
        if let Some(frame) = self.frames.remove(&id) {
            let _ = self.conn.destroy_window(frame.frame);
        }
        // A closed window's own context menu (opened right before, say, a
        // client that immediately quits) would otherwise dangle - its
        // `MenuAction::Close`/etc. would target a `WindowId` `remove_
        // window` below has already forgotten.
        if self.context_menu.as_ref().is_some_and(|(menu, _)| menu.window == id) {
            let _ = self.close_context_menu();
        }
        self.wm.borrow_mut().remove_window(id);
        let _ = self.conn.flush();
        Some(Event::WindowDestroyed(id))
    }

    pub(super) fn frame_for(&self, id: WindowId) -> Option<XWindow> {
        self.frames.get(&id).map(|f| f.frame)
    }
}
