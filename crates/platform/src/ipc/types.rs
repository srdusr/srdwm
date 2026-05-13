//! Response/event payload types this control socket serializes, plus the
//! snapshot functions that build them from `WindowManager` - split out of
//! the original single `ipc.rs` (see `mod.rs`'s own doc comment) purely by
//! concern, no behavior change.

use serde::Serialize;

use srdwm_core::{Direction, GlobalMenu, MenuSource, WindowId, WindowManager};

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct ClientInfo {
    pub(crate) id: u64,
    pub(crate) app_id: String,
    pub(crate) title: String,
    pub(crate) workspace: usize,
    pub(crate) focused: bool,
    pub(crate) minimized: bool,
    pub(crate) visible: bool,
    pub(crate) scratchpad: bool,
    // Whether the layout placed this window (tiled) or the user positioned
    // it directly (floating) - added for an external panel's auto-hide
    // logic, which needs to tell a window the layout placed flush against
    // its own reserved edge (expected, not an overlap) from one the user
    // actually dragged into that space (a real overlap it should react
    // to). Geometry alone can't distinguish the two: a tiled window's edge
    // sitting exactly at the usable-area boundary looks identical, in x/y/
    // width/height terms, to a floating window a human dragged flush
    // against it.
    pub(crate) floating: bool,
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
    pub(crate) layout: String,
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
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    // The window's global-menu D-Bus address (bus name + object paths),
    // if it has exported one - `null` for the common case of a window
    // with no menu at all, which a consumer should treat exactly like a
    // missing field: no menu to show, not an error. See `srdwm_core::
    // GlobalMenu`'s own doc comment for why this is an address and never
    // the menu's actual content.
    pub(crate) global_menu: Option<GlobalMenuInfo>,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct GlobalMenuInfo {
    pub(crate) bus_name: String,
    pub(crate) menu_path: Option<String>,
    pub(crate) app_path: Option<String>,
    pub(crate) window_path: Option<String>,
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
    pub(crate) source: &'static str,
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
pub(crate) struct ClientsResponse {
    pub(crate) clients: Vec<ClientInfo>,
}

#[derive(Serialize)]
pub(crate) struct MonitorsResponse {
    pub(crate) monitors: Vec<MonitorInfo>,
}

/// One key binding, for `srd keybindings`. `description` is empty when the
/// binding did not give one - an empty string rather than `null`, so a
/// consumer can render it without a branch.
#[derive(Serialize)]
pub(crate) struct KeybindingInfo {
    pub(crate) combo: String,
    pub(crate) description: String,
    /// `false` when the config binds this combo but the compositor does not
    /// actually intercept it - see `srdwm_core::KeyBinding::grabbed`. A UI
    /// listing bindings should say so rather than showing a shortcut that
    /// silently does nothing.
    pub(crate) grabbed: bool,
}

#[derive(Serialize)]
pub(crate) struct KeybindingsResponse {
    pub(crate) keybindings: Vec<KeybindingInfo>,
}

#[derive(Serialize)]
pub(crate) struct WorkspacesResponse {
    pub(crate) workspaces: Vec<WorkspaceInfo>,
}

/// One `pid`/pinned-window pair - `WindowManager::all_pinned_windows`'s
/// own doc comment. `id` matches the plain `WindowId` every other
/// dispatch already reads/writes, not a separate type.
#[derive(Serialize)]
pub(crate) struct PinnedInputInfo {
    pub(crate) pid: i32,
    pub(crate) id: WindowId,
}

/// `"pinned_inputs"`'s one-shot reply - every pid Multi-cursor Phase 2
/// (`srd dispatch pin input`) currently has pinned to a window, and which
/// one. Added because pinning had no readback at all: a caller could ask
/// to pin a window blind, but never confirm the pin actually took, or
/// list what's pinned right now without already knowing which pids to
/// ask about.
#[derive(Serialize)]
pub(crate) struct PinnedInputsResponse {
    pub(crate) pinned: Vec<PinnedInputInfo>,
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
pub(crate) struct SettingsResponse {
    pub(crate) shadows: bool,
    pub(crate) close_focus_follows_workspace: bool,
    pub(crate) rounded_corners: Option<bool>,
    pub(crate) animations: bool,
    pub(crate) night_light: bool,
    pub(crate) reading_mode: bool,
    /// `WindowManager::phone_mode`'s own doc comment: read-only here so a
    /// shell panel (AGS) can adapt its own chrome to the same
    /// single-app-at-a-time signal srdwm's own placement already uses,
    /// without a second, separate way to ask "is this a phone-shaped
    /// session".
    pub(crate) phone_mode: bool,
    /// `WindowManager::multi_cursor_enabled`'s own doc comment.
    pub(crate) multi_cursor: bool,
    /// The theme/tiling values `srd set` can already change live
    /// (`border_width`, `border_color`, `corner_radius`, `decoration_
    /// mode`, `gap_inner`, `gap_outer`, `master_ratio`, `master_count`)
    /// had no way to read the *current* value back at all - a settings
    /// panel could set any of these blind, but not honestly show its own
    /// control's starting position, or confirm a set actually took.
    /// Flagged directly by the AGS peer session as the common shape behind
    /// several separate gaps at once: "a control whose value cannot be
    /// read back is a control that lies on every restart."
    pub(crate) border_width: u32,
    /// `#rrggbb`, matching the exact string shape `srd set border_color`
    /// itself accepts (`srdwm_core::parse_hex_color`'s own format) - a
    /// caller can feed this straight back into another `set` unchanged.
    pub(crate) border_color: String,
    pub(crate) corner_radius: u32,
    /// `true` when new windows default to a server-drawn titlebar
    /// (`general.decoration_mode`/`srd set decoration_mode`'s own "server"
    /// value), `false` for "client" (CSD-only default).
    pub(crate) decoration_mode_server: bool,
    pub(crate) gap_inner: u32,
    pub(crate) gap_outer: u32,
    /// `TilingConfig::master_ratio`/`master_count` - see `WindowManager::
    /// adjust_master_ratio_for_drag`'s own doc comment for the live
    /// interactive path (a resize-drag on the master/stack boundary) that
    /// also mutates this, in addition to `srd set master_ratio`/
    /// `master_count`.
    pub(crate) master_ratio: f32,
    pub(crate) master_count: usize,
    /// `WindowManager::per_monitor_workspaces`'s own doc comment - `srd
    /// set per_monitor <bool>`'s readback.
    pub(crate) per_monitor: bool,
    /// `ThemeConfig::traffic_light_buttons`'s readback, as the same
    /// `"traffic_lights"`/`"traditional"` string `srd set button_style`
    /// itself accepts.
    pub(crate) button_style: String,
    /// `"dynamic"`/`"fixed"` string `srd set button_mode` accepts --
    /// `ThemeConfig::dynamic_buttons`'s readback, same shape and reason as
    /// `button_style` directly above.
    pub(crate) button_mode: String,
    /// `ThemeConfig::buttons_left`'s readback, as `"left"`/`"right"`.
    pub(crate) button_side: String,
    /// `ThemeConfig::button_order`'s readback - `null` when unset (the
    /// built-in default for whichever side `button_side` selects), the
    /// same `"close,minimize,maximize"` string shape `srd set button_
    /// order` accepts otherwise.
    pub(crate) button_order: Option<String>,
    pub(crate) title_centered: bool,
    pub(crate) button_glyph_always: bool,
    pub(crate) desktop_icons: bool,
    pub(crate) desktop_icons_all_monitors: bool,
}

/// `"keyboard_layout"`'s one-shot reply shape - the active XKB layout's
/// own name (e.g. `"English (US)"`), whatever `WindowManager::keyboard_
/// layout` currently holds. Added for an AGS peer session's keyboard-
/// layout badge, the last Hyprland-only control left in their shell
/// (`hyprctl devices -j` had no srdwm equivalent at all before this).
#[derive(Serialize)]
pub(crate) struct KeyboardLayoutResponse {
    pub(crate) layout: String,
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
pub(crate) struct WorkspaceInfo {
    pub(crate) id: usize,
    pub(crate) name: String,
    pub(crate) layout: String,
    pub(crate) active: bool,
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
    pub(crate) monitor: Option<srdwm_core::MonitorId>,
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
pub(crate) struct MonitorInfo {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) primary: bool,
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
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    // The output's true full rect, ignoring any exclusive zone - what
    // `toggle_fullscreen` targets, and what a display-arrangement/output-
    // positioning UI should read (`set_output_position` moves this rect,
    // not the work-area one above). Equal to x/y/width/height above when
    // nothing on this monitor currently reserves any space at all, which
    // is exactly why the mistake above is easy to make and easy to miss
    // in testing - it only shows up once something (a bar, a dock) is
    // actually reserving space.
    pub(crate) full_x: i32,
    pub(crate) full_y: i32,
    pub(crate) full_width: u32,
    pub(crate) full_height: u32,
    // `true` for every genuinely live output. `false` marks an
    // administratively-disabled-but-still-connected one (`srd dispatch
    // set output enabled <name> false`) - requested directly by the AGS
    // peer session so their monitor-layout panel has a name/row to
    // re-enable by, rather than the output vanishing from this list
    // entirely the moment the control that turns it off is used. A
    // genuinely *unplugged* output, disabled or not, still just
    // disappears from this list as before - `enabled: false` means "off,
    // but still here to turn back on", not "not connected".
    pub(crate) enabled: bool,
    // `true` when this entry is one part of a real output divided by
    // `srd.monitor.split` - not a second `wl_output`, not a second
    // physical connector. See `srdwm_core::monitor::Monitor::split`'s own
    // doc comment: a display-arrangement UI should not offer to move or
    // extend a physical arrangement onto one of these, since there is no
    // independent output behind it, only a placement-only division of a
    // real one.
    pub(crate) split: bool,
    // This output's real scale factor - `1.0` for an unscaled one. See
    // this struct's own doc comment for what it converts between and why.
    pub(crate) scale: f64,
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
    pub(crate) active_workspace: usize,
    // `true` for a fully virtual/headless output (`srd dispatch create
    // fake-monitor`) - a real `wl_output` global with no DRM connector
    // behind it. Requested directly by the AGS peer session after a fake
    // monitor's `wl_output` caused a real live incident: it looks like an
    // ordinary new physical monitor to any client watching the core
    // Wayland registry (unlike `wlr-output-management-v1`, which already
    // excludes it), so AGS's own remembered-layout restore treated one
    // appearing as a real hotplug and repositioned the *real* monitor to
    // make room for it. Before this field existed, AGS's only option was
    // matching the name against `^FAKE-` - this is the real
    // discriminator that pattern was standing in for. `#[serde(rename)]`
    // rather than a field literally named `virtual` because that word is
    // a reserved identifier in Rust.
    #[serde(rename = "virtual")]
    pub(crate) is_virtual: bool,
}

/// Pushed to every subscriber (and used as `subscribe`'s own initial
/// reply) instead of `ClientsResponse`'s plain `{"clients": [...]}"` shape,
/// so every line a subscriber ever reads on that connection looks the
/// same - no special-casing the first one. Not used for the one-shot
/// `"clients"` command, whose response shape predates this and stays as-is
/// for existing polling consumers (`crates/ctl`, any external script).
#[derive(Serialize)]
pub(crate) struct ClientsEvent<'a> {
    pub(crate) event: &'static str,
    pub(crate) clients: &'a [ClientInfo],
}

/// `WorkspaceInfo`'s equivalent of `ClientsEvent` - a distinct event on
/// the same `subscribe` connection (one JSON object per line, `"event"`
/// says which), not folded into `ClientsEvent`: workspaces and windows
/// change independently (switching workspace touches no window; a window
/// closing touches no workspace), so diffing and broadcasting them
/// together would push a workspace-shaped payload on every window change
/// and vice versa for no reason.
#[derive(Serialize)]
pub(crate) struct WorkspacesEvent<'a> {
    pub(crate) event: &'static str,
    pub(crate) workspaces: &'a [WorkspaceInfo],
}

/// A third, independently-diffed event on the same `subscribe` connection
/// - see `WorkspacesEvent`'s own doc comment for why this isn't folded
/// into either of the other two: a layout cycle touches no window and no
/// workspace, so it needs its own change-diff to avoid pushing an
/// unrelated payload on every unrelated change.
#[derive(Serialize)]
pub(crate) struct KeyboardLayoutEvent<'a> {
    pub(crate) event: &'static str,
    pub(crate) layout: &'a str,
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
pub(crate) struct MonitorsEvent<'a> {
    pub(crate) event: &'static str,
    pub(crate) monitors: &'a [MonitorInfo],
}

/// The same per-window snapshot both `"clients"` and `"subscribe"`/the
/// change-diff in `IpcServer::poll` build - pulled out so the two can
/// never silently drift into reporting different fields.
pub(crate) fn client_snapshot(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> Vec<ClientInfo> {
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
pub(crate) fn workspace_snapshot(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> Vec<WorkspaceInfo> {
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
pub(crate) struct OkResponse {
    pub(crate) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<&'static str>,
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
pub(crate) fn resolve_monitor_id(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>, id: Option<WindowId>, name: Option<&str>) -> Option<srdwm_core::MonitorId> {
    match id {
        Some(id) => Some(id as srdwm_core::MonitorId),
        None => name.and_then(|name| wm.borrow().monitors().iter().find(|m| m.name == name).map(|m| m.id)),
    }
}

pub(crate) fn monitor_snapshot(wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> Vec<MonitorInfo> {
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
        is_virtual: m.is_virtual,
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
        // A fake monitor is never administratively disabled/re-enabled --
        // see `virtual_heads.rs`'s own module doc comment - so this
        // branch (disabled-but-still-connected outputs) can never be one.
        is_virtual: false,
    });
    live.chain(disabled).collect()
}

pub(crate) fn ok() -> Vec<u8> {
    serde_json::to_vec(&OkResponse { ok: true, error: None }).unwrap_or_default()
}

pub(crate) fn err(msg: &'static str) -> Vec<u8> {
    serde_json::to_vec(&OkResponse { ok: false, error: Some(msg) }).unwrap_or_default()
}

/// The `move_window` dispatch's own direction-name parser - `crates/
/// config`'s own `parse_direction` (used by `srd.window.move`) isn't
/// reusable here: different crate, and it returns an `mlua::Result` tied
/// to the Lua binding's own error type. Same four names, same "small
/// duplication across crate boundaries beats a cross-crate dependency for
/// four match arms" tradeoff every other bit of shared naming in this
/// codebase already accepts.
pub(crate) fn parse_direction(name: &str) -> Option<Direction> {
    match name {
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        _ => None,
    }
}
