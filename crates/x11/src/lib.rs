//! X11 backend for srdwm: a classic reparenting window manager that draws
//! its own title bar (close/maximize/minimize buttons, drag-to-move,
//! edge/corner resize) rather than relying on any toolkit's decorations -
//! this is what gives "full title bar support" parity with Windows/macOS on
//! X11.
//!
//! Compared to the legacy C++ `x11_platform.cc` (see docs/PRIOR_ART.md),
//! this fixes several real bugs rather than porting them:
//! - the titlebar was drawn at a hardcoded 800px width; here it's sized to
//!   the actual frame width every time (see `redraw_decoration`).
//! - there was no drag/resize/button hit-testing at all; here it's the
//!   shared `srdwm_core::window::ResizeEdge::hit_test` used by every backend.
//! - `check_for_other_wm` always returned `true` because its error handler
//!   discarded errors; here we do a *checked* `change_window_attributes`
//!   with `SUBSTRUCTURE_REDIRECT` and propagate a real `BadAccess` as
//!   [`PlatformError::AnotherWmRunning`].
//! - RandR monitor geometry used the output's physical size in
//!   millimeters instead of the CRTC's pixel mode; here it reads the CRTC.
//!
//! Not implemented (documented rather than faked): XKB-level keymaps (only
//! a hand-maintained keysym table covering common keys, see `keysyms.rs`),
//! ICCCM `WM_HINTS`/urgency, and EWMH pager/taskbar hints beyond
//! `_NET_SUPPORTED`/`_NET_CLIENT_LIST`/`_NET_WM_STATE` maximize.

mod keysyms;

use srdwm_core::{Event, Modifiers, MouseButton, TitlebarHit, Window as CoreWindow, WindowId, TITLEBAR_HEIGHT};
use srdwm_core::{Monitor, Rect, WindowManager};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    ButtonIndex, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    EventMask, GrabMode, ModMask, Rectangle, StackMode, Window as XWindow, WindowClass,
};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        WM_STATE,
        _NET_SUPPORTED,
        _NET_WM_NAME,
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_CLIENT_LIST,
        _NET_ACTIVE_WINDOW,
        UTF8_STRING,
    }
}

struct Frame {
    frame: XWindow,
    client: XWindow,
    supports_delete: bool,
}

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(e.to_string())
}

pub struct X11Platform {
    conn: RustConnection,
    root: XWindow,
    atoms: Atoms,
    gc: x11rb::protocol::xproto::Gcontext,
    font: x11rb::protocol::xproto::Font,
    wm: Rc<RefCell<WindowManager>>,
    xid_to_core: HashMap<XWindow, WindowId>,
    frames: HashMap<WindowId, Frame>,
    min_keycode: u8,
    max_keycode: u8,
    keysyms_per_keycode: u8,
    keyboard_mapping: Vec<u32>,
}

impl X11Platform {
    pub fn connect(wm: Rc<RefCell<WindowManager>>) -> PlatformResult<Self> {
        let (conn, screen_num) = RustConnection::connect(None).map_err(|e| PlatformError::ConnectionFailed(e.to_string()))?;
        let root = conn.setup().roots[screen_num].root;

        // Registering for SUBSTRUCTURE_REDIRECT is how X tells us "no other
        // WM may do this" - if one already has it, this request comes back
        // as a checked BadAccess. This replaces the legacy check, which
        // always returned true because its error handler discarded errors.
        let aux = ChangeWindowAttributesAux::new()
            .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE);
        conn.change_window_attributes(root, &aux).map_err(err)?.check().map_err(|_| PlatformError::AnotherWmRunning)?;

        let atoms = Atoms::new(&conn).map_err(err)?.reply().map_err(err)?;
        conn.change_property32(x11rb::protocol::xproto::PropMode::REPLACE, root, atoms._NET_SUPPORTED, x11rb::protocol::xproto::AtomEnum::ATOM, &[
            atoms._NET_WM_STATE,
            atoms._NET_WM_STATE_MAXIMIZED_VERT,
            atoms._NET_WM_STATE_MAXIMIZED_HORZ,
            atoms._NET_CLIENT_LIST,
            atoms._NET_ACTIVE_WINDOW,
        ]).map_err(err)?;

        let font = conn.generate_id().map_err(err)?;
        conn.open_font(font, b"fixed").map_err(err)?;

        let gc = conn.generate_id().map_err(err)?;
        let gc_aux = CreateGCAux::new().font(font).graphics_exposures(0);
        conn.create_gc(gc, root, &gc_aux).map_err(err)?;

        let setup = conn.setup();
        let (min_keycode, max_keycode) = (setup.min_keycode, setup.max_keycode);
        let mapping = conn
            .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)
            .map_err(err)?
            .reply()
            .map_err(err)?;
        let keysyms_per_keycode = mapping.keysyms_per_keycode;
        let keyboard_mapping = mapping.keysyms;

        conn.flush().map_err(err)?;

        Ok(Self {
            conn,
            root,
            atoms,
            gc,
            font,
            wm,
            xid_to_core: HashMap::new(),
            frames: HashMap::new(),
            min_keycode,
            max_keycode,
            keysyms_per_keycode,
            keyboard_mapping,
        })
    }

    fn keycode_to_keysym(&self, keycode: u8) -> u32 {
        if keycode < self.min_keycode || keycode > self.max_keycode || self.keysyms_per_keycode == 0 {
            return 0;
        }
        let idx = (keycode - self.min_keycode) as usize * self.keysyms_per_keycode as usize;
        self.keyboard_mapping.get(idx).copied().unwrap_or(0)
    }

    fn keysym_to_keycode(&self, keysym: u32) -> Option<u8> {
        for kc in self.min_keycode..=self.max_keycode {
            let idx = (kc - self.min_keycode) as usize * self.keysyms_per_keycode as usize;
            if self.keyboard_mapping.get(idx).copied() == Some(keysym) {
                return Some(kc);
            }
        }
        None
    }

    fn modifiers_from_state(state: u16) -> Modifiers {
        let mut m = Modifiers::empty();
        if state & ModMask::SHIFT.bits() != 0 {
            m |= Modifiers::SHIFT;
        }
        if state & ModMask::CONTROL.bits() != 0 {
            m |= Modifiers::CTRL;
        }
        if state & ModMask::M1.bits() != 0 {
            m |= Modifiers::ALT;
        }
        if state & ModMask::M4.bits() != 0 {
            m |= Modifiers::SUPER;
        }
        m
    }

    fn modmask_for(modifiers: Modifiers) -> ModMask {
        let mut mask = ModMask::from(0u16);
        if modifiers.contains(Modifiers::SHIFT) {
            mask |= ModMask::SHIFT;
        }
        if modifiers.contains(Modifiers::CTRL) {
            mask |= ModMask::CONTROL;
        }
        if modifiers.contains(Modifiers::ALT) {
            mask |= ModMask::M1;
        }
        if modifiers.contains(Modifiers::SUPER) {
            mask |= ModMask::M4;
        }
        mask
    }

    /// Grabs the given `"Mod4+Shift+Return"`-style key combos on the root
    /// window so their KeyPress events reach us even when a client has
    /// input focus. Call after loading config (once bindings are known).
    pub fn grab_keybindings(&mut self, combos: &[String]) -> PlatformResult<()> {
        for combo in combos {
            let parts: Vec<&str> = combo.split('+').collect();
            let Some((key_name, mod_parts)) = parts.split_last() else { continue };
            let mut modifiers = Modifiers::empty();
            for m in mod_parts {
                modifiers |= match *m {
                    "Ctrl" => Modifiers::CTRL,
                    "Shift" => Modifiers::SHIFT,
                    "Alt" => Modifiers::ALT,
                    "Mod4" | "Super" => Modifiers::SUPER,
                    _ => Modifiers::empty(),
                };
            }
            let Some(keysym) = keysyms::name_to_keysym(key_name) else {
                log::warn!("cannot grab '{combo}': unknown key name '{key_name}'");
                continue;
            };
            let Some(keycode) = self.keysym_to_keycode(keysym) else {
                log::warn!("cannot grab '{combo}': no keycode for keysym {keysym:#x}");
                continue;
            };
            let mask = Self::modmask_for(modifiers);
            self.conn
                .grab_key(true, self.root, mask, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(err)?;
        }
        self.conn.flush().map_err(err)?;
        Ok(())
    }

    fn manage_new_window(&mut self, client: XWindow) -> PlatformResult<Option<Event>> {
        let geom = self.conn.get_geometry(client).map_err(err)?.reply().map_err(err)?;
        let title = self.window_title(client).unwrap_or_default();
        let supports_delete = self.supports_wm_delete(client);

        let id = {
            let mut wm = self.wm.borrow_mut();
            let id = wm.alloc_window_id();
            let mut w = CoreWindow::new(id, title);
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
        self.conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                frame,
                self.root,
                placed.x as i16,
                placed.y as i16,
                placed.width as u16,
                placed.height as u16,
                0,
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

    fn unmanage(&mut self, client: XWindow) -> Option<Event> {
        let id = self.xid_to_core.remove(&client)?;
        if let Some(frame) = self.frames.remove(&id) {
            let _ = self.conn.destroy_window(frame.frame);
        }
        self.wm.borrow_mut().remove_window(id);
        let _ = self.conn.flush();
        Some(Event::WindowDestroyed(id))
    }

    fn frame_for(&self, id: WindowId) -> Option<XWindow> {
        self.frames.get(&id).map(|f| f.frame)
    }

    fn handle_event(&mut self, event: XEvent) -> PlatformResult<Option<Event>> {
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

    fn raise_and_focus(&mut self, id: WindowId) -> PlatformResult<()> {
        self.wm.borrow_mut().focus_window(id);
        if let Some(frame) = self.frame_for(id) {
            self.conn.configure_window(frame, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).map_err(err)?;
        }
        self.redraw_all_decorations()?;
        Ok(())
    }

    fn request_close(&mut self, id: WindowId) -> PlatformResult<()> {
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

    fn sync_geometry(&mut self, id: WindowId) -> PlatformResult<()> {
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

/// Small helper so we can grab an owned snapshot of a `&Window` out of a
/// `Ref<WindowManager>` borrow without holding the borrow across the redraw call.
trait ClonedForRender {
    fn cloned_for_render(self) -> Option<CoreWindow>;
}
impl ClonedForRender for Option<&CoreWindow> {
    fn cloned_for_render(self) -> Option<CoreWindow> {
        self.cloned()
    }
}

impl Platform for X11Platform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::X11
    }

    fn poll_events(&mut self) -> PlatformResult<Vec<Event>> {
        self.conn.flush().map_err(err)?;
        let mut out = Vec::new();
        let first = self.conn.wait_for_event().map_err(err)?;
        if let Some(e) = self.handle_event(first)? {
            out.push(e);
        }
        while let Some(ev) = self.conn.poll_for_event().map_err(err)? {
            if let Some(e) = self.handle_event(ev)? {
                out.push(e);
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
        self.conn
            .configure_window(
                frame_id,
                &ConfigureWindowAux::new().x(geometry.x).y(geometry.y).width(geometry.width).height(geometry.height),
            )
            .map_err(err)?;
        self.conn
            .configure_window(
                client_id,
                &ConfigureWindowAux::new().x(0).y(TITLEBAR_HEIGHT as i32).width(geometry.width).height(geometry.height.saturating_sub(TITLEBAR_HEIGHT)),
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
            self.conn.set_input_focus(x11rb::protocol::xproto::InputFocus::POINTER_ROOT, frame.client, x11rb::CURRENT_TIME).map_err(err)?;
            self.conn.change_property32(x11rb::protocol::xproto::PropMode::REPLACE, self.root, self.atoms._NET_ACTIVE_WINDOW, x11rb::protocol::xproto::AtomEnum::WINDOW, &[frame.client]).map_err(err)?;
            self.conn.flush().map_err(err)?;
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
        Ok(())
    }

    fn set_border_width(&mut self, window: WindowId, width: u32) -> PlatformResult<()> {
        if let Some(w) = self.wm.borrow_mut().window_mut(window) {
            w.border_width = width;
        }
        Ok(())
    }

    fn redraw_decoration(&mut self, window: WindowId, win: &CoreWindow, focused: bool) -> PlatformResult<()> {
        if !win.decorated {
            return Ok(());
        }
        let Some(frame) = self.frame_for(window) else { return Ok(()) };
        let (bg, fg) = if focused { (0x2e3440u32, 0x88c0d0u32) } else { (0x2e3440u32, 0x4c566a) };

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
}
