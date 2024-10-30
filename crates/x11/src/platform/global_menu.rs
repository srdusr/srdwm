//! Global-menu support for the native X11 backend - parity with the
//! Wayland backend's XWayland integration (`crates/wayland/src/xwayland.rs`
//! and `srdwm_platform::appmenu_registrar`), reading the identical X11
//! properties and running the identical `com.canonical.AppMenu.Registrar`
//! D-Bus service, since a client-side toolkit (GTK/Qt) exports its menu the
//! same way regardless of which X server it's actually talking to.

use super::*;

impl X11Platform {
    /// Reads `xid`'s global-menu D-Bus address straight off its own X11
    /// properties - `_GTK_UNIQUE_BUS_NAME` plus whichever menu-path atom
    /// the client actually set. Exact mirror of `crates/wayland/src/
    /// xwayland.rs::EwmhState::read_global_menu`; see that method's doc
    /// comment for the full reasoning (menubar wins over app-menu, the
    /// `appmenu-gtk-module` Unity-shim case `classify_menu_source` exists
    /// for). No bus name means no menu at all, so this returns `None`
    /// rather than a `GlobalMenu` with an empty `bus_name`.
    pub(super) fn read_global_menu(&self, xid: XWindow) -> Option<srdwm_core::GlobalMenu> {
        let read_string = |atom: u32| -> Option<String> {
            let reply = self.conn.get_property(false, xid, atom, x11rb::protocol::xproto::AtomEnum::ANY, 0, u32::MAX).ok()?.reply().ok()?;
            if reply.value.is_empty() {
                return None;
            }
            String::from_utf8(reply.value).ok().filter(|s| !s.is_empty())
        };

        // Checked before anything GTK-atom-related - see `xwayland.rs`'s
        // identical check in its own `read_global_menu` for why: these two
        // are already a complete address on their own, and a Qt app under
        // a KDE Plasma session never sets `_GTK_UNIQUE_BUS_NAME` at all.
        if let (Some(bus_name), Some(menu_path)) = (read_string(self.atoms._KDE_NET_WM_APPMENU_SERVICE_NAME), read_string(self.atoms._KDE_NET_WM_APPMENU_OBJECT_PATH)) {
            return Some(srdwm_core::GlobalMenu { bus_name, menu_path: Some(menu_path), app_path: None, window_path: None, source: srdwm_core::MenuSource::DbusMenu });
        }

        let bus_name = read_string(self.atoms._GTK_UNIQUE_BUS_NAME)?;
        let app_path = read_string(self.atoms._GTK_APPLICATION_OBJECT_PATH);
        let window_path = read_string(self.atoms._GTK_WINDOW_OBJECT_PATH);
        let is_real_gtk_application = app_path.is_some() || window_path.is_some();
        let gtk_menu_path = read_string(self.atoms._GTK_MENUBAR_OBJECT_PATH).or_else(|| read_string(self.atoms._GTK_APP_MENU_OBJECT_PATH));
        let unity_path = read_string(self.atoms._UNITY_OBJECT_PATH);
        let (menu_path, source) = srdwm_core::classify_menu_source(gtk_menu_path, is_real_gtk_application, unity_path);
        Some(srdwm_core::GlobalMenu { bus_name, menu_path, app_path, window_path, source })
    }

    /// Refreshes the focused window's `global_menu` from its own X11
    /// properties - call on every real focus change (`Platform::focus`,
    /// the single chokepoint every focus path already goes through, same
    /// role `update_net_active_window` plays for the Wayland backend).
    /// Global-menu properties are usually set once, shortly after a client
    /// registers on the session bus, which can race a window's own initial
    /// map - reading only at map time would miss a client that finished
    /// registering a moment later, so this re-reads on every focus instead.
    /// `read_global_menu` returning `None` (the common case for anything
    /// non-GTK, or a GTK app with no menu to export) correctly clears a
    /// stale value left over from whichever window was focused before.
    pub(super) fn refresh_focused_global_menu(&mut self, id: WindowId, client: XWindow) {
        let menu = self.read_global_menu(client);
        if let Some(w) = self.wm.borrow_mut().window_mut(id) {
            w.global_menu = menu;
        }
    }

    /// Drains `AppmenuRegistrarState`'s channel and applies every event to
    /// the matching `Window.global_menu` - call once per `poll_events`
    /// tick, same as the Wayland backend's `xwayland.rs::apply_registrar_
    /// events`. `xid_to_core` already maps XID straight to `WindowId` here
    /// (unlike the Wayland backend, which has to scan for the matching
    /// `X11Surface`), so this needs no extra lookup structure of its own.
    pub(super) fn apply_registrar_events(&mut self) {
        let Some(registrar) = &self.appmenu_registrar else { return };
        let events = registrar.drain_events();
        if events.is_empty() {
            return;
        }
        for event in events {
            let (window_id, menu) = match event {
                srdwm_platform::RegistrarEvent::Registered { window_id, bus_name, menu_path } => (
                    window_id,
                    Some(srdwm_core::GlobalMenu { bus_name, menu_path: Some(menu_path), app_path: None, window_path: None, source: srdwm_core::MenuSource::DbusMenu }),
                ),
                srdwm_platform::RegistrarEvent::Unregistered { window_id } => (window_id, None),
            };
            let Some(&id) = self.xid_to_core.get(&window_id) else { continue };
            if let Some(w) = self.wm.borrow_mut().window_mut(id) {
                w.global_menu = menu;
            }
        }
    }
}
