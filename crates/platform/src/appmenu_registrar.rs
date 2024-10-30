//! `com.canonical.AppMenu.Registrar`: the classic Unity/`appmenu-qt5`
//! global-menu registration service. Unlike every other menu source srdwm
//! reads (the Wayland backend's `xwayland.rs`'s `_GTK_*`/
//! `_UNITY_OBJECT_PATH` X11 properties and `gtk_shell.rs`'s native-Wayland
//! `gtk_surface1.set_dbus_properties`), a classic Qt app using
//! `appmenu-qt5`'s original model never puts its menu's D-Bus address on an
//! X11 property at all - it calls `RegisterWindow(windowId, menuObjectPath)`
//! on this well-known D-Bus name instead, with the *sender* of that call
//! being the only way to learn which bus name the menu actually lives on.
//! Reading properties can't discover this; something has to actually own
//! this name on the session bus and receive the call, which is what this
//! module does.
//!
//! Lives in `srdwm-platform`, not `srdwm-wayland`, even though `windowId`
//! is always a raw X11 XID (this model predates Wayland entirely): both the
//! Wayland backend's XWayland integration *and* the native X11 backend
//! (`srdwm-x11`) need the exact same service against the exact same kind of
//! id, and neither depends on the other - `srdwm-platform` is the one
//! crate both already depend on. Each backend constructs its own
//! `AppmenuRegistrarState` (only ever one should actually be running at a
//! time, since only one backend runs per session) and resolves each
//! `RegistrarEvent`'s `window_id` back to its own `WindowId` however it
//! already maps X11 ids - the Wayland backend's `xwayland.rs::apply_
//! registrar_events` matches `X11Surface::window_id()` against `id_to_
//! window`, the X11 backend's `xid_to_core` map directly.
//!
//! This compositor has no async runtime anywhere else (see `srdwm/src/
//! main.rs`'s plain synchronous loop), and zbus's `blocking` API exists
//! specifically so this doesn't need one: `zbus::blocking::Connection`
//! still runs its socket-reading/dispatch on a background thread
//! internally (via `async-io`'s own global executor, which spawns that
//! thread the first time any work is submitted to it - nothing here has
//! to drive that explicitly), so holding the built `Connection` alive is
//! the only thing `AppmenuRegistrarState` needs to do to keep the service
//! running. The interface impl below only ever sends a plain event over a
//! `std::sync::mpsc` channel - draining it is a non-blocking, synchronous
//! `try_iter()` call from the compositor's own event-loop tick, the same
//! "background thread feeds a channel, the main loop drains it" shape
//! `IpcServer` already uses for the control socket.

use std::sync::mpsc::{Receiver, Sender};

use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;

/// One `RegisterWindow`/`UnregisterWindow` call, captured off the D-Bus
/// message and handed to a backend's own event-loop tick to resolve
/// against a real `WindowId` and update `Window.global_menu`.
#[derive(Debug)]
pub enum RegistrarEvent {
    Registered { window_id: u32, bus_name: String, menu_path: String },
    Unregistered { window_id: u32 },
}

/// The D-Bus interface itself - deliberately does nothing but forward
/// each call onto `tx`. Resolving `window_id` against `WindowManager` has
/// to happen back on the compositor's own thread (everything `WindowManager`
/// touches is `Rc`/`RefCell`, not `Send`), so there is nothing more useful
/// this could do while actually running on zbus's background thread.
struct RegistrarIface {
    tx: Sender<RegistrarEvent>,
}

#[zbus::interface(name = "com.canonical.AppMenu.Registrar")]
impl RegistrarIface {
    /// The registrar's own defined method: a window announcing (or
    /// re-announcing - a client is free to call this again if its menu
    /// path changes) its `com.canonical.dbusmenu` object. `#[zbus(header)]`
    /// is what makes the caller's bus name visible at all - it's the
    /// message envelope, not a request argument, so there is no other way
    /// to learn it.
    fn register_window(&mut self, window_id: u32, menu_object_path: OwnedObjectPath, #[zbus(header)] header: Header<'_>) {
        let Some(bus_name) = header.sender() else { return };
        let _ = self.tx.send(RegistrarEvent::Registered { window_id, bus_name: bus_name.to_string(), menu_path: menu_object_path.to_string() });
    }

    /// The protocol's own way of saying "no menu after all" - a window
    /// closing its menu, or closing entirely, without necessarily also
    /// destroying the D-Bus connection that registered it. Matches
    /// `xwayland.rs`/`gtk_shell.rs`'s own handling of the equivalent case:
    /// clear `Window.global_menu` rather than leaving it stale.
    fn unregister_window(&mut self, window_id: u32) {
        let _ = self.tx.send(RegistrarEvent::Unregistered { window_id });
    }
}

/// Owns the registrar's D-Bus connection for as long as an X11-capable
/// backend (XWayland under the Wayland backend, or the native X11 backend)
/// is running. `None` if starting the service failed (another process
/// already owns the name, or no session bus is reachable at all) - a
/// classic Qt app's global menu simply won't be seen in that case, same
/// "degrade, don't crash the compositor over a cosmetic feature" handling
/// as every other optional protocol here.
pub struct AppmenuRegistrarState {
    // Never read again after construction - its only job is to outlive
    // the compositor so the background-thread service it owns keeps
    // running. `Option` because starting it can fail (see above).
    _connection: Option<zbus::blocking::Connection>,
    rx: Receiver<RegistrarEvent>,
}

impl AppmenuRegistrarState {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let iface = RegistrarIface { tx };
        // `replace_existing_names`: a previous srdwm process that crashed
        // or was killed (rather than exiting cleanly) can leave this name
        // owned until the bus itself notices the connection is gone, which
        // is not guaranteed to have happened yet by the time a fresh
        // process starts - same reasoning as `IpcServer::bind_in` removing
        // a stale control-socket file before binding its own fresh one, so
        // a restart doesn't leave the *new* process's registrar silently
        // unreachable behind a name the old one still holds.
        let connection = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.serve_at("/com/canonical/AppMenu/Registrar", iface))
            .and_then(|b| b.name("com.canonical.AppMenu.Registrar"))
            .map(|b| b.replace_existing_names(true))
            .and_then(|b| b.build());
        let _connection = match connection {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("appmenu_registrar: couldn't start com.canonical.AppMenu.Registrar ({e}); classic Qt/appmenu-qt5 global menus won't be seen");
                None
            }
        };
        Self { _connection, rx }
    }

    /// Every registration/unregistration that arrived since the last call,
    /// non-blockingly - called once per event-loop tick from `poll_events`,
    /// the same drain-a-channel shape `IpcServer::poll` already uses for
    /// the control socket.
    pub fn drain_events(&self) -> Vec<RegistrarEvent> {
        self.rx.try_iter().collect()
    }
}

impl Default for AppmenuRegistrarState {
    fn default() -> Self {
        Self::new()
    }
}
