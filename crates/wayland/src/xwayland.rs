//! XWayland integration: lets legacy X11-only clients (anything that can't
//! speak the Wayland protocol natively) run inside the Wayland session,
//! bridged into the same `srdwm_core::WindowManager`/`Space` pipeline as
//! native `xdg-shell` windows.
//!
//! Only wired up for the udev/DRM backend (`udev`) for now: XWayland's
//! window-manager side (`X11Wm::start_wm`) is driven entirely through a
//! `calloop` event loop, which only the udev backend has - the nested
//! winit backend still drives its own manual poll loop (see `lib.rs`'s
//! module docs). Adding a second, XWayland-only `calloop::EventLoop` to the
//! winit backend too is possible but left as a follow-up.
//!
//! Scope: regular (server-managed) windows go through the exact same
//! `WindowManager::add_window`/decoration/hit-test path as xdg-shell
//! windows - an X11 app gets tiled, placed by `SmartPlacement`, and
//! decorated with our drawn titlebar exactly like a native Wayland client.
//! Override-redirect windows (menus, tooltips, drag images) are
//! deliberately *not* run through `WindowManager` at all - matching real
//! ICCCM semantics, no WM is ever supposed to manage or decorate them --
//! they're mapped into `Space` at whatever geometry the client itself
//! requests. Selections/clipboard, XSETTINGS, and RandR primary-output
//! sync are not implemented (all have harmless no-op default trait
//! methods in `XwmHandler`).

use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge as X11ResizeEdge, WmWindowProperty, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};
use smithay::{delegate_xwayland_shell, desktop::Window as DWindow};

use srdwm_core::{Event as CoreEvent, ResizeEdge, Window as CoreWindow, TITLEBAR_HEIGHT};

use crate::state::CompState;

pub(crate) type X11Window = smithay::xwayland::xwm::X11Window;

/// Spawns XWayland and registers the calloop sources that drive it: the
/// `XWayland` process/readiness source, and (once ready) `X11Wm`'s own
/// internal X11-connection source. Both are owned by the event loop after
/// `insert_source`, not by any struct here - dropping the loop (or the
/// `X11Wm` on disconnect) is what shuts things down.
///
/// Before spawning, arranges for XWayland to run with `-shm`: this
/// compositor only ever supports `wl_shm` (see `udev/mod.rs`'s module docs on
/// why it's deliberately software-only, no GBM/DMA-BUF), and XWayland's
/// default behavior of trying `glamor` first and falling back to
/// shared-memory buffers on failure does *not* fall back to the
/// `xwayland_shell_v1` protocol for associating X11 windows with
/// `wl_surface`s - confirmed by tracing the actual Wayland protocol
/// exchange with `WAYLAND_DEBUG=1`. Starting with `-shm` from the outset
/// avoids the failed glamor attempt entirely, which keeps XWayland on the
/// code path that does use `xwayland_shell_v1` correctly.
///
/// `smithay::xwayland::XWayland::spawn` builds its `Xwayland` command line
/// internally with a fixed argument list (no way to add `-shm` directly),
/// and can't be bypassed either: the `XWaylandClientData` type it inserts
/// as the spawned client's data has private fields, so nothing outside
/// smithay can construct one, and `X11Wm`/the internal surface-association
/// commit hook both depend on the client's data specifically being that
/// type. Instead, a tiny wrapper script shadows `Xwayland` on `PATH`
/// (`Command::new("Xwayland")`'s lookup honors the `PATH` smithay copies
/// from this process's own environment) and always re-execs the real
/// binary with `-shm` prepended.
pub(crate) fn spawn(handle: &LoopHandle<'static, CompState>, display_handle: &smithay::reexports::wayland_server::DisplayHandle) -> std::io::Result<()> {
    if let Err(e) = ensure_shm_wrapper_on_path() {
        log::warn!("could not set up an -shm wrapper for XWayland ({e}); XWayland windows will likely fail to render - see xwayland.rs's `spawn` docs");
    }

    let (xwayland, client) = XWayland::spawn(display_handle, None, std::iter::empty::<(String, String)>(), true, std::process::Stdio::null(), std::process::Stdio::null(), |_| ())?;

    let handle_for_ready = handle.clone();
    handle
        .insert_source(xwayland, move |event, _, data: &mut CompState| match event {
            XWaylandEvent::Ready { x11_socket, display_number } => {
                log::info!("XWayland ready on display :{display_number}");
                match X11Wm::start_wm(handle_for_ready.clone(), x11_socket, client.clone()) {
                    Ok(wm) => {
                        data.xwm = Some(wm);
                        fix_wm_name(display_number);
                        data.ewmh = EwmhState::connect(display_number);
                    }
                    Err(e) => log::error!("failed to start X11 window manager for XWayland: {e}"),
                }
            }
            XWaylandEvent::Error => log::error!("XWayland exited unexpectedly during startup"),
        })
        .map_err(|e| std::io::Error::other(format!("failed to register XWayland source: {e}")))?;
    Ok(())
}

/// Overwrites `_NET_WM_NAME` on XWayland's WM-check window from "Smithay X
/// WM" to "srdwm".
///
/// `X11Wm::start_wm` hardcodes that string with no override hook exposed --
/// no public method on `X11Wm`, and `wm_window`/its connection are private
/// fields, so it can't be reached through smithay's API at all. Every X11
/// client that asks "who is the window manager" (`xprop`, `wmctrl`,
/// `xdotool`, fetch tools, app-compat shims that branch on WM identity)
/// gets told the name of the *library*, not the compositor - which is
/// actively misleading, not just cosmetic: it broke a shell function that
/// resolved the WM's process name from this exact property to kill it on
/// logout, `pkill`ing "Smithay" and matching nothing.
///
/// Worked around by opening a second, independent X11 connection of our
/// own to the same XWayland display - exactly what `xprop` itself would
/// do - and rewriting the property directly. `_NET_SUPPORTING_WM_CHECK`
/// (which `start_wm` does set correctly) is how a plain client is meant to
/// find the WM-check window in the first place, so reading it back off the
/// root window rather than assuming a window ID keeps this from silently
/// going stale if smithay ever changes how it allocates that window.
fn fix_wm_name(display_number: u32) {
    use smithay::reexports::x11rb::connection::Connection;
    use smithay::reexports::x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use smithay::reexports::x11rb::rust_connection::RustConnection;
    use smithay::reexports::x11rb::wrapper::ConnectionExt as _;

    let display = format!(":{display_number}");
    let (conn, screen_num) = match RustConnection::connect(Some(&display)) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("xwayland: couldn't open a second connection to fix _NET_WM_NAME: {e}");
            return;
        }
    };
    let root = conn.setup().roots[screen_num].root;

    let intern = |name: &str| -> Option<u32> { conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom) };
    let (Some(supporting_wm_check), Some(net_wm_name), Some(utf8_string)) =
        (intern("_NET_SUPPORTING_WM_CHECK"), intern("_NET_WM_NAME"), intern("UTF8_STRING"))
    else {
        log::warn!("xwayland: couldn't intern EWMH atoms to fix _NET_WM_NAME");
        return;
    };

    let reply = conn.get_property(false, root, supporting_wm_check, AtomEnum::WINDOW, 0, 1).ok().and_then(|c| c.reply().ok());
    let wm_window = reply.as_ref().and_then(|r| r.value32()).and_then(|mut it| it.next());
    let Some(wm_window) = wm_window else {
        log::warn!("xwayland: _NET_SUPPORTING_WM_CHECK unset on the XWayland root; can't fix _NET_WM_NAME");
        return;
    };

    if let Err(e) = conn.change_property8(PropMode::REPLACE, wm_window, net_wm_name, utf8_string, b"srdwm") {
        log::warn!("xwayland: failed to set _NET_WM_NAME: {e}");
        return;
    }
    let _ = conn.flush();
}

/// Keeps `_NET_ACTIVE_WINDOW`/`_NET_CLIENT_LIST`/`_NET_CLIENT_LIST_STACKING`
/// on the XWayland root window up to date.
///
/// srdwm declares all three in `_NET_SUPPORTED` (smithay's `X11Wm` sets that
/// part up on its own), but never actually wrote them: `_NET_ACTIVE_WINDOW`
/// stayed `0x0` and `_NET_CLIENT_LIST` stayed empty regardless of what was
/// focused or mapped. Confirmed this is not something `X11Wm` does for us
/// automatically - it only updates `_NET_ACTIVE_WINDOW` in response to a
/// real X11 `FocusIn`/`FocusOut` event on the window, which requires an
/// actual `SetInputFocus` request to have been issued in the first place,
/// and nothing in this codebase ever issues one (Wayland keyboard focus and
/// X11 input focus are separate things; only the former was ever set). The
/// practical effect: any X11-aware client trying to answer "what's the
/// focused window" or "what windows exist" - `xdotool`, `wmctrl`, and
/// (per a downstream report) an AGS global-menu widget resolving the
/// focused window to query its DBusMenu registrar - got nothing.
///
/// Rather than depend on `X11Wm`'s `FocusIn`-triggered path (which would
/// also need us to issue real `SetInputFocus` requests, itself a bigger
/// change with its own risk of fighting Wayland focus), this writes both
/// properties directly, from srdwm's own already-authoritative focus and
/// window-list state - exactly the "no new protocol needed" shape a
/// real EWMH-maintaining WM uses. Same reasoning as `fix_wm_name` for using
/// a second, independent connection rather than reaching into `X11Wm`'s
/// private one: there is no public accessor for it.
pub(crate) struct EwmhState {
    conn: smithay::reexports::x11rb::rust_connection::RustConnection,
    root: u32,
    net_active_window: u32,
    net_client_list: u32,
    net_client_list_stacking: u32,
    /// Global-menu atoms - see `read_global_menu`. Individually optional
    /// (unlike the EWMH atoms above): a server old enough, or configured
    /// oddly enough, to not know these names is still a fully functional
    /// X server for everything else this module does, so a failure to
    /// intern any one of them just means that field never resolves rather
    /// than aborting `connect` entirely.
    gtk_unique_bus_name: Option<u32>,
    gtk_application_object_path: Option<u32>,
    gtk_window_object_path: Option<u32>,
    gtk_menubar_object_path: Option<u32>,
    gtk_app_menu_object_path: Option<u32>,
    unity_object_path: Option<u32>,
}

impl EwmhState {
    fn connect(display_number: u32) -> Option<Self> {
        use smithay::reexports::x11rb::connection::Connection;
        use smithay::reexports::x11rb::protocol::xproto::ConnectionExt as _;
        use smithay::reexports::x11rb::rust_connection::RustConnection;

        let display = format!(":{display_number}");
        let (conn, screen_num) = match RustConnection::connect(Some(&display)) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("xwayland: couldn't open a connection for EWMH property updates: {e}");
                return None;
            }
        };
        let root = conn.setup().roots[screen_num].root;
        let intern = |name: &str| -> Option<u32> { conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom) };
        let (Some(net_active_window), Some(net_client_list), Some(net_client_list_stacking)) =
            (intern("_NET_ACTIVE_WINDOW"), intern("_NET_CLIENT_LIST"), intern("_NET_CLIENT_LIST_STACKING"))
        else {
            log::warn!("xwayland: couldn't intern EWMH atoms; _NET_ACTIVE_WINDOW/_NET_CLIENT_LIST won't be maintained");
            return None;
        };
        let gtk_unique_bus_name = intern("_GTK_UNIQUE_BUS_NAME");
        let gtk_application_object_path = intern("_GTK_APPLICATION_OBJECT_PATH");
        let gtk_window_object_path = intern("_GTK_WINDOW_OBJECT_PATH");
        let gtk_menubar_object_path = intern("_GTK_MENUBAR_OBJECT_PATH");
        let gtk_app_menu_object_path = intern("_GTK_APP_MENU_OBJECT_PATH");
        let unity_object_path = intern("_UNITY_OBJECT_PATH");
        let state = Self {
            conn,
            root,
            net_active_window,
            net_client_list,
            net_client_list_stacking,
            gtk_unique_bus_name,
            gtk_application_object_path,
            gtk_window_object_path,
            gtk_menubar_object_path,
            gtk_app_menu_object_path,
            unity_object_path,
        };
        // `_NET_CLIENT_LIST`/`_STACKING` are properties on the X root window,
        // which XWayland recreates fresh on every launch - but nothing
        // guarantees a *client* reading them does so only after this
        // compositor's own first `update_net_client_list()` call, and until
        // that first real add/remove there is no guarantee the property even
        // has a defined initial value. Clearing it here, before any window
        // has ever mapped, means a freshly connected client can never read a
        // leftover or undefined list - it always starts empty and correct.
        state.set_client_list(&[]);
        Some(state)
    }

    /// Reads `xid`'s global-menu D-Bus address straight off its own X11
    /// properties - `_GTK_UNIQUE_BUS_NAME` plus whichever menu-path atom
    /// the client actually set. `_GTK_MENUBAR_OBJECT_PATH` (a real menu
    /// bar) wins over `_GTK_APP_MENU_OBJECT_PATH` (the single-item
    /// fallback simpler/older clients export) if a client somehow sets
    /// both; `_UNITY_OBJECT_PATH` is the pre-`_GTK_*` name some
    /// still-relevant toolkits (older Qt builds with the appmenu-qt5
    /// platform theme) use instead, tried last. No bus name means no menu
    /// at all - the paths are meaningless without it - so this returns
    /// `None` rather than a `GlobalMenu` with an empty `bus_name`.
    ///
    /// Which atom actually won is recorded as `source` - a consumer needs
    /// it to pick the right D-Bus action-group prefix (`app`/`win` for a
    /// real `GMenuModel`, `unity` for the older export), and
    /// `appmenu-gtk-module` is known to set the `_GTK_*` atoms *and*
    /// `_UNITY_OBJECT_PATH` simultaneously in some configurations - a
    /// consumer with only the resolved path string, and no record of
    /// which one it came from, can't tell the two cases apart even though
    /// picking the wrong prefix means every menu item renders permanently
    /// insensitive (a silent failure that reads exactly like a broken
    /// app, not a wiring bug). Reported by the AGS peer session building
    /// the consumer, from hitting exactly this live.
    fn read_global_menu(&self, xid: u32) -> Option<srdwm_core::GlobalMenu> {
        use smithay::reexports::x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

        let read_string = |atom: Option<u32>| -> Option<String> {
            let atom = atom?;
            let reply = self.conn.get_property(false, xid, atom, AtomEnum::ANY, 0, u32::MAX).ok()?.reply().ok()?;
            if reply.value.is_empty() {
                return None;
            }
            String::from_utf8(reply.value).ok().filter(|s| !s.is_empty())
        };

        let bus_name = read_string(self.gtk_unique_bus_name)?;
        let gtk_menu_path = read_string(self.gtk_menubar_object_path).or_else(|| read_string(self.gtk_app_menu_object_path));
        let (menu_path, source) = match gtk_menu_path {
            Some(path) => (Some(path), srdwm_core::MenuSource::Gtk),
            None => match read_string(self.unity_object_path) {
                Some(path) => (Some(path), srdwm_core::MenuSource::Unity),
                None => (None, srdwm_core::MenuSource::Gtk),
            },
        };
        let app_path = read_string(self.gtk_application_object_path);
        let window_path = read_string(self.gtk_window_object_path);
        Some(srdwm_core::GlobalMenu { bus_name, menu_path, app_path, window_path, source })
    }

    /// `xid` is `None` when focus is on a native Wayland window (or
    /// nothing) rather than an X11 one - `_NET_ACTIVE_WINDOW`'s value is
    /// only meaningful for X11 clients, so this writes `0` (the documented
    /// "no active window" sentinel) rather than leaving the last X11
    /// window's id stale and misleading.
    fn set_active_window(&self, xid: Option<u32>) {
        use smithay::reexports::x11rb::connection::Connection;
        use smithay::reexports::x11rb::protocol::xproto::{AtomEnum, PropMode};
        use smithay::reexports::x11rb::wrapper::ConnectionExt as _;
        if let Err(e) = self.conn.change_property32(PropMode::REPLACE, self.root, self.net_active_window, AtomEnum::WINDOW, &[xid.unwrap_or(0)]) {
            log::warn!("xwayland: failed to set _NET_ACTIVE_WINDOW: {e}");
            return;
        }
        let _ = self.conn.flush();
    }

    fn set_client_list(&self, xids: &[u32]) {
        use smithay::reexports::x11rb::connection::Connection;
        use smithay::reexports::x11rb::protocol::xproto::{AtomEnum, PropMode};
        use smithay::reexports::x11rb::wrapper::ConnectionExt as _;
        // Same order for both: EWMH only defines a strict order for the
        // `_STACKING` variant (bottom-to-top), and `stacking_order` is
        // already srdwm's one authoritative ordering of its windows - a
        // second, differently-ordered list for plain `_NET_CLIENT_LIST`
        // would need tracking mapping order separately for no real benefit.
        for (atom, name) in [(self.net_client_list, "_NET_CLIENT_LIST"), (self.net_client_list_stacking, "_NET_CLIENT_LIST_STACKING")] {
            if let Err(e) = self.conn.change_property32(PropMode::REPLACE, self.root, atom, AtomEnum::WINDOW, xids) {
                log::warn!("xwayland: failed to set {name}: {e}");
                return;
            }
        }
        let _ = self.conn.flush();
    }
}

impl CompState {
    /// Call on every focus change (from `set_keyboard_focus`, the single
    /// chokepoint every focus path already goes through). `surface` is
    /// whatever just gained keyboard focus; resolves to an X11 window id
    /// only if that surface's window is XWayland-backed.
    pub(crate) fn update_net_active_window(&self, surface: Option<&smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>) {
        let Some(ewmh) = &self.ewmh else { return };
        let id = surface.and_then(|s| self.surface_to_id.get(s)).copied();
        let xid = id.and_then(|id| self.id_to_window.get(&id)).and_then(|w| w.x11_surface()).map(|x| x.window_id());
        ewmh.set_active_window(xid);
        // Global-menu properties are usually set once, shortly after a
        // client registers on the session bus - which can race a window's
        // own initial map, so reading them only at map time would miss a
        // client that finished that registration a moment later. Refreshed
        // here instead: every real focus change is a natural, already-
        // existing hook, and a menu only actually needs to be current for
        // whichever window is focused right now anyway. `read_global_menu`
        // returning `None` (the common case for anything non-GTK, or a
        // GTK app with no menu to export) correctly clears a stale value
        // from a previous window that used to occupy this `id`.
        if let (Some(id), Some(xid)) = (id, xid) {
            let menu = ewmh.read_global_menu(xid);
            if let Some(w) = self.wm.borrow_mut().window_mut(id) {
                w.global_menu = menu;
            }
        }
    }

    /// Call whenever the set of mapped windows changes (X11 window map,
    /// unmap, or destroy - see the `XwmHandler` methods below).
    pub(crate) fn update_net_client_list(&self) {
        let Some(ewmh) = &self.ewmh else { return };
        let xids: Vec<u32> = self
            .wm
            .borrow()
            .stacking_order()
            .filter_map(|w| self.id_to_window.get(&w.id))
            .filter_map(|w| w.x11_surface())
            .map(|x| x.window_id())
            .collect();
        ewmh.set_client_list(&xids);
    }
}

/// Writes a small shell script named `Xwayland` to a private directory and
/// prepends that directory to this process's own `PATH` - the next
/// `Command::new("Xwayland")` (namely `XWayland::spawn`'s, which copies
/// `PATH` from this process's environment into the child's) resolves to
/// the wrapper instead of the real binary. The wrapper always re-execs the
/// real `Xwayland` with `-shm` prepended to whatever arguments it was
/// given, so it's transparent to everything else `spawn` sets up.
fn ensure_shm_wrapper_on_path() -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let real_xwayland = find_on_path("Xwayland").ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Xwayland not found on PATH"))?;

    let wrapper_dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("srdwm-xwayland-shm-wrapper");
    std::fs::create_dir_all(&wrapper_dir)?;

    let wrapper_path = wrapper_dir.join("Xwayland");
    let quoted = shell_single_quote(&real_xwayland.to_string_lossy());
    std::fs::write(&wrapper_path, format!("#!/bin/sh\nexec {quoted} -shm \"$@\"\n"))?;
    let mut perms = std::fs::metadata(&wrapper_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&wrapper_path, perms)?;

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut new_path = wrapper_dir.into_os_string();
    new_path.push(":");
    new_path.push(old_path);
    // SAFETY: called once, synchronously, before any XWayland process (or
    // any other thread) is spawned.
    unsafe { std::env::set_var("PATH", new_path) };
    Ok(())
}

fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).map(|dir| dir.join(name)).find(|candidate| candidate.is_file())
}

/// POSIX single-quoting: safe for any byte sequence, including embedded
/// single quotes (`'` -> `'\''`).
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_single_quote_handles_embedded_quotes() {
        assert_eq!(shell_single_quote("/usr/bin/Xwayland"), "'/usr/bin/Xwayland'");
        assert_eq!(shell_single_quote("/it's/here"), r"'/it'\''s/here'");
    }
}

fn to_core_resize_edge(edge: X11ResizeEdge) -> ResizeEdge {
    match edge {
        X11ResizeEdge::Top => ResizeEdge::Top,
        X11ResizeEdge::Bottom => ResizeEdge::Bottom,
        X11ResizeEdge::Left => ResizeEdge::Left,
        X11ResizeEdge::Right => ResizeEdge::Right,
        X11ResizeEdge::TopLeft => ResizeEdge::TopLeft,
        X11ResizeEdge::TopRight => ResizeEdge::TopRight,
        X11ResizeEdge::BottomLeft => ResizeEdge::BottomLeft,
        X11ResizeEdge::BottomRight => ResizeEdge::BottomRight,
    }
}

impl CompState {
    /// Retries `finish_x11_window_setup` for every mapped X11 window still
    /// waiting on its `wl_surface` association - called on every
    /// compositor commit, since that association can complete without ever
    /// invoking `surface_associated` (see the module docs).
    pub(crate) fn retry_pending_x11_windows(&mut self) {
        if self.xwayland_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.xwayland_pending);
        for surface in pending {
            self.finish_x11_window_setup(&surface);
            let done = self.xwayland_windows.get(&surface.window_id()).is_some_and(|id| self.id_to_window.contains_key(id));
            if !done {
                self.xwayland_pending.push(surface);
            }
        }
    }

    /// Finishes setting up a *server-managed* (non-override-redirect) X11
    /// window once both halves are known: it's been granted its map
    /// request, and XWayland has associated it with a `wl_surface`. Safe to
    /// call from either order's callback; idempotent.
    fn finish_x11_window_setup(&mut self, surface: &X11Surface) {
        let Some(wl_surface) = surface.wl_surface() else {
            log::debug!("xwayland: finish_x11_window_setup xid={:?} - no wl_surface yet", surface.window_id());
            return;
        };
        let Some(&id) = self.xwayland_windows.get(&surface.window_id()) else {
            log::debug!("xwayland: finish_x11_window_setup xid={:?} - not in xwayland_windows", surface.window_id());
            return;
        };
        if self.id_to_window.contains_key(&id) {
            log::debug!("xwayland: finish_x11_window_setup xid={:?} id={id} - already set up", surface.window_id());
            return;
        }
        log::info!("xwayland: finishing setup for xid={:?} id={id}", surface.window_id());
        let geom = self.wm.borrow().window(id).map(|w| w.geometry).unwrap_or_default();

        let dwindow = DWindow::new_x11_window(surface.clone());
        let _ = surface.configure(Rectangle::new((geom.x, geom.y + TITLEBAR_HEIGHT as i32).into(), (geom.width as i32, (geom.height - TITLEBAR_HEIGHT) as i32).into()));

        self.space.map_element(dwindow.clone(), (geom.x, geom.y + TITLEBAR_HEIGHT as i32), true);
        self.surface_to_id.insert(wl_surface.clone(), id);
        self.id_to_window.insert(id, dwindow);
        self.redraw_decoration_buffer(id);
        // `WindowManager::add_window` already made this the focused window
        // in srdwm's own bookkeeping (it unconditionally does, for every
        // new window), but that's purely internal state - without this, a
        // freshly-opened XWayland app never receives a single keystroke
        // until it's clicked, and (found investigating a downstream EWMH
        // report) `_NET_ACTIVE_WINDOW` never updates either, since this is
        // `set_keyboard_focus`'s only caller for X11 windows and that's the
        // sole place `_NET_ACTIVE_WINDOW` gets written. The xdg-shell path
        // (`new_managed_window` in state/lifecycle.rs) already does this; this is the
        // equivalent X11 creation path, which never got the same fix.
        self.set_keyboard_focus(Some(wl_surface));
        self.pending.borrow_mut().push(CoreEvent::WindowCreated(id));
        self.update_net_client_list();
        crate::foreign_toplevel::window_created(self, id);
    }

    fn remove_x11_window(&mut self, xid: X11Window) {
        let Some(id) = self.xwayland_windows.get(&xid).copied() else { return };
        if let Some(w) = self.id_to_window.remove(&id) {
            self.space.unmap_elem(&w);
        }
        self.decorations.remove(&id);
        // Same reason as `state/lifecycle.rs`'s native `remove_window`: don't leave
        // the context menu open against a window that's about to stop
        // existing.
        if self.context_menu.as_ref().is_some_and(|m| m.window == id) {
            self.close_context_menu();
        }
        self.wm.borrow_mut().remove_window(id);
        self.pending.borrow_mut().push(CoreEvent::WindowDestroyed(id));
        crate::foreign_toplevel::window_closed(self, id);
        // Same reason as the equivalent call in `state/lifecycle.rs`'s native
        // `remove_window`: core may have already moved focus to whatever's
        // now on top, and the Wayland/X11 side needs to be told to follow.
        crate::input::sync_keyboard_focus(self);
        self.update_net_client_list();
    }
}

impl XWaylandShellHandler for CompState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm: XwmId, _wl_surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, surface: X11Surface) {
        log::debug!("xwayland: surface_associated xid={:?}", surface.window_id());
        self.finish_x11_window_setup(&surface);
    }
}

delegate_xwayland_shell!(CompState);

impl XwmHandler for CompState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("XwmHandler callback fired without an X11Wm")
    }

    fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Created but not (yet) mapped - nothing to do until a map request.
        log::debug!("xwayland: new_window xid={:?} title={:?}", window.window_id(), window.title());
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Not managed until it actually maps - see `mapped_override_redirect_window`.
        log::debug!("xwayland: new_override_redirect_window xid={:?}", window.window_id());
    }

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        log::debug!("xwayland: map_window_request xid={:?} title={:?} class={:?}", window.window_id(), window.title(), window.class());
        let id = {
            let mut wm = self.wm.borrow_mut();
            let id = wm.alloc_window_id();
            let mut w = CoreWindow::new(id, window.title());
            w.app_id = window.class();
            // Not `window.geometry()`: at `MapRequest` time this can still
            // be whatever tiny/default size the X11 window was *created*
            // with, before XWayland ever applies a `ConfigureRequest` --
            // and our own `configure_request` handler is deliberately a
            // no-op (we own layout for managed windows, matching
            // `new_managed_window`'s xdg-shell path below, which doesn't
            // trust the client's initial size either).
            w.geometry = srdwm_core::Rect::new(0, 0, 800, 600 + TITLEBAR_HEIGHT);
            wm.add_window(w);
            id
        };
        self.xwayland_windows.insert(window.window_id(), id);
        // Grant the map request *now*, unconditionally: per `X11Surface`'s
        // docs this is what tells XWayland the window may proceed, and it
        // does so before ever finishing our own wl_surface-dependent setup
        // (`finish_x11_window_setup` bails out until `wl_surface()`
        // resolves). Deferring `set_mapped` until after that check would
        // deadlock - XWayland doesn't seem to advance the window past
        // surface creation (no `get_xwayland_surface`/`set_serial`, no
        // buffer attach) until the map is granted.
        let _ = window.set_mapped(true);
        self.finish_x11_window_setup(&window);
        if !self.id_to_window.contains_key(&id) {
            self.xwayland_pending.push(window);
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(wl_surface) = window.wl_surface() else { return };
        let geom = window.geometry();
        let dwindow = DWindow::new_x11_window(window.clone());
        self.space.map_element(dwindow.clone(), (geom.loc.x, geom.loc.y), true);
        // Allocated only for the surface_to_id/id_to_window bookkeeping
        // `commit()` needs - deliberately never passed to
        // `WindowManager::add_window`: override-redirect windows are not
        // managed, per ICCCM.
        let id = self.wm.borrow_mut().alloc_window_id();
        self.xwayland_windows.insert(window.window_id(), id);
        self.surface_to_id.insert(wl_surface, id);
        self.id_to_window.insert(id, dwindow);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.remove_x11_window(window.window_id());
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let xid = window.window_id();
        self.remove_x11_window(xid);
        self.xwayland_windows.remove(&xid);
    }

    /// Re-reads title/class after either changes post-map and updates
    /// `Window`/foreign-toplevel listeners if either actually did - the
    /// XWayland equivalent of `sync_toplevel_metadata` (see its own doc
    /// comment for the identical xdg-shell-side problem this mirrors).
    ///
    /// `map_window_request` only ever reads `window.title()`/`.class()`
    /// once, at `MapRequest` - but for some real clients (confirmed live:
    /// Spotify, OpenSnitch's tray-prompt window) the *managed* X11 window
    /// never carries `WM_NAME`/`WM_CLASS` at all at that moment, or ever;
    /// the properties land on a separately-reparented child window instead,
    /// and XWayland's own X11Wm surfaces that as a `property_notify` on
    /// *this* window once it observes the change. Without handling it here,
    /// `Window.title`/`app_id` stay permanently empty for such a window --
    /// which reaches `srd.rule` (class matching), the compositor's own
    /// titlebar text, and every `wlr-foreign-toplevel-management` listener
    /// (a dock's running-indicator, an app switcher, icon lookup), not just
    /// this compositor's own UI.
    fn property_notify(&mut self, _xwm: XwmId, window: X11Surface, property: WmWindowProperty) {
        if !matches!(property, WmWindowProperty::Title | WmWindowProperty::Class) {
            return;
        }
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        let title = window.title();
        let app_id = window.class();
        let changed = {
            let mut wm = self.wm.borrow_mut();
            let Some(w) = wm.window_mut(id) else { return };
            let changed = w.title != title || w.app_id != app_id;
            w.title = title;
            w.app_id = app_id;
            changed
        };
        if changed {
            // Same reasoning as `sync_toplevel_metadata`: only a rule
            // actually matching for the first time warrants a decoration/
            // geometry refresh, since `sync_geometry` re-stacks the window
            // to the top of `Space` as an unconditional side effect of
            // `map_element` - calling it on every later title change would
            // silently yank an unrelated, unfocused window back to front.
            if self.wm.borrow_mut().reapply_rules_if_pending(id) {
                self.redraw_decoration_buffer(id);
                self.sync_geometry(id);
            }
            crate::foreign_toplevel::send_state(self, id);
        }
    }

    fn configure_request(&mut self, _xwm: XwmId, _window: X11Surface, _x: Option<i32>, _y: Option<i32>, _w: Option<u32>, _h: Option<u32>, _reorder: Option<Reorder>) {
        // We own layout for managed windows; smithay always sends back a
        // synthetic configure with the window's actual current geometry
        // after this callback returns (see `xwayland::xwm`'s `handle_event`
        // for `ConfigureRequest`), so there is nothing to do here - this
        // mirrors how `srdwm_x11::X11Platform` acks `ConfigureRequest` with
        // the client's real geometry rather than whatever it asked for.
    }

    fn configure_notify(&mut self, _xwm: XwmId, window: X11Surface, geometry: Rectangle<i32, Logical>, _above: Option<X11Window>) {
        // Only override-redirect windows are allowed to reposition
        // themselves at will; managed windows' geometry is owned by us.
        if !window.is_override_redirect() {
            return;
        }
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        if let Some(w) = self.id_to_window.get(&id) {
            self.space.map_element(w.clone(), (geometry.loc.x, geometry.loc.y), false);
        }
    }

    /// The same six requests found missing for native Wayland windows
    /// (`XdgShellHandler`'s `maximize_request`/`unmaximize_request`/
    /// `fullscreen_request`/`unfullscreen_request`/`minimize_request`,
    /// see `protocols.rs`) exist here too, under EWMH/ICCCM naming --
    /// `_NET_WM_STATE_MAXIMIZED_VERT`/`_HORZ`, `_NET_WM_STATE_FULLSCREEN`,
    /// `_NET_WM_STATE_HIDDEN` toggled via a client message - and were
    /// equally unimplemented, silently doing nothing for any XWayland
    /// app's own window-menu maximize/minimize/fullscreen action. `move_
    /// request`/`resize_request` right below were already implemented,
    /// which is what made this omission easy to miss; the drag/resize
    /// half of this class of gap already had parity, only the state-
    /// toggle half didn't. `unminimize_request` has no native-Wayland
    /// equivalent to mirror - xdg-shell has no client-initiated "restore
    /// from minimized" request at all, only EWMH does.
    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        if !self.wm.borrow().window(id).is_some_and(|w| w.maximized) {
            self.wm.borrow_mut().toggle_maximize(id);
            self.sync_geometry(id);
            crate::foreign_toplevel::send_state(self, id);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        if self.wm.borrow().window(id).is_some_and(|w| w.maximized) {
            self.wm.borrow_mut().toggle_maximize(id);
            self.sync_geometry(id);
            crate::foreign_toplevel::send_state(self, id);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        if !self.wm.borrow().is_fullscreen(id) {
            self.wm.borrow_mut().toggle_fullscreen(id);
            self.redraw_decoration_buffer(id);
            self.sync_geometry(id);
            crate::foreign_toplevel::send_state(self, id);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        if self.wm.borrow().is_fullscreen(id) {
            self.wm.borrow_mut().toggle_fullscreen(id);
            self.redraw_decoration_buffer(id);
            self.sync_geometry(id);
            crate::foreign_toplevel::send_state(self, id);
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        self.wm.borrow_mut().minimize_window(id);
        crate::foreign_toplevel::send_state(self, id);
    }

    fn unminimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        self.wm.borrow_mut().restore_window(id);
        crate::foreign_toplevel::send_state(self, id);
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, resize_edge: X11ResizeEdge) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        let pos = self.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default();
        self.wm.borrow_mut().start_resize(id, to_core_resize_edge(resize_edge), pos.x as i32, pos.y as i32);
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let Some(&id) = self.xwayland_windows.get(&window.window_id()) else { return };
        let pos = self.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default();
        self.wm.borrow_mut().start_drag(id, pos.x as i32, pos.y as i32);
    }
}
