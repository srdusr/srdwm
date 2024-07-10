//! A tiny local control socket, in the spirit of `hyprctl`/`swaymsg`, so
//! external scripts can query and drive window state without speaking
//! Wayland themselves. Bound at `$XDG_RUNTIME_DIR/srdwm-<display>.sock`.
//! `crates/ctl` (the `srd` binary) is the reference client.
//!
//! Deliberately synchronous and non-blocking-polled from each backend's
//! `poll_events()` tick (see `udev.rs`/`winit.rs`), the same way the
//! Wayland client socket itself is accepted - there is no calloop event
//! loop shared by both backends (`winit.rs` has none at all), so this
//! avoids needing two different registration mechanisms for one feature.
//! An ordinary request is one request/one response/close; nothing here is
//! held open for those, so a stalled or hostile client can only ever leak
//! one never-completed connection object, not block the compositor.
//!
//! `{"cmd":"subscribe"}` is the one exception: instead of closing after its
//! reply, that connection is kept open and pushed a fresh `clients` event
//! every time the window list actually changes, so a dock/panel doesn't
//! have to re-poll `clients` on a timer and diff it itself to notice
//! anything - the single highest-leverage gap found comparing srdwm
//! against sway/i3/Hyprland/bspwm's own IPCs, all of which have an
//! event-subscribe side already. A peer session building an AGS dock hit
//! exactly this wall (see `docs/IMPLEMENTATION_STATUS.md`): with no way to
//! be told about changes, it had to poll `wlr-foreign-toplevel` from a
//! separate Python helper instead of using this socket at all.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use serde::Serialize;

use srdwm_core::{GlobalMenu, MenuSource, WindowId, WindowManager};

pub struct IpcServer {
    listener: UnixListener,
    path: PathBuf,
    conns: Vec<(UnixStream, Vec<u8>)>,
    /// Long-lived connections from `{"cmd":"subscribe"}` - write-only after
    /// their initial snapshot, never read from again (a subscriber has no
    /// further requests to send; a client wanting both query and push needs
    /// two connections, matching Hyprland's separate event socket rather
    /// than sway's single multiplexed one, the simpler of the two to keep
    /// this connection loop's one-purpose-per-connection shape intact).
    subscribers: Vec<UnixStream>,
    /// What was last actually sent to subscribers, so a `poll()` tick with
    /// no real change (the common case, since this runs every ~16ms) skips
    /// serializing and writing anything at all.
    last_broadcast: Vec<ClientInfo>,
}

impl IpcServer {
    /// `display_name` is the Wayland socket name (e.g. `wayland-1`) so
    /// concurrent nested/test instances - used throughout this project for
    /// self-testing - don't collide on one path.
    pub fn bind(display_name: &str) -> std::io::Result<Self> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::bind_in(&dir, display_name)
    }

    /// `bind`'s actual logic, parametrized over the runtime directory --
    /// split out so tests can point this at a `tempfile::tempdir()` instead
    /// of mutating the process-wide `XDG_RUNTIME_DIR` env var (racy under
    /// Rust's default parallel test execution, since every test in this
    /// crate shares one process).
    fn bind_in(dir: &std::path::Path, display_name: &str) -> std::io::Result<Self> {
        let path = dir.join(format!("srdwm-{display_name}.sock"));
        // A stale socket left behind by a crashed/killed previous instance
        // makes `bind` fail with `AddrInUse` even though nothing is
        // listening; a fresh instance always wins over a dead one.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, path, conns: Vec::new(), subscribers: Vec::new(), last_broadcast: Vec::new() })
    }

    /// Accepts any waiting connections, advances in-progress reads, and
    /// pushes a fresh snapshot to every subscriber if the window list
    /// actually changed since the last one. Returns `true` if a request
    /// mutated window state, so the caller can fold that into its own
    /// dirty/`sync()` decision the same as any other event source.
    pub fn poll(&mut self, wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> bool {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    if stream.set_nonblocking(true).is_ok() {
                        self.conns.push((stream, Vec::new()));
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        let mut dirty = false;
        let mut new_subscribers = Vec::new();
        self.conns.retain_mut(|(stream, buf)| {
            let mut chunk = [0u8; 512];
            match stream.read(&mut chunk) {
                Ok(0) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(_) => return false,
            }
            let Some(nl) = buf.iter().position(|&b| b == b'\n') else {
                // Cap a request that never terminates - a hostile or
                // broken client shouldn't accumulate memory forever.
                return buf.len() < 4096;
            };
            let line = buf[..nl].to_vec();
            let cmd = serde_json::from_slice::<serde_json::Value>(&line).ok().and_then(|v| v.get("cmd").and_then(|c| c.as_str().map(str::to_string)));
            let (response, changed) = handle_request(&line, wm);
            dirty |= changed;
            let mut out = response;
            out.push(b'\n');
            if stream.write_all(&out).is_err() {
                return false;
            }
            if cmd.as_deref() == Some("subscribe") {
                // Handed off to `subscribers` below rather than kept here --
                // this connection is done being read from, only ever
                // written to from now on.
                if let Ok(cloned) = stream.try_clone() {
                    new_subscribers.push(cloned);
                }
                return false;
            }
            // Every other command is still one request/one response/close,
            // same as before subscribe existed.
            false
        });
        self.subscribers.extend(new_subscribers);

        if !self.subscribers.is_empty() {
            let current: Vec<ClientInfo> = client_snapshot(wm);
            if current != self.last_broadcast {
                if let Ok(mut out) = serde_json::to_vec(&ClientsEvent { event: "clients", clients: &current }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
                self.last_broadcast = current;
            }
        }
        dirty
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Serialize, Clone, PartialEq)]
struct ClientInfo {
    id: u64,
    app_id: String,
    title: String,
    workspace: usize,
    focused: bool,
    minimized: bool,
    visible: bool,
    scratchpad: bool,
    // Whether the layout placed this window (tiled) or the user positioned
    // it directly (floating) - added for an external panel's auto-hide
    // logic, which needs to tell a window the layout placed flush against
    // its own reserved edge (expected, not an overlap) from one the user
    // actually dragged into that space (a real overlap it should react
    // to). Geometry alone can't distinguish the two: a tiled window's edge
    // sitting exactly at the usable-area boundary looks identical, in x/y/
    // width/height terms, to a floating window a human dragged flush
    // against it.
    floating: bool,
    // Geometry, in the same global logical-pixel space everything else in
    // this compositor uses. Added for an external panel's Overview/window-
    // switcher, which has no other way to lay out window miniatures to
    // scale - neither `zwlr_foreign_toplevel_management_v1` nor
    // `ext_foreign_toplevel_list_v1` carries geometry at all, by design of
    // those protocols, so this compositor's own IPC is the only place it
    // can come from.
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    // The window's global-menu D-Bus address (bus name + object paths),
    // if it has exported one - `null` for the common case of a window
    // with no menu at all, which a consumer should treat exactly like a
    // missing field: no menu to show, not an error. See `srdwm_core::
    // GlobalMenu`'s own doc comment for why this is an address and never
    // the menu's actual content.
    global_menu: Option<GlobalMenuInfo>,
}

#[derive(Serialize, Clone, PartialEq)]
struct GlobalMenuInfo {
    bus_name: String,
    menu_path: Option<String>,
    app_path: Option<String>,
    window_path: Option<String>,
    // Which export flavour `menu_path` actually came from - "gtk" (a
    // real GMenuModel; actions are `app.xxx`/`win.xxx`, resolved against
    // `app_path`/`window_path` under those two prefixes) or "unity" (the
    // older Ubuntu-era export; actions are `unity.xxx`, all under one
    // group at `menu_path` itself). Not cosmetic: a consumer that guesses
    // wrong here gets a menu that renders with every item permanently
    // insensitive, since it inserted the D-Bus action group under the
    // wrong prefix - indistinguishable from a genuinely broken app
    // without this field. Some XWayland clients (`appmenu-gtk-module` in
    // particular) set both the `_GTK_*` and `_UNITY_OBJECT_PATH` atoms at
    // once, so a consumer can't reliably infer this from which paths are
    // merely non-null.
    source: &'static str,
}

impl From<&GlobalMenu> for GlobalMenuInfo {
    fn from(m: &GlobalMenu) -> Self {
        let source = match m.source {
            MenuSource::Gtk => "gtk",
            MenuSource::Unity => "unity",
        };
        Self { bus_name: m.bus_name.clone(), menu_path: m.menu_path.clone(), app_path: m.app_path.clone(), window_path: m.window_path.clone(), source }
    }
}

#[derive(Serialize)]
struct ClientsResponse {
    clients: Vec<ClientInfo>,
}

#[derive(Serialize)]
struct MonitorsResponse {
    monitors: Vec<MonitorInfo>,
}

/// One entry per `srdwm_core::Monitor` - both rects a panel/dock actually
/// needs to answer "does maximize respect my zone" and "does fullscreen
/// ignore it" without reasoning about either indirectly. Requested by an
/// AGS peer session after two separate live-debugging rounds (maximize-
/// past-dock, fullscreen-past-dock) each took several back-and-forth turns
/// that a single read of this would have settled immediately.
#[derive(Serialize)]
struct MonitorInfo {
    id: u32,
    name: String,
    primary: bool,
    // The usable area: the output shrunk by any layer-shell exclusive
    // zone currently reserved on it (a bar/dock's `set_exclusive_zone`).
    // What `toggle_maximize`/new-window placement/tiling all target.
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    // The output's true full rect, ignoring any exclusive zone - what
    // `toggle_fullscreen` targets. Equal to x/y/width/height above when
    // nothing on this monitor currently reserves any space at all.
    full_x: i32,
    full_y: i32,
    full_width: u32,
    full_height: u32,
}

/// Pushed to every subscriber (and used as `subscribe`'s own initial
/// reply) instead of `ClientsResponse`'s plain `{"clients": [...]}"` shape,
/// so every line a subscriber ever reads on that connection looks the
/// same - no special-casing the first one. Not used for the one-shot
/// `"clients"` command, whose response shape predates this and stays as-is
/// for existing polling consumers (`crates/ctl`, any external script).
#[derive(Serialize)]
struct ClientsEvent<'a> {
    event: &'static str,
    clients: &'a [ClientInfo],
}

/// The same per-window snapshot both `"clients"` and `"subscribe"`/the
/// change-diff in `IpcServer::poll` build - pulled out so the two can
/// never silently drift into reporting different fields.
fn client_snapshot(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> Vec<ClientInfo> {
    let wm = wm.borrow();
    let current = wm.current_workspace();
    let focused = wm.focused_id();
    wm.windows()
        .map(|w| ClientInfo {
            id: w.id,
            app_id: w.app_id.clone(),
            title: w.title.clone(),
            workspace: w.workspace,
            focused: focused == Some(w.id),
            minimized: w.minimized,
            visible: !w.minimized && w.workspace == current,
            scratchpad: w.scratchpad,
            floating: w.floating,
            x: w.geometry.x,
            y: w.geometry.y,
            width: w.geometry.width,
            height: w.geometry.height,
            global_menu: w.global_menu.as_ref().map(GlobalMenuInfo::from),
        })
        .collect()
}

#[derive(Serialize)]
struct OkResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

fn ok() -> Vec<u8> {
    serde_json::to_vec(&OkResponse { ok: true, error: None }).unwrap_or_default()
}

fn err(msg: &'static str) -> Vec<u8> {
    serde_json::to_vec(&OkResponse { ok: false, error: Some(msg) }).unwrap_or_default()
}

/// Parses and applies one request line, returning the response body (no
/// trailing newline) and whether it changed window state.
fn handle_request(line: &[u8], wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> (Vec<u8>, bool) {
    let Ok(req) = serde_json::from_slice::<serde_json::Value>(line) else {
        return (err("invalid request"), false);
    };
    let cmd = req.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    let id = req.get("id").and_then(|v| v.as_u64()).map(|v| v as WindowId);

    match cmd {
        "clients" => (serde_json::to_vec(&ClientsResponse { clients: client_snapshot(wm) }).unwrap_or_default(), false),
        "monitors" => {
            let monitors: Vec<MonitorInfo> = wm
                .borrow()
                .monitors()
                .iter()
                .map(|m| MonitorInfo {
                    id: m.id,
                    name: m.name.clone(),
                    primary: m.primary,
                    x: m.geometry.x,
                    y: m.geometry.y,
                    width: m.geometry.width,
                    height: m.geometry.height,
                    full_x: m.full_geometry.x,
                    full_y: m.full_geometry.y,
                    full_width: m.full_geometry.width,
                    full_height: m.full_geometry.height,
                })
                .collect();
            (serde_json::to_vec(&MonitorsResponse { monitors }).unwrap_or_default(), false)
        }
        // The connection is handed off to `IpcServer::subscribers` by the
        // caller (`poll`, which is the only place that can see the raw
        // `cmd` string this deep call already consumed) right after this
        // reply is written - this arm only has to produce that reply, in
        // the same `ClientsEvent` shape every later push uses.
        "subscribe" => {
            let clients = client_snapshot(wm);
            (serde_json::to_vec(&ClientsEvent { event: "clients", clients: &clients }).unwrap_or_default(), false)
        }
        "toggle_visibility" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let mut wm = wm.borrow_mut();
            let current = wm.current_workspace();
            let Some(w) = wm.windows().find(|w| w.id == id) else {
                return (err("no such window"), false);
            };
            let now_hidden = w.minimized || w.workspace != current;
            if now_hidden {
                // Follows the caller to whichever workspace is current --
                // matches Hyprland's `special:scratchpad`/Sway's `scratchpad
                // show`, which is the behaviour the `scratchpad` script and
                // its keybindings are written against.
                wm.move_window_to_workspace(id, current);
                wm.restore_window(id);
                wm.focus_window(id);
            } else {
                wm.minimize_window(id);
            }
            (ok(), true)
        }
        "focus" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().focus_window(id);
            (ok(), true)
        }
        "close" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().close_window(id);
            (ok(), true)
        }
        _ => (err("unknown command"), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::BufRead;
    use std::rc::Rc;

    fn read_line(stream: &mut UnixStream) -> String {
        // The server side is set non-blocking, but the client-side handle a
        // test holds is left in its default blocking mode - a plain
        // `read_line` on it can simply wait for the byte that's about to
        // arrive, no polling loop needed here.
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        line
    }

    #[test]
    fn subscribe_gets_an_immediate_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);

        let line = read_line(&mut client);
        assert!(line.contains(r#""event":"clients""#));
        assert!(line.contains(r#""clients":[]"#));
    }

    #[test]
    fn subscribe_then_a_window_change_pushes_a_fresh_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);
        let _initial = read_line(&mut client);

        {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "hello"));
        }
        server.poll(&wm);

        let pushed = read_line(&mut client);
        assert!(pushed.contains(r#""event":"clients""#));
        assert!(pushed.contains(r#""title":"hello""#));
    }

    #[test]
    fn a_poll_with_no_real_change_pushes_nothing_new() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);
        let _initial = read_line(&mut client);

        // Nothing changed between these two polls - a second push would
        // show up as a second readable line the client isn't expecting.
        server.poll(&wm);
        server.poll(&wm);
        client.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 16];
        match client.read(&mut buf) {
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            other => panic!("expected no further data, got {other:?}"),
        }
    }

    #[test]
    fn a_oneshot_clients_request_still_closes_the_connection_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        client.write_all(b"{\"cmd\":\"clients\"}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut client);
        // The plain, pre-existing shape - no `"event"` field - so
        // existing one-shot polling consumers (`crates/ctl`) see no change.
        assert!(!line.contains(r#""event""#));
        assert!(line.contains(r#""clients""#));

        {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "later"));
        }
        server.poll(&wm);
        client.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 16];
        let result = client.read(&mut buf);
        // A one-shot connection was never registered as a subscriber, so a
        // later change must not be pushed to it - and the server already
        // closed its end after the single reply, so a read either sees EOF
        // (0 bytes) or, depending on how quickly the close propagates,
        // WouldBlock; either is correct, actual new data would not be.
        match result {
            Ok(n) => assert_eq!(n, 0, "expected EOF, got {n} bytes of unexpected data"),
            Err(e) => assert_eq!(e.kind(), ErrorKind::WouldBlock),
        }
    }
}
