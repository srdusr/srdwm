//! A tiny local control socket, in the spirit of `hyprctl`/`swaymsg`, so
//! external scripts can query and drive window state without speaking
//! Wayland themselves. Bound at `$XDG_RUNTIME_DIR/srdwm-<display>.sock`.
//! `crates/ctl` (the `srd` binary) is the reference client.
//!
//! Deliberately synchronous and non-blocking-polled from each backend's
//! `poll_events()` tick (see `udev`/`winit`), the same way the
//! Wayland client socket itself is accepted - there is no calloop event
//! loop shared by both backends (`winit` has none at all), so this
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

use srdwm_core::{Direction, GlobalMenu, MenuSource, WindowId, WindowManager};

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
    /// `last_broadcast`'s workspace equivalent - diffed and pushed
    /// independently, see `WorkspacesEvent`'s doc comment for why this
    /// isn't folded into the field above.
    last_broadcast_workspaces: Vec<WorkspaceInfo>,
    /// `last_broadcast`'s keyboard-layout equivalent - see
    /// `KeyboardLayoutEvent`'s own doc comment.
    last_broadcast_keyboard_layout: String,
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
        Ok(Self {
            listener,
            path,
            conns: Vec::new(),
            subscribers: Vec::new(),
            last_broadcast: Vec::new(),
            last_broadcast_workspaces: Vec::new(),
            last_broadcast_keyboard_layout: String::new(),
        })
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
        // `last_broadcast`/`last_broadcast_workspaces` are kept in sync
        // with reality unconditionally, whether or not anyone is actually
        // subscribed right now - only the socket write itself is gated on
        // `self.subscribers` being non-empty. This matters at the exact
        // moment a new subscriber joins: their `"subscribe"` reply (built
        // separately, inside `handle_request`, from its own fresh
        // `client_snapshot`/`workspace_snapshot` call) already sent them a
        // full current snapshot, but `new_subscribers` hasn't been merged
        // into `self.subscribers` yet at this point in `poll` - so
        // `self.subscribers` may still be empty here even though a reply
        // just went out. Skipping the *sync* as well as the write (an
        // earlier version of this gated both behind one `is_empty` check)
        // left `last_broadcast*` stale until the next real change, so the
        // very next tick's diff saw a mismatch against what the new
        // subscriber was already sent and pushed a redundant duplicate --
        // for clients this coincidentally never fired (an empty window
        // list at construction matches `last_broadcast`'s own empty
        // starting value), but workspaces are never empty (`WindowManager
        // ::new` always seeds one), so every first subscriber got a
        // spurious extra `workspaces` line one tick after connecting.
        // Always syncing, and only conditionally writing, keeps both
        // invariants true at once: a fresh subscriber's direct reply is
        // never redundantly repeated, and an *existing* subscriber still
        // gets notified of any real change that happens in the same tick
        // a new one joins, since the diff against the old subscriber list
        // runs before `new_subscribers` is merged in below regardless.
        let current: Vec<ClientInfo> = client_snapshot(wm);
        if current != self.last_broadcast {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&ClientsEvent { event: "clients", clients: &current }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast = current;
        }
        let current_workspaces: Vec<WorkspaceInfo> = workspace_snapshot(wm);
        if current_workspaces != self.last_broadcast_workspaces {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&WorkspacesEvent { event: "workspaces", workspaces: &current_workspaces }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast_workspaces = current_workspaces;
        }
        let current_layout = wm.borrow().keyboard_layout.clone();
        if current_layout != self.last_broadcast_keyboard_layout {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&KeyboardLayoutEvent { event: "keyboard_layout", layout: &current_layout }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast_keyboard_layout = current_layout;
        }
        self.subscribers.extend(new_subscribers);
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
    // The active layout's own name for this window's workspace (`"dynamic"`
    // by default, `"tiling"` once a workspace opts in, or any name a config
    // registered via `srd.layout.configure`) - added because `floating`
    // alone can't answer "did *no* layout place this window" for an AGS
    // peer session's dock auto-hide logic: `floating` only reflects the
    // explicit per-window flag (scratchpad/rules/`srd.window.toggle_
    // floating`), which stays `false` by default even under `"dynamic"`,
    // the no-op layout that never places anything - every window on it is
    // effectively free-positioned regardless of what `floating` says. A
    // consumer that wants "is this window actually being tiled" needs
    // both: `layout` names a real tiling layout AND `floating` is false.
    layout: String,
    // Geometry, in the same global logical-pixel space everything else in
    // this compositor uses. Added for an external panel's Overview/window-
    // switcher, which has no other way to lay out window miniatures to
    // scale - neither `zwlr_foreign_toplevel_management_v1` nor
    // `ext_foreign_toplevel_list_v1` carries geometry at all, by design of
    // those protocols, so this compositor's own IPC is the only place it
    // can come from.
    //
    // This is `Window.geometry` verbatim: the *frame* rect, decorated
    // window and all - for a decorated window that means `height`
    // includes `TITLEBAR_HEIGHT` on top of the client's own content size,
    // the same "band added on top" convention hit-testing/rendering/every
    // other internal consumer of `Window.geometry` already uses. Not the
    // client's own content rect, which for a decorated window sits
    // `TITLEBAR_HEIGHT` logical pixels lower and that much shorter. A
    // consumer treating this as on-screen window extent (e.g. an overlap/
    // auto-hide test) wants the frame rect, which is what this is.
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
    // Which export flavour `menu_path` actually came from, and - as of
    // `MenuSource::DbusMenu` - which *protocol* it's actually in, not
    // just which action-group prefix to use. Getting the prefix wrong
    // (gtk/unity) renders a menu with every item permanently insensitive;
    // getting the protocol wrong (unity/dbusmenu) renders no menu at all,
    // since `Gio.DBusMenuModel` silently returns an empty model against a
    // `com.canonical.dbusmenu` object rather than erroring - both are
    // real failure modes an AGS peer session hit live, which is why this
    // is three values, not two:
    //
    // - "gtk": a real `GMenuModel`. Actions are `app.xxx`/`win.xxx`,
    //   resolved against `app_path`/`window_path` under those two
    //   prefixes.
    // - "unity": still `GMenuModel`/`org.gtk.Menus` underneath (same
    //   consumer as "gtk") - `appmenu-gtk-module`'s compatibility shim,
    //   which serves it under one `unity.xxx`-prefixed group at
    //   `menu_path` itself instead of `app`/`win`. Some XWayland clients
    //   set both the `_GTK_*` and `_UNITY_OBJECT_PATH` atoms at once, so a
    //   consumer can't reliably infer this from which paths are merely
    //   non-null.
    // - "dbusmenu": a genuinely different wire protocol,
    //   `com.canonical.dbusmenu` - needs an actual dbusmenu client, not a
    //   `GMenuModel` read under any prefix.
    source: &'static str,
}

impl From<&GlobalMenu> for GlobalMenuInfo {
    fn from(m: &GlobalMenu) -> Self {
        let source = match m.source {
            MenuSource::Gtk => "gtk",
            MenuSource::Unity => "unity",
            MenuSource::DbusMenu => "dbusmenu",
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

#[derive(Serialize)]
struct WorkspacesResponse {
    workspaces: Vec<WorkspaceInfo>,
}

/// `"keyboard_layout"`'s one-shot reply shape - the active XKB layout's
/// own name (e.g. `"English (US)"`), whatever `WindowManager::keyboard_
/// layout` currently holds. Added for an AGS peer session's keyboard-
/// layout badge, the last Hyprland-only control left in their shell
/// (`hyprctl devices -j` had no srdwm equivalent at all before this).
#[derive(Serialize)]
struct KeyboardLayoutResponse {
    layout: String,
}

/// One entry per `srdwm_core::Workspace` - added so an external panel (an
/// AGS peer session's workspace pills/Overview) can enumerate and switch
/// workspaces through this socket instead of a separate protocol client.
/// Deliberately no `urgent`/attention-request field: nothing in srdwm
/// tracks that concept anywhere yet (confirmed - `crates/wayland/src/
/// workspace.rs`'s own `ext-workspace-v1` implementation only ever sends
/// `State::Active` or empty, never a `Urgent` bit), so adding one here
/// would mean inventing what "urgent" means with no real signal behind it
/// rather than exposing something that already exists. A real design
/// (what sets it - an `xdg_toplevel` has no urgency concept at all; X11's
/// `_NET_WM_STATE_DEMANDS_ATTENTION` is the only actual signal on this
/// compositor today, and it's XWayland-only) belongs as its own piece of
/// work, not a field guessed at here.
#[derive(Serialize, Clone, PartialEq)]
struct WorkspaceInfo {
    id: usize,
    name: String,
    layout: String,
    active: bool,
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

/// `WorkspaceInfo`'s equivalent of `ClientsEvent` - a distinct event on
/// the same `subscribe` connection (one JSON object per line, `"event"`
/// says which), not folded into `ClientsEvent`: workspaces and windows
/// change independently (switching workspace touches no window; a window
/// closing touches no workspace), so diffing and broadcasting them
/// together would push a workspace-shaped payload on every window change
/// and vice versa for no reason.
#[derive(Serialize)]
struct WorkspacesEvent<'a> {
    event: &'static str,
    workspaces: &'a [WorkspaceInfo],
}

/// A third, independently-diffed event on the same `subscribe` connection
/// - see `WorkspacesEvent`'s own doc comment for why this isn't folded
/// into either of the other two: a layout cycle touches no window and no
/// workspace, so it needs its own change-diff to avoid pushing an
/// unrelated payload on every unrelated change.
#[derive(Serialize)]
struct KeyboardLayoutEvent<'a> {
    event: &'static str,
    layout: &'a str,
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
            layout: wm.layout_name(w.workspace).unwrap_or("dynamic").to_string(),
            x: w.geometry.x,
            y: w.geometry.y,
            width: w.geometry.width,
            height: w.geometry.height,
            global_menu: w.global_menu.as_ref().map(GlobalMenuInfo::from),
        })
        .collect()
}

/// `client_snapshot`'s workspace equivalent - same "one shared builder for
/// every consumer" reasoning, so `"workspaces"`, `"subscribe"`'s initial
/// reply, and the change-diff in `IpcServer::poll` can never drift into
/// reporting different fields for the same state.
fn workspace_snapshot(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> Vec<WorkspaceInfo> {
    let wm = wm.borrow();
    let current = wm.current_workspace();
    wm.workspaces().iter().map(|w| WorkspaceInfo { id: w.id, name: w.name.clone(), layout: w.layout.clone(), active: w.id == current }).collect()
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

/// The `move_window` dispatch's own direction-name parser - `crates/
/// config`'s own `parse_direction` (used by `srd.window.move`) isn't
/// reusable here: different crate, and it returns an `mlua::Result` tied
/// to the Lua binding's own error type. Same four names, same "small
/// duplication across crate boundaries beats a cross-crate dependency for
/// four match arms" tradeoff every other bit of shared naming in this
/// codebase already accepts.
fn parse_direction(name: &str) -> Option<Direction> {
    match name {
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        _ => None,
    }
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
        "workspaces" => (serde_json::to_vec(&WorkspacesResponse { workspaces: workspace_snapshot(wm) }).unwrap_or_default(), false),
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
            // Three JSON objects, not one: `poll` writes this response plus
            // one trailing `\n` verbatim, so an embedded `\n` between each
            // here is all it takes to hand a fresh subscriber every initial
            // snapshot as its own line - exactly the shape every later
            // push already uses, so there's nothing for a consumer to
            // special-case about the first few lines it reads.
            let clients = client_snapshot(wm);
            let workspaces = workspace_snapshot(wm);
            let layout = wm.borrow().keyboard_layout.clone();
            let mut out = serde_json::to_vec(&ClientsEvent { event: "clients", clients: &clients }).unwrap_or_default();
            out.push(b'\n');
            out.extend(serde_json::to_vec(&WorkspacesEvent { event: "workspaces", workspaces: &workspaces }).unwrap_or_default());
            out.push(b'\n');
            out.extend(serde_json::to_vec(&KeyboardLayoutEvent { event: "keyboard_layout", layout: &layout }).unwrap_or_default());
            (out, false)
        }
        "keyboard_layout" => {
            (serde_json::to_vec(&KeyboardLayoutResponse { layout: wm.borrow().keyboard_layout.clone() }).unwrap_or_default(), false)
        }
        "cycle_keyboard_layout" => {
            wm.borrow_mut().request_keyboard_layout_cycle();
            (ok(), true)
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
        // `srd dispatch lock` - no id, there's only ever one session to
        // lock. Core cannot lock the screen itself (real rendering/input-
        // routing, backend-owned); this just queues the request the same
        // way `set_output_position` queues one for whichever backend owns
        // real output hardware - see `WindowManager::request_lock`'s own
        // doc comment.
        "lock" => {
            wm.borrow_mut().request_lock();
            (ok(), true)
        }
        // `srd.window.maximize()`/`.fullscreen()`'s exact IPC-side
        // equivalents - lets an external script (or a live diagnostic
        // check, same as `toggle_visibility`/`focus`/`close` already allow)
        // drive either without needing a keybinding to already exist.
        "toggle_maximize" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_maximize(id);
            (ok(), true)
        }
        "toggle_fullscreen" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_fullscreen(id);
            (ok(), true)
        }
        // The four general compositor operations that have no standard
        // Wayland protocol to fall back on - confirmed with the AGS peer
        // session that `zwlr_foreign_toplevel_manager_v1` already covers
        // activate/close/maximize/minimize/fullscreen (so those stay
        // protocol-only, no bespoke verb here), but nothing in that
        // protocol or `ext-workspace-v1` can toggle floating, pin a
        // window, move one within the tiling order, or move one to a
        // specific workspace. Designed as plain general operations any
        // client can use (a panel, a script, a keybinding daemon), not
        // shaped around one particular shell's own IPC habits.
        "toggle_floating" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_floating(id);
            (ok(), true)
        }
        "toggle_pinned" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_always_on_top(id);
            (ok(), true)
        }
        // `{"cmd":"move_window","id":<window id>,"direction":"left"|"right"|"up"|"down"}`
        // - `WindowManager::move_window_direction` swaps the *focused*
        // window with its neighbour in that direction, so a caller asking
        // to move a window that isn't currently focused needs it focused
        // first; matches `movewindow` needing the target window active in
        // every tiling WM this gesture is modeled on.
        "move_window" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let Some(dir) = req.get("direction").and_then(|v| v.as_str()).and_then(parse_direction) else {
                return (err("direction must be one of: left, right, up, down"), false);
            };
            let mut wm = wm.borrow_mut();
            if wm.focused_id() != Some(id) {
                wm.focus_window(id);
            }
            wm.move_window_direction(dir);
            (ok(), true)
        }
        // `{"cmd":"move_to_workspace","id":<window id>,"workspace":<workspace id>}`
        // - the operation the AGS peer's Overview needs for drag-a-window-
        // onto-another-workspace, which `ext-workspace-v1` (activation
        // only, no toplevel-to-workspace verb) and `zwlr-foreign-toplevel`
        // (no workspace concept at all) both lack entirely.
        "move_to_workspace" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let Some(workspace) = req.get("workspace").and_then(|v| v.as_u64()) else {
                return (err("missing workspace"), false);
            };
            wm.borrow_mut().move_window_to_workspace(id, workspace as srdwm_core::WorkspaceId);
            (ok(), true)
        }
        // The workspace-side equivalent of `focus`: `id` here is a
        // `WorkspaceId`, not a `WindowId` - both are plain `usize`/`u64`
        // on the wire, so the same generic `id` field this whole match
        // already reads serves both, same as every other dispatch arm.
        "activate_workspace" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().switch_workspace(id as srdwm_core::WorkspaceId);
            (ok(), true)
        }
        // `{"cmd":"set_output_position","id":<monitor id>,"x":<i32>,"y":<i32>}`
        // - the primitive an output-configuration UI (a display-settings
        // panel, concretely the monitor-mirroring toggle this was built
        // for) needs and had no way to reach before: `wlr-output-
        // management-v1` already supports repositioning an output
        // (`crates/wayland/src/output_management.rs`), but only to a
        // client willing to implement that whole protocol itself just to
        // move one output. This exposes the same capability over the
        // plain IPC socket every other `srd dispatch` action already
        // uses. Deliberately just "move this output" with no separate
        // "mirror" concept anywhere: positioning two outputs at the same
        // coordinates already shows the same desktop region on both (every
        // window/render decision downstream works in shared global space,
        // not per-output), so mirroring is something a caller *achieves*
        // with this primitive, not something srdwm needs to know about as
        // its own state.
        //
        // Not applied here, and deliberately not a `WindowId` on the wire
        // despite reusing the same `id` field every other dispatch already
        // reads (both are plain integers on the wire; only the Rust-side
        // type differs) - this crate has no real output handle to move,
        // only `WindowManager`'s passive mirror of whatever the backend
        // last reported. Queued via `request_output_position` and applied
        // by whichever backend actually owns the hardware on its own next
        // poll, the same one-poll-tick latency every other backend-owned
        // effect in this IPC layer already has (a redraw, a geometry
        // change) - `changed = true` still makes sense to return since
        // this genuinely will change what's on screen once the backend
        // catches up, just not synchronously within this call.
        "set_output_position" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let (Some(x), Some(y)) = (req.get("x").and_then(|v| v.as_i64()), req.get("y").and_then(|v| v.as_i64())) else {
                return (err("missing x/y"), false);
            };
            wm.borrow_mut().request_output_position(id as srdwm_core::MonitorId, x as i32, y as i32);
            (ok(), true)
        }
        // `{"cmd":"capture_workspace","id":<workspace id>,"path":<string>,
        // "width":<u32>,"height":<u32>}` - `width`/`height` are optional,
        // both or neither. Exists for a workspace switcher's thumbnail
        // previews (AGS's Overview): `wlr-screencopy` - what `grim` and
        // this compositor's own `screencopy.rs` both use - can only ever
        // capture what an output is currently *presenting*, so a workspace
        // that isn't the active one is structurally invisible to it. This
        // is the one thing screencopy can't do, queued the same
        // cross-boundary way `set_output_position` is (core has no
        // renderer of its own) and drained by whichever backend is
        // actually running on its own next poll. Same one-poll-tick
        // latency as every other backend-owned effect this IPC layer
        // already has - the file exists shortly after this call returns,
        // not necessarily before it.
        "capture_workspace" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let Some(path) = req.get("path").and_then(|v| v.as_str()) else {
                return (err("missing path"), false);
            };
            let size = match (req.get("width").and_then(|v| v.as_u64()), req.get("height").and_then(|v| v.as_u64())) {
                (Some(w), Some(h)) => Some((w as u32, h as u32)),
                (None, None) => None,
                _ => return (err("width and height must both be given, or neither"), false),
            };
            wm.borrow_mut().request_capture_workspace(id as srdwm_core::WorkspaceId, path.to_string(), size);
            (ok(), true)
        }
        // Live theme values - an AGS peer session's equivalent of
        // Hyprland's `hyprctl keyword general:col.active_border ...`, the
        // mechanism their shell already uses to repaint window borders the
        // instant an accent palette/radius/etc changes in Settings. Was a
        // real, invisible gap before this: every one of these already had
        // a real, mutable `WindowManager` field (`theme.default_border_*`,
        // `tiling.gap_*`, `shadows_enabled`, `rounded_corners_enabled`),
        // set once from Lua config at startup and never touched again --
        // so a running session had no way to change any of it without a
        // full restart, unlike everything else `srd dispatch` already
        // covers live.
        //
        // No extra redraw call needed here: returning `changed = true`
        // (same as every other mutating command) is exactly what makes
        // `main.rs`'s `sync()` run its next tick, which already calls
        // `redraw_decoration`/`apply_geometry` for every visible window
        // unconditionally - this only has to mutate the right field and
        // let that existing machinery do the rest.
        "set" => handle_set(&req, wm),
        _ => (err("unknown command"), false),
    }
}

/// `{"cmd":"set","key":"border_width","value":3}` and the rest of `"set"`'s
/// keys - pulled out of `handle_request`'s match arm purely to keep that
/// match's per-arm bodies roughly the same size; no reuse motive.
///
/// A window's `border_color`/`border_width` are copied from `theme.
/// default_border_color`/`default_border_width` once, at creation
/// (`WindowManager::add_window`), and a rule's explicit `border_color`/
/// `border_width` action can overwrite that afterward - so a window
/// carrying the *old* default is, in practice, exactly the set of windows
/// that never had a rule override it (a rule-set colour coincidentally
/// equal to today's default is the only false positive, and updating it
/// to the new default too is a reasonable outcome, not a real bug). That
/// predicate is what the two colour/width arms below walk existing
/// windows with, rather than touching every window unconditionally.
fn handle_set(req: &serde_json::Value, wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> (Vec<u8>, bool) {
    let key = req.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = req.get("value");
    match key {
        "border_width" => {
            let Some(width) = value.and_then(|v| v.as_u64()) else { return (err("border_width needs a numeric value"), false) };
            let width = width as u32;
            let mut wm = wm.borrow_mut();
            let old = wm.theme.default_border_width;
            wm.theme.default_border_width = width;
            let matching: Vec<_> = wm.windows().filter(|w| w.border_width == old).map(|w| w.id).collect();
            for id in matching {
                if let Some(w) = wm.window_mut(id) {
                    w.border_width = width;
                }
            }
            (ok(), true)
        }
        "border_color" => {
            let Some(hex) = value.and_then(|v| v.as_str()) else { return (err("border_color needs a hex string value"), false) };
            let Some(rgb) = srdwm_core::parse_hex_color(hex) else { return (err("border_color must be a hex string like #cba6f7"), false) };
            let mut wm = wm.borrow_mut();
            let old = wm.theme.default_border_color;
            wm.theme.default_border_color = rgb;
            let matching: Vec<_> = wm.windows().filter(|w| w.border_color == old).map(|w| w.id).collect();
            for id in matching {
                if let Some(w) = wm.window_mut(id) {
                    w.border_color = rgb;
                }
            }
            (ok(), true)
        }
        // `border_width`'s exact twin, for the titlebar/border-strip corner
        // radius - same "only touch windows still carrying the old
        // default" predicate, so a window a rule already gave its own
        // explicit `corner_radius` isn't silently overwritten by a later
        // live-set.
        "corner_radius" => {
            let Some(radius) = value.and_then(|v| v.as_u64()) else { return (err("corner_radius needs a numeric value"), false) };
            let radius = radius as u32;
            let mut wm = wm.borrow_mut();
            let old = wm.theme.default_corner_radius;
            wm.theme.default_corner_radius = radius;
            let matching: Vec<_> = wm.windows().filter(|w| w.corner_radius == old).map(|w| w.id).collect();
            for id in matching {
                if let Some(w) = wm.window_mut(id) {
                    w.corner_radius = radius;
                }
            }
            (ok(), true)
        }
        // Live A/B-testing knob for `srdwm_core::ThemeConfig::
        // default_decorated` - see its own doc comment for the "which
        // desktop environment does what" reasoning behind making this
        // configurable at all. Deliberately only affects windows created
        // *after* this call, not existing ones - retroactively flipping
        // an already-mapped window's decoration needs the same redraw-buffer
        // + geometry-resync `set_decorated_from_mode` does on the Wayland
        // side (backend-specific, unreachable from this backend-agnostic
        // `crates/platform` code), and the actual use case here is testing
        // which default a freshly opened app gets, not live-migrating
        // windows already on screen.
        "decoration_mode" => {
            let Some(mode) = value.and_then(|v| v.as_str().map(str::to_string)) else {
                return (err("decoration_mode needs \"server\" or \"client\""), false);
            };
            if mode != "server" && mode != "client" {
                return (err("decoration_mode needs \"server\" or \"client\""), false);
            }
            wm.borrow_mut().theme.default_decorated = mode != "client";
            (ok(), true)
        }
        // Tiling-only: `arrange_workspace` skips floating/fullscreen
        // windows regardless, and under `"dynamic"` (the no-op default
        // layout) nothing reads `tiling.gap_*` at all - so setting these
        // is a correct no-op, visually, until a workspace actually runs
        // the `"tiling"` layout, exactly matching what Hyprland's own
        // `general:gaps_*` do under its own non-tiling/floating windows.
        "gap_inner" => {
            let Some(v) = value.and_then(|v| v.as_u64()) else { return (err("gap_inner needs a numeric value"), false) };
            wm.borrow_mut().tiling.gap_inner = v as u32;
            (ok(), true)
        }
        "gap_outer" => {
            let Some(v) = value.and_then(|v| v.as_u64()) else { return (err("gap_outer needs a numeric value"), false) };
            wm.borrow_mut().tiling.gap_outer = v as u32;
            (ok(), true)
        }
        "shadows" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("shadows needs a boolean value"), false) };
            wm.borrow_mut().shadows_enabled = v;
            (ok(), true)
        }
        // A bool, not a radius: the actual corner radius is a fixed
        // constant (`crates/wayland/src/decoration.rs::CORNER_RADIUS`),
        // not a per-session config value anywhere in the compositor yet --
        // this can only turn rounding on/off, matching `WindowManager::
        // rounded_corners_enabled`'s existing `Option<bool>` shape (also
        // config-settable at startup via `general.rounded_corners`, never
        // live until now). A live-settable numeric radius is real, separate
        // future work, not something to fake here with a value that's
        // silently ignored.
        "rounded_corners" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("rounded_corners needs a boolean value"), false) };
            wm.borrow_mut().rounded_corners_enabled = Some(v);
            (ok(), true)
        }
        "blur" => (err("blur is not supported - no GPU shader path on this compositor's software renderer yet"), false),
        _ => (err("unknown set key"), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::BufRead;
    use std::rc::Rc;

    /// Takes a reader the caller already owns, rather than building a fresh
    /// `BufReader` around a cloned handle every call (what this used to do):
    /// `BufRead::read_line` is free to read further ahead than one line in
    /// a single syscall whenever more is already sitting in the kernel
    /// socket buffer - true the moment a caller (like `"subscribe"`'s
    /// two-line initial reply, added alongside the workspace event) writes
    /// more than one line in one `write_all`. A fresh `BufReader` built
    /// per call has nowhere to keep whatever it over-read once the call
    /// returns and it's dropped - that data is already gone from the
    /// kernel's queue, so the *next* fresh `BufReader`'s read blocks
    /// forever waiting for bytes that already arrived and were silently
    /// discarded. Hung an entire test run silently, with no compiler error
    /// and no panic to point at it, until the underlying `cargo test`
    /// process was found sitting at 0% CPU with no explanation. One
    /// `BufReader`, reused for every `read_line` call in a test, keeps
    /// whatever it over-reads available for the next call instead.
    fn read_line(reader: &mut std::io::BufReader<UnixStream>) -> String {
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
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);

        // Three lines, not one: a client snapshot, a workspace snapshot,
        // and a keyboard-layout snapshot, same as every later push - see
        // the `"subscribe"` match arm's own doc comment for why all three
        // are sent immediately rather than waiting for the first real
        // change of each kind.
        let clients_line = read_line(&mut reader);
        assert!(clients_line.contains(r#""event":"clients""#));
        assert!(clients_line.contains(r#""clients":[]"#));

        let workspaces_line = read_line(&mut reader);
        assert!(workspaces_line.contains(r#""event":"workspaces""#));
        assert!(workspaces_line.contains(r#""id":0"#));
        assert!(workspaces_line.contains(r#""active":true"#));

        let layout_line = read_line(&mut reader);
        assert!(layout_line.contains(r#""event":"keyboard_layout""#));
        assert!(layout_line.contains(r#""layout":""#));
    }

    #[test]
    fn subscribe_then_a_window_change_pushes_a_fresh_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);
        let _initial_clients = read_line(&mut reader);
        let _initial_workspaces = read_line(&mut reader);
        let _initial_layout = read_line(&mut reader);

        {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "hello"));
        }
        server.poll(&wm);

        let pushed = read_line(&mut reader);
        assert!(pushed.contains(r#""event":"clients""#));
        assert!(pushed.contains(r#""title":"hello""#));
    }

    #[test]
    fn subscribe_then_a_workspace_switch_pushes_a_fresh_workspaces_event() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().add_workspace("2", "dynamic");

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);
        let _initial_clients = read_line(&mut reader);
        let _initial_workspaces = read_line(&mut reader);
        let _initial_layout = read_line(&mut reader);

        wm.borrow_mut().switch_workspace(1);
        server.poll(&wm);

        // Switching touches no window, so the clients list is unchanged --
        // the next line waiting must be the workspaces push, not a
        // clients one that never comes.
        let pushed = read_line(&mut reader);
        assert!(pushed.contains(r#""event":"workspaces""#));
        assert!(pushed.contains(r#""id":1,"name":"2","layout":"dynamic","active":true"#));
    }

    #[test]
    fn a_poll_with_no_real_change_pushes_nothing_new() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);
        let _initial_clients = read_line(&mut reader);
        let _initial_workspaces = read_line(&mut reader);
        let _initial_layout = read_line(&mut reader);

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
    fn workspaces_command_reports_the_default_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"workspaces\"}\n").unwrap();
        server.poll(&wm);

        let line = read_line(&mut reader);
        assert!(!line.contains(r#""event""#), "one-shot command, same plain shape as \"clients\"");
        assert!(line.contains(r#""id":0,"name":"1","layout":"dynamic","active":true"#));
    }

    #[test]
    fn keyboard_layout_command_reports_the_current_layout() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_keyboard_layout("English (US)");

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"keyboard_layout\"}\n").unwrap();
        server.poll(&wm);

        let line = read_line(&mut reader);
        assert!(!line.contains(r#""event""#), "one-shot command, same plain shape as \"clients\"");
        assert!(line.contains(r#""layout":"English (US)""#));
    }

    #[test]
    fn cycle_keyboard_layout_dispatch_queues_a_request_for_main_rs_to_act_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"cycle_keyboard_layout\"}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        // `IpcServer` has no seat/keyboard of its own to actually cycle --
        // it can only queue the intent for `main.rs`'s `sync()` to act on,
        // same as `close_requests`/`activate_workspace`. This is as far as
        // this crate can verify the request landed.
        assert_eq!(wm.borrow_mut().take_keyboard_layout_cycle_requests(), 1);
    }

    #[test]
    fn activate_workspace_dispatch_switches_the_current_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().add_workspace("2", "dynamic");

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"activate_workspace\",\"id\":1}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        assert_eq!(wm.borrow().current_workspace(), 1);
    }

    #[test]
    fn lock_dispatch_queues_a_lock_request_with_no_id_needed() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        assert!(!wm.borrow_mut().drain_lock_request(), "nothing queued before the dispatch");

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"lock\"}\n").unwrap();
        server.poll(&wm);
        let response = read_line(&mut reader);

        assert!(!response.contains(r#""ok":false"#), "lock must not require an id: {response}");
        assert!(wm.borrow_mut().drain_lock_request(), "the dispatch must have queued a lock request");
    }

    #[test]
    fn toggle_floating_dispatch_flips_the_windows_floating_flag() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let id = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "a"));
            id
        };
        assert!(!wm.borrow().is_floating(id));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(format!("{{\"cmd\":\"toggle_floating\",\"id\":{id}}}\n").as_bytes()).unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        assert!(wm.borrow().is_floating(id));
    }

    #[test]
    fn toggle_pinned_dispatch_flips_always_on_top() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let id = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "a"));
            id
        };

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(format!("{{\"cmd\":\"toggle_pinned\",\"id\":{id}}}\n").as_bytes()).unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        assert!(wm.borrow().window(id).unwrap().always_on_top);
    }

    #[test]
    fn move_window_dispatch_swaps_with_the_neighbour_and_focuses_the_target_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "primary", srdwm_core::Rect::new(0, 0, 1920, 1080))]);
        let (a, b) = {
            let mut wm = wm.borrow_mut();
            let a = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(a, "a"));
            let b = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(b, "b"));
            // Set geometry *after* `add_window`, not before - a dynamic-
            // layout workspace's own `SmartPlacement` grid overrides
            // whatever geometry a freshly constructed `Window` already
            // carried in, the same lesson `crates/core/src/manager/
            // tests.rs`'s decoration-mode tests already ran into.
            wm.window_mut(a).unwrap().geometry = srdwm_core::Rect::new(0, 0, 400, 300);
            wm.window_mut(b).unwrap().geometry = srdwm_core::Rect::new(500, 0, 400, 300);
            (a, b)
        };
        // Focus `a` first, then ask to move `b` - the dispatch must focus
        // `b` itself before swapping, not silently move whatever was
        // already focused.
        wm.borrow_mut().focus_window(a);

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(format!("{{\"cmd\":\"move_window\",\"id\":{b},\"direction\":\"left\"}}\n").as_bytes()).unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        let wm = wm.borrow();
        assert_eq!(wm.window(b).unwrap().geometry.x, 0, "b must have swapped into a's old position");
        assert_eq!(wm.window(a).unwrap().geometry.x, 500, "a must have swapped into b's old position");
    }

    #[test]
    fn move_to_workspace_dispatch_moves_the_window_without_switching_the_current_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().add_workspace("2", "dynamic");
        let id = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "a"));
            id
        };

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(format!("{{\"cmd\":\"move_to_workspace\",\"id\":{id},\"workspace\":1}}\n").as_bytes()).unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        let wm = wm.borrow();
        assert_eq!(wm.window(id).unwrap().workspace, 1);
        assert_eq!(wm.current_workspace(), 0, "moving a window to a workspace must not also switch to it");
    }

    #[test]
    fn a_oneshot_clients_request_still_closes_the_connection_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"clients\"}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut reader);
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
