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
    /// `last_broadcast`'s monitor equivalent - see `MonitorsEvent`'s own
    /// doc comment.
    last_broadcast_monitors: Vec<MonitorInfo>,
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
            last_broadcast_monitors: Vec::new(),
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
        let current_monitors = monitor_snapshot(wm);
        if current_monitors != self.last_broadcast_monitors {
            if !self.subscribers.is_empty() {
                if let Ok(mut out) = serde_json::to_vec(&MonitorsEvent { event: "monitors", monitors: &current_monitors }) {
                    out.push(b'\n');
                    self.subscribers.retain_mut(|s| s.write_all(&out).is_ok());
                }
            }
            self.last_broadcast_monitors = current_monitors;
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

/// `"settings"`'s one-shot reply - the live-settable toggles `"set"`
/// accepts, so a migrated toggle script (night-light, reading-mode,
/// hypr-performance-profile) can read current state back instead of
/// tracking its own on/off marker file, the same gap `hyprctl getoption
/// -j` used to fill for the Hyprland scripts these replaced.
/// `rounded_corners` is `null` for "never explicitly set" (see
/// `WindowManager::rounded_corners_enabled`'s own doc comment) --
/// `Option<bool>` serializes to exactly that.
#[derive(Serialize)]
struct SettingsResponse {
    shadows: bool,
    rounded_corners: Option<bool>,
    animations: bool,
    night_light: bool,
    reading_mode: bool,
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
    // The monitor currently showing this workspace, if any - `None` when
    // it isn't visible anywhere right now. In shared mode (`workspace.
    // per_monitor` off, the default) every monitor shows the same
    // workspace, so at most one workspace ever has this set; in per-
    // monitor mode each monitor can be on a different one, so more than
    // one entry here can carry a (different) monitor id at once. Requested
    // by an AGS peer session alongside `MonitorInfo::active_workspace`
    // below - the same fact, indexed from the other direction, so a
    // caller can look it up from whichever side (a workspace pill, or a
    // per-monitor picker) it already has in hand without cross-
    // referencing the other endpoint itself.
    monitor: Option<srdwm_core::MonitorId>,
}

/// One entry per `srdwm_core::Monitor` - both rects a panel/dock actually
/// needs to answer "does maximize respect my zone" and "does fullscreen
/// ignore it" without reasoning about either indirectly. Requested by an
/// AGS peer session after two separate live-debugging rounds (maximize-
/// past-dock, fullscreen-past-dock) each took several back-and-forth turns
/// that a single read of this would have settled immediately.
///
/// Every geometry field here - `x`/`y`/`width`/`height` and `full_x`/
/// `full_y`/`full_width`/`full_height` alike - is in the same space:
/// *physical* pixels, matching the real output mode, not the logical
/// points a Wayland client itself sees. `srd dispatch set output
/// position` takes the same physical space, so an arrangement computed
/// purely from fields on this struct (chaining outputs by `full_width`,
/// say) can be sent straight back with no conversion. `scale` is the one
/// exception - multiply a logical value by it to get physical, or divide
/// physical by it to get logical - needed only when a caller has to
/// reconcile this compositor's own physical bookkeeping against a real
/// Wayland client's logical one (window positions/sizes a client reports
/// about itself, for instance). See `srdwm_core::monitor::Monitor::
/// scale`'s own doc comment for the live bug this was added to fix: an
/// AGS peer session's own arrangement math, built purely from this
/// struct's fields, silently opened a dead gap between two monitors on
/// any output whose scale wasn't exactly `1.0`, because nothing here told
/// it a non-`1.0` scale was even in play.
#[derive(Serialize, Clone, PartialEq)]
struct MonitorInfo {
    id: u32,
    name: String,
    primary: bool,
    // The usable area (work area): the output shrunk by any layer-shell
    // exclusive zone currently reserved on it (a bar/dock's `set_
    // exclusive_zone`). What `toggle_maximize`/new-window placement/
    // tiling all target - NOT what a display-arrangement UI should
    // position outputs by. Confirmed live: an AGS peer session's monitor-
    // layout panel originally read x/y/width/height here for positioning
    // outputs relative to each other and got every layout offset by
    // whatever the bar's own reserved zone happened to be (34px, on this
    // machine) - the bare, unprefixed names here read as "the output"
    // rather than "the output's usable sub-rect", which is exactly the
    // trap. Position outputs using `full_x`/`full_y`/`full_width`/
    // `full_height` below instead.
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    // The output's true full rect, ignoring any exclusive zone - what
    // `toggle_fullscreen` targets, and what a display-arrangement/output-
    // positioning UI should read (`set_output_position` moves this rect,
    // not the work-area one above). Equal to x/y/width/height above when
    // nothing on this monitor currently reserves any space at all, which
    // is exactly why the mistake above is easy to make and easy to miss
    // in testing - it only shows up once something (a bar, a dock) is
    // actually reserving space.
    full_x: i32,
    full_y: i32,
    full_width: u32,
    full_height: u32,
    // `true` for every genuinely live output. `false` marks an
    // administratively-disabled-but-still-connected one (`srd dispatch
    // set output enabled <name> false`) - requested directly by the AGS
    // peer session so their monitor-layout panel has a name/row to
    // re-enable by, rather than the output vanishing from this list
    // entirely the moment the control that turns it off is used. A
    // genuinely *unplugged* output, disabled or not, still just
    // disappears from this list as before - `enabled: false` means "off,
    // but still here to turn back on", not "not connected".
    enabled: bool,
    // `true` when this entry is one part of a real output divided by
    // `srd.monitor.split` - not a second `wl_output`, not a second
    // physical connector. See `srdwm_core::monitor::Monitor::split`'s own
    // doc comment: a display-arrangement UI should not offer to move or
    // extend a physical arrangement onto one of these, since there is no
    // independent output behind it, only a placement-only division of a
    // real one.
    split: bool,
    // This output's real scale factor - `1.0` for an unscaled one. See
    // this struct's own doc comment for what it converts between and why.
    scale: f64,
    // The workspace id currently showing on this monitor - `Window
    // Manager::workspace_for_monitor`, keyed by monitor id, not name (a
    // display-arrangement UI already has this monitor's own id in hand
    // from this same entry). Every monitor has exactly one value here even
    // in shared mode (`workspace.per_monitor` off): they all report the
    // same id, `current_workspace`, rather than this field going missing
    // just because it isn't independently meaningful yet - a caller
    // shouldn't need to know which mode is active just to read this.
    // `0` (an id that can never be real - workspace ids are 1-based) for
    // a disabled-but-listed output below: it shows nothing, so there is no
    // real answer, and `0` reads unambiguously as "none" rather than
    // silently reusing `current_workspace`, which that output isn't
    // actually displaying. See `WorkspaceInfo::monitor` for the same fact
    // indexed from the other direction.
    active_workspace: usize,
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

/// A fourth independently-diffed `subscribe` event - a monitor connecting
/// or disconnecting touches neither a window, a workspace, nor the
/// keyboard layout, so it needs the same dedicated change-diff those three
/// already get. Requested by an AGS peer session so a display-arrangement
/// panel's "output connected" indicator can drop its own 4-second poll of
/// `"monitors"` (worked, but `hypr.connect("monitor-added", ...)` - the
/// signal this shell normally listens for - is a dead handler id on any
/// backend but Hyprland, since `lib/compositor.ts` swallows unknown signal
/// names by design rather than erroring).
#[derive(Serialize)]
struct MonitorsEvent<'a> {
    event: &'static str,
    monitors: &'a [MonitorInfo],
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
    // `is_workspace_visible`, not a single `id == current` comparison: in
    // `workspace.per_monitor` mode more than one workspace can be showing
    // at once (one per monitor), each needing its own `active: true` here
    // - `WorkspaceInfo::active` is already a plain per-workspace bool, not
    // "the one active id", so this needed no wire-format change at all,
    // just computing the flag correctly for both modes. Shared mode
    // (`per_monitor_workspaces` off, the default) behaves exactly as
    // before: `is_workspace_visible` reduces straight back to the same
    // `id == current` check.
    let monitors = wm.monitors();
    wm.workspaces()
        .iter()
        .map(|w| {
            // First monitor (if any) currently showing this workspace --
            // see `WorkspaceInfo::monitor`'s own doc comment for why "first"
            // rather than "the" is the honest framing (shared mode never has
            // more than one; per-monitor mode could, in principle, if a
            // caller pointed two monitors at the same workspace id).
            let monitor = monitors.iter().find(|m| wm.workspace_for_monitor(m.id) == w.id).map(|m| m.id);
            WorkspaceInfo { id: w.id, name: w.name.clone(), layout: w.layout.clone(), active: wm.is_workspace_visible(w.id), monitor }
        })
        .collect()
}

#[derive(Serialize)]
struct OkResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

/// Shared by the one-shot `"monitors"` reply and `IpcServer::poll`'s own
/// subscribe-broadcast diff below, so a hotplug event and a plain query
/// can never quietly drift into reporting different fields for the same
/// monitor - the same reasoning `client_snapshot`/`workspace_snapshot`
/// already established for their own data.
/// Resolves a dispatch's target monitor from whichever of `id`/`name` it
/// actually sent - shared by `set_output_position` and `set_output_
/// enabled`, both of which accept either. `id` wins if both are somehow
/// given (matching the generic `id` field every other dispatch already
/// reads); `name` is resolved against the live monitor list, matching
/// what `srd monitors`/`wlr-output-management-v1` both key on
/// (`eDP-1`, not an arbitrary index) - a display-
/// arrangement UI reasonably lists outputs by that name rather than
/// making a caller look its own id up first just to turn around and send
/// it straight back.
fn resolve_monitor_id(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>, id: Option<WindowId>, name: Option<&str>) -> Option<srdwm_core::MonitorId> {
    match id {
        Some(id) => Some(id as srdwm_core::MonitorId),
        None => name.and_then(|name| wm.borrow().monitors().iter().find(|m| m.name == name).map(|m| m.id)),
    }
}

fn monitor_snapshot(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> Vec<MonitorInfo> {
    let wm = wm.borrow();
    let live = wm.monitors().iter().map(|m| MonitorInfo {
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
        enabled: true,
        split: m.split,
        scale: m.scale,
        active_workspace: wm.workspace_for_monitor(m.id),
    });
    // Disabled-but-still-connected outputs, appended rather than merged in
    // by name - see `MonitorInfo::enabled`'s own doc comment for why
    // these are listed at all. `id: u32::MAX`: a disabled output has no
    // real backend id any more (see `WindowManager::request_output_
    // enabled`'s own doc comment on why re-enabling has to go by name),
    // so this is a deliberate, obviously-not-a-real-index sentinel rather
    // than reusing `0`/whatever the id happened to be before disabling,
    // which could collide with (or be mistaken for) a real live monitor's
    // own id.
    let disabled = wm.disabled_monitors().map(|(name, m)| MonitorInfo {
        id: u32::MAX,
        name: name.to_string(),
        primary: m.primary,
        x: m.geometry.x,
        y: m.geometry.y,
        width: m.geometry.width,
        height: m.geometry.height,
        full_x: m.full_geometry.x,
        full_y: m.full_geometry.y,
        full_width: m.full_geometry.width,
        full_height: m.full_geometry.height,
        enabled: false,
        split: false,
        // Not tracked for a disabled output - `DisabledMonitor` keeps a
        // last-known geometry snapshot but not a scale, and re-deriving
        // one from stale connector state isn't worth the plumbing for an
        // output that's off. `1.0` here means "unknown", not "confirmed
        // unscaled" - a caller re-arranging outputs shouldn't trust it
        // until this one is live again and `srd monitors` reports its
        // real value.
        scale: 1.0,
        // See `MonitorInfo::active_workspace`'s own doc comment - `0` for
        // "shows nothing, not tracked" the same way `id: u32::MAX` above
        // is a deliberate not-a-real-value sentinel for this same entry.
        active_workspace: 0,
    });
    live.chain(disabled).collect()
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
        "settings" => {
            let wm = wm.borrow();
            let settings = SettingsResponse {
                shadows: wm.shadows_enabled,
                rounded_corners: wm.rounded_corners_enabled,
                animations: wm.animations_enabled,
                night_light: wm.color_filter == srdwm_core::ColorFilter::NightLight,
                reading_mode: wm.color_filter == srdwm_core::ColorFilter::ReadingMode,
            };
            (serde_json::to_vec(&settings).unwrap_or_default(), false)
        }
        "monitors" => (serde_json::to_vec(&MonitorsResponse { monitors: monitor_snapshot(wm) }).unwrap_or_default(), false),
        // The connection is handed off to `IpcServer::subscribers` by the
        // caller (`poll`, which is the only place that can see the raw
        // `cmd` string this deep call already consumed) right after this
        // reply is written - this arm only has to produce that reply, in
        // the same `ClientsEvent` shape every later push uses.
        "subscribe" => {
            // Four JSON objects, not one: `poll` writes this response plus
            // one trailing `\n` verbatim, so an embedded `\n` between each
            // here is all it takes to hand a fresh subscriber every initial
            // snapshot as its own line - exactly the shape every later
            // push already uses, so there's nothing for a consumer to
            // special-case about the first few lines it reads.
            let clients = client_snapshot(wm);
            let workspaces = workspace_snapshot(wm);
            let layout = wm.borrow().keyboard_layout.clone();
            let monitors = monitor_snapshot(wm);
            let mut out = serde_json::to_vec(&ClientsEvent { event: "clients", clients: &clients }).unwrap_or_default();
            out.push(b'\n');
            out.extend(serde_json::to_vec(&WorkspacesEvent { event: "workspaces", workspaces: &workspaces }).unwrap_or_default());
            out.push(b'\n');
            out.extend(serde_json::to_vec(&KeyboardLayoutEvent { event: "keyboard_layout", layout: &layout }).unwrap_or_default());
            out.push(b'\n');
            out.extend(serde_json::to_vec(&MonitorsEvent { event: "monitors", monitors: &monitors }).unwrap_or_default());
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
            let before = wm.borrow().current_workspace();
            // `switch_workspace_on_monitor` falls straight through to the
            // ordinary shared-mode `switch_workspace` when `workspace.
            // per_monitor` is off, so this is the one call site that works
            // correctly either way - no branching on the config flag
            // needed here. The monitor it applies to in per-monitor mode:
            // the focused window's own monitor, falling back to the
            // primary monitor if nothing is focused (an empty desktop) --
            // the same "whichever output a keybinding should apply to"
            // choice real per-output-aware WMs (Hyprland, niri) make.
            {
                let mut wm = wm.borrow_mut();
                let monitor = wm
                    .focused_id()
                    .and_then(|f| wm.window(f))
                    .map(|w| w.monitor)
                    .or_else(|| wm.primary_monitor().map(|m| m.id))
                    .unwrap_or(0);
                wm.switch_workspace_on_monitor(id as srdwm_core::WorkspaceId, monitor);
            }
            let after = wm.borrow().current_workspace();
            let known: Vec<_> = wm.borrow().workspaces().iter().map(|w| w.id).collect();
            log::warn!("WS-IPC-DIAG requested_id={id} before={before} after={after} known_ids={known:?}");
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
            // Accepts a monitor `name` as well as the plain `id` every
            // other dispatch already reads - `srd monitors`/`wlr-output-
            // management-v1` both key on name first (`eDP-1`,
            // not an arbitrary index), and a display-arrangement UI
            // reasonably lists outputs by that name rather than making a
            // caller look its own id up first just to turn around and send
            // it straight back. `id` still wins if both are somehow given.
            let Some(monitor_id) = resolve_monitor_id(wm, id, req.get("name").and_then(|v| v.as_str())) else {
                return (err("missing id or a name matching a connected monitor"), false);
            };
            let (Some(x), Some(y)) = (req.get("x").and_then(|v| v.as_i64()), req.get("y").and_then(|v| v.as_i64())) else {
                return (err("missing x/y"), false);
            };
            wm.borrow_mut().request_output_position(monitor_id, x as i32, y as i32);
            (ok(), true)
        }
        // `{"cmd":"set_output_enabled","id"|"name":...,"enabled":<bool>}`
        // - "primary only"/a per-display toggle, the two AGS monitor-
        // layout panel rows gated pending this. Disabling and re-enabling
        // reuse this backend's own existing hotplug-removal/bring-up code
        // paths rather than a new mechanism (see the udev platform's own
        // drain site) - the same real, already-tested steps a genuine
        // unplug/replug already goes through, just triggered
        // administratively instead of by a real DRM event.
        //
        // Resolved to a *name* here, unlike `set_output_position` (which
        // stays on `resolve_monitor_id`/plain `MonitorId`) - see
        // `WindowManager::request_output_enabled`'s own doc comment for
        // why: disabling removes the output from `monitors()` entirely, so
        // its id has nothing left to mean by the time a caller wants to
        // *re-enable* it. `id` is still accepted, resolved against the
        // live list the same way `resolve_monitor_id` does, but that only
        // ever works for the disable direction (the output is still live
        // when you ask to turn it off) - re-enabling a currently-disabled
        // output needs its `name` given directly, since no live entry
        // exists to resolve an `id` against at that point.
        "set_output_enabled" => {
            let name = match req.get("name").and_then(|v| v.as_str()) {
                Some(name) => Some(name.to_string()),
                None => id.and_then(|id| wm.borrow().monitors().iter().find(|m| m.id == id as srdwm_core::MonitorId).map(|m| m.name.clone())),
            };
            let Some(name) = name else { return (err("missing name, or an id matching a currently-connected monitor"), false) };
            let Some(enabled) = req.get("enabled").and_then(|v| v.as_bool()) else {
                return (err("missing enabled"), false);
            };
            wm.borrow_mut().request_output_enabled(name, enabled);
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
        // Same shape as `shadows` - `WindowManager::animations_enabled`
        // already existed (config-settable at startup via `general.
        // animations`) but had no live IPC toggle, unlike shadows/rounded
        // corners which did. Added specifically so a performance-profile
        // script (ported from a Hyprland one that used `hyprctl keyword
        // animations:enabled`) has something real to call instead of
        // silently no-op-ing under srdwm.
        "animations" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("animations needs a boolean value"), false) };
            wm.borrow_mut().animations_enabled = v;
            (ok(), true)
        }
        "blur" => (err("blur is not supported - no GPU shader path on this compositor's software renderer yet"), false),
        // The two ported Hyprland `decoration:screen_shader` scripts --
        // mutually exclusive by construction (`srdwm_core::ColorFilter` is
        // one enum, not two bools), matching the original scripts' own
        // "both point at the same single shader slot" behaviour: setting
        // either key `true` clears the other, and `false` always clears to
        // `None` regardless of which one (if any) was actually active.
        "night_light" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("night_light needs a boolean value"), false) };
            wm.borrow_mut().color_filter = if v { srdwm_core::ColorFilter::NightLight } else { srdwm_core::ColorFilter::None };
            (ok(), true)
        }
        "reading_mode" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("reading_mode needs a boolean value"), false) };
            wm.borrow_mut().color_filter = if v { srdwm_core::ColorFilter::ReadingMode } else { srdwm_core::ColorFilter::None };
            (ok(), true)
        }
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

        // Four lines, not one: a client snapshot, a workspace snapshot, a
        // keyboard-layout snapshot, and a monitor snapshot, same as every
        // later push - see the `"subscribe"` match arm's own doc comment
        // for why all four are sent immediately rather than waiting for
        // the first real change of each kind.
        let clients_line = read_line(&mut reader);
        assert!(clients_line.contains(r#""event":"clients""#));
        assert!(clients_line.contains(r#""clients":[]"#));

        let workspaces_line = read_line(&mut reader);
        assert!(workspaces_line.contains(r#""event":"workspaces""#));
        assert!(workspaces_line.contains(r#""id":1"#));
        assert!(workspaces_line.contains(r#""active":true"#));

        let layout_line = read_line(&mut reader);
        assert!(layout_line.contains(r#""event":"keyboard_layout""#));
        assert!(layout_line.contains(r#""layout":""#));

        let monitors_line = read_line(&mut reader);
        assert!(monitors_line.contains(r#""event":"monitors""#));
        assert!(monitors_line.contains(r#""monitors":[]"#));
    }

    #[test]
    fn subscribe_then_a_monitor_change_pushes_a_fresh_monitors_event() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader); // clients
        let _ = read_line(&mut reader); // workspaces
        let _ = read_line(&mut reader); // keyboard_layout
        let _ = read_line(&mut reader); // monitors (empty)

        wm.borrow_mut().set_monitors(vec![{
            let mut m = srdwm_core::Monitor::new(0, "HDMI-A-1", srdwm_core::Rect::new(0, 0, 1920, 1080));
            m.primary = true;
            m
        }]);
        server.poll(&wm);

        // A real monitor existing for the first time also changes which
        // monitor (if any) `WorkspaceInfo::monitor` reports for the
        // now-visible workspace - `None` (no monitor existed to show it)
        // to `Some(0)` - so a "workspaces" event fires too, ahead of
        // "monitors" in `poll`'s own emission order. Drained here, not
        // asserted on: this test is about the monitors event specifically.
        let workspaces_line = read_line(&mut reader);
        assert!(workspaces_line.contains(r#""event":"workspaces""#));

        let line = read_line(&mut reader);
        assert!(line.contains(r#""event":"monitors""#));
        assert!(line.contains(r#""name":"HDMI-A-1""#));
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
        let _initial_monitors = read_line(&mut reader);

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
        let _initial_monitors = read_line(&mut reader);

        wm.borrow_mut().switch_workspace(2);
        server.poll(&wm);

        // Switching touches no window, so the clients list is unchanged --
        // the next line waiting must be the workspaces push, not a
        // clients one that never comes.
        let pushed = read_line(&mut reader);
        assert!(pushed.contains(r#""event":"workspaces""#));
        assert!(pushed.contains(r#""id":2,"name":"2","layout":"dynamic","active":true"#));
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
        let _initial_monitors = read_line(&mut reader);

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
        assert!(line.contains(r#""id":1,"name":"1","layout":"dynamic","active":true"#));
    }

    #[test]
    fn workspaces_and_monitors_agree_on_which_monitor_shows_which_workspace() {
        // `WorkspaceInfo::monitor` and `MonitorInfo::active_workspace` are
        // the same fact from either direction - requested by an AGS peer
        // session so a workspace pill or a per-monitor picker can each
        // read it from whichever side it already has in hand. Default
        // `WindowManager::new()` starts on workspace `1` with no real
        // monitor set up yet; adding one real monitor (id `0`) must make
        // workspace `1`'s own `monitor` read back `0`, and that monitor's
        // `active_workspace` read back `1`.
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

        // Two separate connections, not one reused - a one-shot command's
        // connection closes after its reply (see `a_oneshot_clients_
        // request_still_closes_the_connection_as_before`), so a second
        // command on the same `client` would just hit a broken pipe.
        let mut workspaces_client = UnixStream::connect(&server.path).unwrap();
        let mut workspaces_reader = std::io::BufReader::new(workspaces_client.try_clone().unwrap());
        workspaces_client.write_all(b"{\"cmd\":\"workspaces\"}\n").unwrap();
        server.poll(&wm);
        let workspaces_line = read_line(&mut workspaces_reader);
        assert!(workspaces_line.contains(r#""id":1,"name":"1","layout":"dynamic","active":true,"monitor":0"#));

        let mut monitors_client = UnixStream::connect(&server.path).unwrap();
        let mut monitors_reader = std::io::BufReader::new(monitors_client.try_clone().unwrap());
        monitors_client.write_all(b"{\"cmd\":\"monitors\"}\n").unwrap();
        server.poll(&wm);
        let monitors_line = read_line(&mut monitors_reader);
        assert!(monitors_line.contains(r#""name":"eDP-1""#));
        assert!(monitors_line.contains(r#""active_workspace":1"#));
    }

    #[test]
    fn settings_command_reflects_a_prior_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        // A fresh connection per command - one-shot commands close the
        // connection after replying (see `a_oneshot_clients_request_
        // still_closes_the_connection_as_before`), so a second write on
        // the same client after its first reply hits a closed socket.
        let mut set_client = UnixStream::connect(&server.path).unwrap();
        set_client.write_all(b"{\"cmd\":\"set\",\"key\":\"night_light\",\"value\":true}\n").unwrap();
        server.poll(&wm);
        let _set_reply = read_line(&mut std::io::BufReader::new(set_client.try_clone().unwrap()));

        let mut settings_client = UnixStream::connect(&server.path).unwrap();
        settings_client.write_all(b"{\"cmd\":\"settings\"}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut std::io::BufReader::new(settings_client));
        assert!(!line.contains(r#""event""#), "one-shot command, same plain shape as \"clients\"");
        assert!(line.contains(r#""night_light":true"#));
        assert!(line.contains(r#""reading_mode":false"#), "night_light and reading_mode share one slot, so setting one leaves the other off");
    }

    #[test]
    fn setting_night_light_then_reading_mode_clears_night_light() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        client.write_all(b"{\"cmd\":\"set\",\"key\":\"night_light\",\"value\":true}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut std::io::BufReader::new(client));

        let mut client = UnixStream::connect(&server.path).unwrap();
        client.write_all(b"{\"cmd\":\"set\",\"key\":\"reading_mode\",\"value\":true}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut std::io::BufReader::new(client));

        let mut client = UnixStream::connect(&server.path).unwrap();
        client.write_all(b"{\"cmd\":\"settings\"}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut std::io::BufReader::new(client));
        assert!(line.contains(r#""night_light":false"#));
        assert!(line.contains(r#""reading_mode":true"#));
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
        // `id: 2`, not `1` - `WindowManager::new`'s default workspace is
        // id 1 (already current), and `switch_workspace` no-ops when asked
        // to "switch" to the already-current workspace, so activating `1`
        // would silently test nothing: the assertion below would pass
        // whether or not this dispatch actually worked at all.
        client.write_all(b"{\"cmd\":\"activate_workspace\",\"id\":2}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        assert_eq!(wm.borrow().current_workspace(), 2);
    }

    #[test]
    fn set_output_position_resolves_a_monitor_name_to_its_id() {
        // The CLI (`srd dispatch set output position <name> <x> <y>`)
        // sends a `name`, not an `id`, whenever the caller didn't already
        // have a numeric id handy - `srd monitors` reports names, not an
        // arbitrary index, so a display-arrangement UI reasonably works
        // in those terms.
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_monitors(vec![
            srdwm_core::Monitor::new(0, "EmbeddedDisplayPort-1", srdwm_core::Rect::new(0, 0, 1920, 1080)),
            srdwm_core::Monitor::new(1, "HDMI-A-1", srdwm_core::Rect::new(1920, 0, 1920, 1080)),
        ]);

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"set_output_position\",\"name\":\"HDMI-A-1\",\"x\":1920,\"y\":0}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        let queued = wm.borrow_mut().drain_output_position_requests();
        assert_eq!(queued, vec![(1, 1920, 0)], "must resolve to monitor id 1, not treat the name as missing");
    }

    #[test]
    fn set_output_position_with_an_unknown_name_errors_instead_of_silently_no_opping() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "EmbeddedDisplayPort-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"set_output_position\",\"name\":\"nonexistent-output\",\"x\":0,\"y\":0}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut reader);

        assert!(line.contains(r#""error""#), "an unresolvable name must error, not silently queue nothing");
        assert!(wm.borrow_mut().drain_output_position_requests().is_empty());
    }

    #[test]
    fn set_output_enabled_accepts_a_name_directly() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "HDMI-A-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"set_output_enabled\",\"name\":\"HDMI-A-1\",\"enabled\":false}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        assert_eq!(wm.borrow_mut().drain_output_enable_requests(), vec![("HDMI-A-1".to_string(), false)]);
    }

    #[test]
    fn set_output_enabled_resolves_an_id_to_its_name_for_the_disable_direction() {
        // `id` only ever resolves against the *live* monitor list - fine
        // for disabling something currently connected, which is the only
        // case where a stale-index concern doesn't apply yet.
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(3, "HDMI-A-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"set_output_enabled\",\"id\":3,\"enabled\":false}\n").unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        assert_eq!(wm.borrow_mut().drain_output_enable_requests(), vec![("HDMI-A-1".to_string(), false)]);
    }

    #[test]
    fn set_output_enabled_with_neither_name_nor_a_resolvable_id_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"set_output_enabled\",\"enabled\":true}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut reader);

        assert!(line.contains(r#""error""#));
        assert!(wm.borrow_mut().drain_output_enable_requests().is_empty());
    }

    #[test]
    fn monitors_query_lists_a_disabled_output_alongside_live_ones() {
        // What the AGS peer session asked for directly: a disabled output
        // must not just vanish from `srd monitors` - it needs a row
        // (name + `enabled: false`) to offer turning back on.
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        wm.borrow_mut().set_monitors(vec![srdwm_core::Monitor::new(0, "EmbeddedDisplayPort-1", srdwm_core::Rect::new(0, 0, 1920, 1080))]);
        wm.borrow_mut().set_disabled_monitor("HDMI-A-1".to_string(), srdwm_core::Rect::new(1920, 0, 1920, 1080), srdwm_core::Rect::new(1920, 0, 1920, 1080), false);

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"monitors\"}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut reader);

        assert!(line.contains(r#""name":"EmbeddedDisplayPort-1""#));
        assert!(line.contains(r#""enabled":true"#), "live outputs must report enabled:true");
        assert!(line.contains(r#""name":"HDMI-A-1""#), "disabled output must still be listed");
        assert!(line.contains(r#""enabled":false"#), "the disabled output's own row must say so");
    }

    #[test]
    fn monitors_query_marks_a_split_part_so_a_client_does_not_treat_it_as_a_real_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = IpcServer::bind_in(dir.path(), "test").unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let whole = srdwm_core::Monitor::new(0, "eDP-1", srdwm_core::Rect::new(0, 0, 1920, 1080));
        let mut half = srdwm_core::Monitor::new(1, "eDP-1-1", srdwm_core::Rect::new(0, 0, 960, 1080));
        half.split = true;
        wm.borrow_mut().set_monitors(vec![whole, half]);

        let mut client = UnixStream::connect(&server.path).unwrap();
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        client.write_all(b"{\"cmd\":\"monitors\"}\n").unwrap();
        server.poll(&wm);
        let line = read_line(&mut reader);

        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        let monitors = parsed["monitors"].as_array().unwrap();
        let whole = monitors.iter().find(|m| m["name"] == "eDP-1").unwrap();
        let half = monitors.iter().find(|m| m["name"] == "eDP-1-1").unwrap();
        assert_eq!(whole["split"], false, "an ordinary output must not be marked as a split part");
        assert_eq!(half["split"], true, "a split part must be marked so a client can tell it apart from a real output");
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
        client.write_all(format!("{{\"cmd\":\"move_to_workspace\",\"id\":{id},\"workspace\":2}}\n").as_bytes()).unwrap();
        server.poll(&wm);
        let _ = read_line(&mut reader);

        let wm = wm.borrow();
        assert_eq!(wm.window(id).unwrap().workspace, 2);
        assert_eq!(wm.current_workspace(), 1, "moving a window to a workspace must not also switch to it");
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
