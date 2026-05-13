use crate::geometry::Rect;
use crate::layout::{Layout, MasterStackLayout, NoOpLayout, TilingConfig};
use crate::monitor::{DisabledMonitor, Monitor, MonitorId, MonitorSplit};
use crate::placement::{centered_in, PlacementConfig, SmartPlacement, SnapZoneKind, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, SNAP_FLYOUT_EDGE};
use crate::rules::WindowRule;
#[cfg(test)]
use crate::rules::{WindowMatch, WindowRuleActions};
use crate::lock_config::LockConfig;
use crate::theme::ThemeConfig;
use crate::window::{likely_draws_own_titlebar, ResizeEdge, TitlebarHit, Window, WindowId, RESIZE_MARGIN};
use crate::workspace::{Workspace, WorkspaceId};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// A whole-screen colour treatment, drawn by each Wayland backend as a
/// translucent full-output overlay above every window but below the
/// cursor - see `srdwm_wayland::color_filter` for the actual overlay
/// colour/alpha each variant maps to, and why an alpha-blended overlay
/// rather than a true per-pixel shader was chosen at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorFilter {
    #[default]
    None,
    /// Warm tint, reduces perceived blue light. Ported from a Hyprland
    /// `decoration:screen_shader` config that multiplied the framebuffer
    /// by `vec3(1.0, 0.82, 0.60)`.
    NightLight,
    /// Desaturating tint, for reduced visual noise during long-form
    /// reading. Ported from a Hyprland `decoration:screen_shader` config
    /// that replaced every pixel with its own luminance (flat grayscale).
    ReadingMode,
}

struct DragState {
    window: WindowId,
    start_x: i32,
    start_y: i32,
    orig: Rect,
    /// Where the pointer was on the last `update_drag` tick, global space.
    /// Seeded from the drag's own start point so it is never meaningless,
    /// even for a drag that ends before a single motion event arrives.
    ///
    /// Only the *pointer* can answer "is the user reaching for the top of
    /// the screen right now" - `orig`/`Window::geometry` answer "where is
    /// the window", which is a different question during a drag, because
    /// the window hangs below the grab point by however far down its
    /// titlebar the user took hold of it. See `drag_top_edge_monitor`.
    last_x: i32,
    last_y: i32,
}

struct ResizeState {
    window: WindowId,
    edge: ResizeEdge,
    start_x: i32,
    start_y: i32,
    orig: Rect,
    /// `self.tiling.master_ratio` at the moment this resize started --
    /// unconditionally captured (cheap, one `f32` copy) even for a resize
    /// that turns out not to touch it, the same way `orig` itself is
    /// captured regardless of whether the drag ends up floating or tiled.
    /// See `WindowManager::adjust_master_ratio_for_drag`'s own doc comment
    /// for why a live tiling resize needs its own *starting* ratio, not
    /// just the live one mutated in place tick by tick.
    orig_master_ratio: f32,
    /// `WindowManager::tiling_ratio_drag`'s result, decided once here,
    /// *before* `start_resize` calls `focus_window` - `Some(membership)`
    /// for a tiling master/stack ratio drag, `None` for an ordinary
    /// (floating, or non-boundary-edge) resize. See that method's own doc
    /// comment for why this has to be captured now rather than
    /// re-derived later: focusing the target re-stacks it in `self.order`,
    /// the exact list membership is read from, and re-deriving after that
    /// point silently answers for the wrong window.
    ratio_drag_ids: Option<Vec<WindowId>>,
}

/// The platform-independent core of srdwm: owns window/workspace/monitor
/// state and layout policy. Backends (X11, Wayland, ...) drive this via
/// `add_window`/`remove_window`/input events, and apply the `Rect`s it
/// computes back onto real surfaces.
pub struct WindowManager {
    windows: HashMap<WindowId, Window>,
    order: Vec<WindowId>,
    focused: Option<WindowId>,
    monitors: Vec<Monitor>,
    /// Backend-agnostic "please move this output" requests, queued by
    /// `request_output_position` (an IPC `set_output_position` dispatch is
    /// the only caller today) and drained by whichever backend actually
    /// owns real output hardware (`drain_output_position_requests`) on its
    /// own next poll. Core has no way to reposition a real `Output` itself
    /// - monitor geometry flows one direction, backend into core, via
    /// `set_monitors` - so a request from an IPC caller (an AGS display-
    /// settings panel wanting to set up monitor mirroring, concretely) has
    /// to cross back over that boundary the same indirect way window
    /// geometry changes do in the other direction: queued here, applied by
    /// the backend, and `set_monitors` reports the result back on the
    /// backend's next monitor query, same as any other hotplug/reconfigure.
    output_position_requests: Vec<(MonitorId, i32, i32)>,
    /// Same cross-boundary-request pattern as `output_position_requests`
    /// just above, for Phase 2 of the multi-cursor plan - pinning a
    /// virtual pointer object (identified by the owning client's pid, not
    /// an opaque per-object id nothing outside the Wayland backend could
    /// ever learn) to a specific window. See `input_pin.rs`'s own doc
    /// comment.
    pin_input_requests: Vec<(i32, Option<WindowId>)>,
    /// `pid`'s *actual current* pin state, as last reported by `Comp
    /// State::set_virtual_pointer_pin` once it's genuinely applied --
    /// distinct from `pin_input_requests` above, which is a one-shot queue
    /// drained and forgotten the moment the backend picks it up. See
    /// `set_pinned_window`'s own doc comment.
    pinned_windows: HashMap<i32, WindowId>,
    /// Same cross-boundary-request pattern, for fake (fully virtual, no
    /// real hardware) monitors - see `fake_monitor.rs`'s own doc comment
    /// and `crates/wayland/src/udev/virtual_heads.rs`'s module doc
    /// comment for the full design.
    create_fake_monitor_requests: Vec<(String, u32, u32)>,
    remove_fake_monitor_requests: Vec<String>,
    /// Same cross-boundary-request pattern as `output_position_requests`
    /// just above, for enable/disable - see `request_output_enabled`'s
    /// own doc comment for why this is keyed by name, not `MonitorId`.
    output_enable_requests: Vec<(String, bool)>,
    /// The opposite direction of `output_enable_requests`: not a request
    /// *to* the backend, but the backend *reporting* an administratively-
    /// disabled-but-still-connected output's last-known state, purely for
    /// listing purposes - see `set_disabled_monitor`'s own doc comment
    /// for why this deliberately never touches `monitors`/real placement
    /// at all.
    disabled_monitors: HashMap<String, DisabledMonitor>,
    /// `srd.monitor.split(name, parts, direction)` requests, by connector
    /// name - read by a backend's own `monitors()` query to divide one
    /// real output's rectangle into several logical `Monitor` entries. See
    /// [`MonitorSplit`]'s own doc comment for what this deliberately does
    /// and does not give a client (no new `wl_output`).
    monitor_splits: HashMap<String, MonitorSplit>,
    /// Same cross-boundary-request pattern as `output_position_requests`
    /// above - an IPC `set_monitor_split` dispatch (the live CLI/IPC path
    /// for `srd.monitor.split`) mutating `monitor_splits` directly is not
    /// enough on its own: `monitors` above is a passive cache, only
    /// refreshed when a backend re-queries and calls `set_monitors` again
    /// (a real hotplug, or another queued request's own drain site pushing
    /// the same "just go recompute" `MonitorAdded` event - see `output_
    /// position_requests`' own drain site for the exact precedent). A
    /// direct mutation with nothing to trigger that requery left `srd
    /// monitors` reporting the pre-split layout indefinitely, live-
    /// reproduced the first time this was tried: `{"ok":true}` came back,
    /// but the very next `srd monitors` still showed one whole, unsplit
    /// output. Queued here instead so the backend's own drain site can
    /// apply the split *and* push that same recompute signal, exactly like
    /// `output_position_requests` already does.
    monitor_split_requests: Vec<(String, u32, bool)>,
    /// `srd.monitor.scale(name, factor)` requests, by connector name --
    /// read once by a backend when it brings a head up (startup, hotplug,
    /// or re-enable), so a physically large, low-DPI monitor can run
    /// below `1.0` to show more logical desktop space instead of just
    /// larger text at the same pixel count. srdwm otherwise always drove
    /// every real output at a hardcoded `1.0`, with no way to change that
    /// short of a client speaking wlr-output-management itself.
    monitor_scales: HashMap<String, f64>,
    /// Same cross-boundary-request pattern as `output_position_requests`
    /// just above - core has no way to actually blank the screen and
    /// start drawing srdwm's own lock UI itself (that's real compositor
    /// rendering, backend-owned), so an IPC `"lock"` dispatch queues the
    /// intent here via `request_lock` and whichever backend is running
    /// drains it (`drain_lock_request`) on its own next poll.
    lock_requested: bool,
    /// Same cross-boundary-request pattern as `output_position_requests`
    /// again - see `capture::CaptureRequest`'s own doc comment for why
    /// this exists at all (a workspace switcher needing a thumbnail of a
    /// workspace that isn't the one currently presented, which no Wayland
    /// screencopy protocol can see).
    capture_requests: Vec<capture::CaptureRequest>,
    workspaces: Vec<Workspace>,
    /// The shared-mode value, used directly when `per_monitor_workspaces`
    /// is `false` (the default - unlike Hyprland, srdwm's original design
    /// has no notion of an independent workspace set per monitor;
    /// switching workspace changes what's visible on every screen at
    /// once). Still meaningful even when `per_monitor_workspaces` is `true`
    /// - it's the fallback `workspace_for_monitor` returns for a monitor
    /// that has never had its own workspace switched independently yet,
    /// and what a plain `current_workspace()` call reports either way. See
    /// `visible_windows`'s doc comment for the filter this actually drives.
    current_workspace: WorkspaceId,
    /// Whichever workspace was current immediately before the current one
    /// became current - see `switch_workspace`'s doc comment.
    previous_workspace: WorkspaceId,
    /// Read from `workspace.auto_back_and_forth`. When set, switching to
    /// the workspace that's already active switches to `previous_workspace`
    /// instead - sway's `workspace_auto_back_and_forth` behavior, a quick
    /// "jump back to whatever I was just on" toggle on a single keybinding.
    pub auto_back_and_forth: bool,
    /// Read from `workspace.per_monitor` - `false` (the default) keeps
    /// srdwm's original single-shared-workspace design exactly as it was;
    /// `true` switches to Hyprland/niri-style independent per-monitor
    /// workspace sets, where each monitor tracks and displays its own
    /// current workspace, switchable without affecting any other monitor.
    /// Explicitly requested as a configurable choice, not a hardcoded
    /// switch to one model or the other - see `workspace_for_monitor` and
    /// `switch_workspace_on_monitor` for what this actually gates.
    pub per_monitor_workspaces: bool,
    /// Read from `monitor.primary_layout`/`monitor.secondary_layout` --
    /// validated/defaulted config keys that were never read anywhere
    /// before (same dead-config shape as `general.default_layout`'s own
    /// siblings). Empty string means "not set, no override". Applied by
    /// `set_monitors` to whichever workspace `workspace_for_monitor`
    /// resolves for each connected monitor - which only ever *differs*
    /// between monitors when `per_monitor_workspaces` is `true` (every
    /// monitor shares one workspace otherwise, so a primary/secondary
    /// split has nothing distinct to apply to and is skipped).
    pub primary_layout: String,
    pub secondary_layout: String,
    /// Each monitor's own current workspace, when `per_monitor_workspaces`
    /// is `true`. A monitor with no entry here yet (never had its
    /// workspace switched independently - e.g. right after the mode was
    /// turned on, or a newly connected monitor) falls back to
    /// `current_workspace`, the same shared value shared-mode always uses
    /// - see `workspace_for_monitor`. Unused, and left empty, whenever
    /// `per_monitor_workspaces` is `false`.
    monitor_workspaces: HashMap<MonitorId, WorkspaceId>,
    next_workspace_id: WorkspaceId,
    next_window_id: WindowId,
    layouts: HashMap<String, Box<dyn Layout>>,
    pub tiling: TilingConfig,
    pub placement: PlacementConfig,
    /// Feeds `SmartPlacement::place`'s own `cascade_step` - advances on
    /// every real cascade/grid placement, session-long, never reset by a
    /// window closing. See `SmartPlacement::cascade`'s own doc comment
    /// for the reported "every window opens in the same spot" bug this
    /// exists to fix. A `Cell`, not a plain field: `add_window`'s own
    /// `target_monitor` is a `&Monitor` borrowed from `self.monitors` and
    /// stays alive across the same call that needs to bump this counter,
    /// so a plain `&mut self` write there would conflict with that live
    /// immutable borrow - interior mutability sidesteps it without
    /// restructuring the borrow, the same reasoning any of this
    /// compositor's other `Rc<RefCell<...>>`-style shared-mutation points
    /// already accept.
    next_cascade_step: std::cell::Cell<u32>,
    /// Whether geometry changes made via `toggle_maximize`/`toggle_fullscreen`
    /// should be animated. Read from `general.animations`; a backend's open
    /// animation is gated on this too, since core has no notion of "open".
    pub animations_enabled: bool,
    /// Tween duration in milliseconds, read from `general.animation_duration`.
    pub animation_duration_ms: u32,
    /// Whether windows get a drop shadow. Read from `general.shadows`. A
    /// maximized or fullscreen window never gets one regardless of this --
    /// see the Wayland backend's shadow render call site - so this only
    /// ever turns it off entirely, not on for those.
    pub shadows_enabled: bool,
    /// Whether closing your own focused window is allowed to fall back to
    /// a window on a *different* workspace and switch you there to follow
    /// it, when nothing else is left on your current one. Read from
    /// `general.close_focus_follows_workspace`. `false` (the default)
    /// matches every mainstream desktop (Windows/GNOME/macOS never change
    /// your active workspace just because a window closed) - `remove_
    /// window`'s own fallback then only ever considers a same-workspace
    /// window, leaving focus at `None` if there isn't one, rather than
    /// picking whatever window was next in the *global* most-recently-
    /// focused order regardless of which workspace it happens to be on.
    /// Reported live as windows closing and "teleporting" the user to a
    /// previous workspace - exactly this: the global fallback picking a
    /// background window elsewhere, then `focus_window`'s own (separate,
    /// correct, and unrelated to this setting) "switch workspace to match
    /// the newly focused window" side effect following it there. `true`
    /// restores that original always-follow behaviour for anyone who
    /// wants it.
    pub close_focus_follows_workspace: bool,
    /// Width, in pixels, of the resize grab band along a window's edges,
    /// read from `general.resize_margin`. See [`crate::window::RESIZE_MARGIN`]'s
    /// doc comment for the default and why it's what it is.
    pub resize_margin: i32,
    /// Whether a decorated window's content rounds its bottom two corners
    /// to match the titlebar's own curve (an undecorated/CSD window rounds
    /// all four). Read from `general.rounded_corners` - `None` when the
    /// user's config never touched that key at all (deliberately *not*
    /// defaulted in `crates/config`, unlike every other `general.*` key),
    /// so each backend can fall back to its own default rather than one
    /// baked in here: GLES/winit defaults on, udev/Pixman defaults off
    /// (an untested-on-real-hardware per-frame CPU cost for content that
    /// redraws constantly - see `crates/wayland/src/rounded_corners.rs`).
    /// `Some(_)` only when the user explicitly set it, and wins either way.
    pub rounded_corners_enabled: Option<bool>,
    /// Whether the udev backend attempts real GBM+EGL+`DrmCompositor` GPU
    /// rendering instead of the default, always-available software
    /// (Pixman/dumb-buffer) path - read from `general.gpu`, `false` by
    /// default (unlike `rounded_corners_enabled`'s `Option`, this has one
    /// unambiguous default regardless of backend: GPU rendering is udev-
    /// only and experimental everywhere, so "off" is correct whether or
    /// not the eventual backend even has a GPU path at all). `true` here
    /// only ever *attempts* it - `udev::gpu::probe` still falls back to
    /// the untouched software path on any failure at any step (no GBM
    /// device, no atomic-modesetting support, a software-only EGL
    /// renderer, ...), logged but never fatal, so setting this on a
    /// machine or VM without real GPU/KMS support costs nothing beyond
    /// the one failed probe at startup. `SRDWM_GPU=1` (an env var, unset
    /// by default) remains a separate, lower-level override for testing
    /// without touching config - `udev::platform::connect` attempts the
    /// probe if *either* this or the env var says to.
    pub gpu_enabled: bool,
    /// Read from `general.multi_cursor` - `false` by default. Gates
    /// whether the udev backend renders one extra cursor sprite per
    /// *other* physical pointer device that's recently moved (`UdevState::
    /// secondary_cursors`, "Multi-cursor Phase 1"). Off by default because
    /// live use found the un-gated version actively confusing rather than
    /// useful: real hardware routinely reports what is really one mouse
    /// as more than one distinct libinput device (a side-button/scroll
    /// cluster on its own HID path, concretely), so an always-on second
    /// sprite showed up uninvited and, since nothing else ever moved that
    /// phantom device again, sat frozen on screen with no way to control
    /// or dismiss it - reported live as exactly that: "I see two cursors
    /// and can't even control the other one". The two scenarios this
    /// feature actually exists for are unaffected by this being off:
    /// genuinely using two input devices at once is now something to
    /// opt into rather than be surprised by, and "an agent controls a
    /// window without interrupting me" is Multi-cursor Phase 2's own job
    /// (`crates/wayland/src/virtual_pointer.rs`'s pinned delivery), which
    /// never shows a visible cursor at all - it was never blocked on
    /// this flag to begin with.
    pub multi_cursor_enabled: bool,
    /// Read from `general.phone_mode` - `false` by default. Optional
    /// single-app-at-a-time placement policy for a phone-shaped display:
    /// see `add_window`'s own use of this (a new window defaults to
    /// maximized instead of floating/tiled small, unless a rule says
    /// otherwise) for the concrete effect. Deliberately just a placement
    /// default, not a distinct "mode" this crate tracks any other state
    /// for - toggling it live via `srd set phone_mode <bool>` only
    /// changes how the *next* new window opens, same as any other
    /// default-policy config value (`general.animations`, `general.
    /// shadows`) already behaves, not a live re-layout of every window
    /// already open. Also exposed read-only via `srd settings` so a shell
    /// panel (AGS, concretely) can adapt its own chrome to the same
    /// signal without needing a second, separate way to ask "is this a
    /// phone-shaped session" - the actual "optional phone mode for AGS"
    /// half of this ask is real work in *that* project, not this one;
    /// this is the one thing srdwm itself needed to add so AGS has
    /// something real to read.
    pub phone_mode: bool,
    /// Whether srdwm draws real desktop icons (Home/Computer/Trash plus one
    /// per real `~/Desktop` entry) on the primary output's wallpaper --
    /// read from `general.desktop_icons`. Unlike `gpu_enabled`, this
    /// defaults to `true`: a directly user-requested, purely visual
    /// feature with no hardware-support question to hedge against, not an
    /// experimental backend path that needs an opt-in safety net.
    pub desktop_icons_enabled: bool,
    /// Whether desktop icons are mirrored onto every enabled monitor's own
    /// corner, or only drawn on the primary monitor - read from `general.
    /// desktop_icons_all_monitors`. Defaults to `true`, matching real macOS
    /// convention (each display gets its own Desktop icons view) rather
    /// than the older Windows-style "icons live on monitor 1 only" - a
    /// directly reported gap ("in other monitor it's not showing the
    /// desktop icons"), not a hardware question to hedge on like `gpu_
    /// enabled`. The same underlying icon set/cells are shared across every
    /// mirror: dragging a copy on one monitor moves the one real icon,
    /// which then shows in its new cell on every monitor it's mirrored to.
    pub desktop_icons_all_monitors: bool,
    /// Static minimum space reserved on each edge of every monitor,
    /// logical pixels, read from `general.reserve_top`/`_bottom`/`_left`/
    /// `_right` - `0` (no static reservation) by default. Exists for the
    /// gap between "the compositor starts rendering/placing things" and
    /// "the bar/dock has actually connected and called `set_exclusive_
    /// zone`": a layer-shell client's own reserved strip only exists once
    /// that client has mapped a real surface, which is reliably *after*
    /// this compositor's own first render pass and first-window placement
    /// decisions (autostart spawns the compositor's own children, which
    /// then have to connect, negotiate, and commit before their zone is
    /// real). Desktop icons already re-derive their own origin every frame
    /// so they self-correct once the real zone lands (see `ensure_desktop_
    /// icons`'s own doc comment) - but a *window* placed in that gap gets
    /// a one-time placement decision, not a continuously-corrected one, so
    /// it can end up spawned under where the bar will render, with nothing
    /// to nudge it out afterward. Set this to the bar/dock's own known
    /// height/width (whatever `~/.config/ags` or another panel actually
    /// reserves) so every usable-area computation (`Platform::monitors()`)
    /// already accounts for it from the very first call, before any real
    /// client has connected at all. Takes the *larger* of this and
    /// whatever real exclusive zone currently exists per edge, never the
    /// smaller - so a real, larger bar still wins once it registers, and
    /// this is a floor, not a competing claim.
    pub reserve_top: u32,
    pub reserve_bottom: u32,
    pub reserve_left: u32,
    pub reserve_right: u32,
    /// External program desktop icons open into, read from `general.
    /// file_manager`. Empty (the default) means "shell out to `xdg-open
    /// <path>`" - the de-facto standard dispatcher to whatever the user's
    /// own `mimeapps.list` already names, present on essentially every
    /// Linux/BSD desktop regardless of which file manager is installed.
    /// Set means "shell out to `<file_manager> <path>` instead", the same
    /// "user names a program, srdwm shells out to it" shape `general.
    /// terminal`-style keybindings already use from Lua (`srd.spawn`), just
    /// read from config instead of a keybinding script since desktop icons
    /// have no Lua callback of their own to run.
    pub file_manager: String,
    /// Whether a single left-click opens a desktop icon instead of the
    /// classic double-click, read from `general.desktop_icon_single_click`.
    /// `false` (double-click) by default, matching Windows/macOS/most
    /// Linux desktops' own default; some environments (older GNOME, some
    /// file managers) default the other way, hence this being a real
    /// config option rather than a hardcoded choice.
    pub desktop_icon_single_click: bool,
    /// External program the bare-desktop menu's "Open Terminal Here"
    /// action shells out to (with `~/Desktop` as its working directory),
    /// read from `general.terminal`. Empty (the default) tries a short
    /// list of common terminals on `$PATH` - there's no `xdg-open`-
    /// equivalent for "a shell", unlike `file_manager`.
    pub terminal: String,
    /// The whole-screen colour treatment currently active (night light's
    /// warm tint or reading mode's desaturation), live-settable via `srd
    /// set night_light`/`srd set reading_mode` - see [`ColorFilter`]. Off
    /// by default; the two are mutually exclusive by construction (one
    /// enum, not two independent bools), matching the ported Hyprland
    /// scripts this replaces, which pointed the same single
    /// `screen_shader` slot at one file or the other.
    pub color_filter: ColorFilter,
    /// Whether hovering a window (no click needed) focuses it, read from
    /// `general.focus_follows_mouse`. Off by default - matches
    /// `general.focus_follows_mouse`'s own documented default, and every
    /// desktop's convention of click-to-focus unless a user explicitly
    /// opts into the classic X11 sloppy-focus behaviour.
    pub focus_follows_mouse: bool,
    /// Whether hover-driven focus (above) also raises the window, not just
    /// focuses it - read from `general.auto_raise`. Meaningless (never
    /// consulted) while `focus_follows_mouse` is off, since a plain click
    /// already raises unconditionally regardless of this.
    pub auto_raise: bool,
    /// Default decoration colours and border width, read from `theme.colors.*`/
    /// `theme.decorations.*`. See `ThemeConfig`'s own doc comment.
    pub theme: ThemeConfig,
    /// Read from `theme.lock.*`. See `LockConfig`'s own doc comment for
    /// why this isn't just folded into `theme` above.
    pub lock: LockConfig,
    /// `general.maximize_covers_dock`: whether maximize runs to the bottom
    /// of the screen, under a bottom-anchored dock, rather than stopping
    /// above it. A top bar's own reservation is always honoured either
    /// way. Default `true` - see `input::layers::maximize_geometry_for`.
    pub maximize_covers_dock: bool,
    /// Every key binding as `(combo, description)`, published by the main
    /// loop after the config loads and after every reload.
    ///
    /// Core neither owns nor interprets these - it has no Lua state and
    /// never dispatches a key itself. It holds them so the IPC layer, which
    /// is handed a `WindowManager` and nothing else, can serve them to a
    /// panel or launcher that wants to show the user what their keys do.
    /// Asked for as whether "our bindings show in ags's launcher": they
    /// could not, because nothing published them anywhere a client could
    /// read.
    pub keybindings: Vec<(String, String)>,
    /// Set by `request_refresh`, drained by the main loop. Same
    /// cross-boundary queued-request shape as `lock_requested`.
    refresh_requested: bool,
    /// Every setting changed live since startup, as `srd set` key -> the
    /// raw JSON text of its value, in insertion-independent key order.
    ///
    /// Exists so a config reload does not silently undo a change the user
    /// just made by hand. `apply_general_settings` rebuilds the whole
    /// `ThemeConfig` and general-settings block from the config file, which
    /// is the correct precedence for a *file* edit - but it also wiped
    /// every live `srd set`, and the titlebar right-click menu's own
    /// "Customize" section is built entirely out of live `srd set`s. So
    /// changing a button style from that menu and then saving `init.lua`
    /// for any unrelated reason silently reverted it.
    ///
    /// That was survivable while reloads only happened on an explicit
    /// `Mod4+Ctrl+r`. `general.config_reload_on_write` makes a reload
    /// happen on every save, which turns a rare surprise into a reliable
    /// one - a control that silently reverts is worse than no control.
    ///
    /// Raw JSON text rather than a typed value because this crate has no
    /// serde dependency and no business gaining one for this; the platform
    /// crate parses it back and replays it through the same `handle_set`
    /// that recorded it, so a replayed setting cannot diverge from a real
    /// one. `BTreeMap` for a deterministic replay order.
    live_settings: std::collections::BTreeMap<String, String>,
    drag: Option<DragState>,
    resize: Option<ResizeState>,
    rules: Vec<WindowRule>,
    /// Last floating position+size a user interactively moved/resized each
    /// `app_id` to, applied to that app's *next* new window instead of the
    /// fixed 800x600-near-centre every backend otherwise hardcodes - see
    /// `end_resize`/`end_drag` (where this is recorded) and `add_window`
    /// (where it's read). Keyed by `app_id` alone, not per-window: the ask
    /// is "my terminal should open where/how big I last left one", not
    /// per-window-instance memory. Only an interactive drag/resize updates
    /// this - not a maximize/fullscreen toggle (that's a separate,
    /// temporary state with its own `restore_geometry`, not a new
    /// "position/size I want to keep using") and not a drag-to-edge snap (a
    /// deliberate one-off snap to a half/quarter of the screen isn't "where
    /// I'll want my next terminal to open" either).
    ///
    /// In-memory here (this struct has no file I/O of its own - see
    /// `srdwm_core`'s own module doc comment on why core stays pure logic);
    /// `crates/wayland/src/window_memory.rs` is what actually persists this
    /// to `$XDG_STATE_HOME/srd/window-memory.json` and re-seeds it via
    /// `set_remembered_geometry` at startup, the same load/save-at-the-
    /// platform-layer split `monitor_layout.rs`/`desktop_icons_state.rs`
    /// already use for their own per-feature state.
    remembered_geometry: HashMap<String, (i32, i32, u32, u32)>,
    /// Windows a client-close was requested for, drained once per tick by
    /// `main.rs`'s event loop and forwarded to `Platform::close`. Needed
    /// because `WindowManager` is platform-agnostic and has no way to send
    /// a client its close request directly - see `close_window`.
    close_requests: Vec<WindowId>,
    /// The active XKB layout's own name (e.g. `"English (US)"`, whatever
    /// `xkb_keymap_layout_get_name` reports) - set by the platform once at
    /// startup and again after every `take_keyboard_layout_cycle_requests`
    /// is acted on. Empty until the platform has reported it at least once
    /// (a nested/test `WindowManager::new()` with no real keyboard, most
    /// of core's own tests). Read-only from an external caller's point of
    /// view (an AGS peer session's keyboard-layout badge, over `srd`); the
    /// only way to change it is a real layout cycle.
    pub keyboard_layout: String,
    /// How many `srd dispatch cycle_keyboard_layout` requests have arrived
    /// since the last drain - a count, not a flag, so two IPC requests in
    /// one tick both take effect rather than the second being silently
    /// swallowed. Same "core records the intent, `main.rs`'s `sync()`
    /// forwards it to the platform that can actually act on it" shape as
    /// `close_requests`, for the same reason: `WindowManager` has no real
    /// keyboard/seat handle of its own to cycle.
    keyboard_layout_cycle_requests: u32,
    /// Which monitor the pointer is currently over, as last reported by
    /// `set_pointer_monitor` - core has no pointer of its own (backend-
    /// agnostic, same reason `close_requests` exists instead of a direct
    /// client call), so a real backend's own pointer-motion handler is the
    /// only thing that can ever know this. `add_window`'s own target-
    /// monitor fallback chain reads it: a new window already preferred the
    /// *focused* window's monitor over the primary one (see that fix's own
    /// doc comment, `add_window`) - correct when something is focused on
    /// the monitor the user is actually at, but not when nothing is (an
    /// empty desktop there, or the last-focused window happens to sit on a
    /// *different* monitor than the one the user is currently pointing at
    /// while launching something new). Reported live: opening an
    /// application while on a non-primary monitor with nothing focused
    /// there still opened it on the primary one. `None` until the first
    /// real pointer-motion event arrives (matches `focused`'s own `None`-
    /// until-something-happens shape).
    pointer_monitor: Option<MonitorId>,
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowManager {
    pub fn new() -> Self {
        let mut layouts: HashMap<String, Box<dyn Layout>> = HashMap::new();
        layouts.insert("tiling".into(), Box::new(MasterStackLayout));
        layouts.insert("dynamic".into(), Box::new(NoOpLayout("dynamic")));
        layouts.insert("floating".into(), Box::new(NoOpLayout("floating")));

        Self {
            windows: HashMap::new(),
            order: Vec::new(),
            focused: None,
            monitors: Vec::new(),
            output_position_requests: Vec::new(),
            pin_input_requests: Vec::new(),
            pinned_windows: HashMap::new(),
            create_fake_monitor_requests: Vec::new(),
            remove_fake_monitor_requests: Vec::new(),
            output_enable_requests: Vec::new(),
            disabled_monitors: HashMap::new(),
            monitor_splits: HashMap::new(),
            monitor_split_requests: Vec::new(),
            monitor_scales: HashMap::new(),
            lock_requested: false,
            capture_requests: Vec::new(),
            // 1-based, not 0-based: workspace ids match the human-visible
            // numbers (`workspace.names` defaults to "1".."9","0",
            // `apply_workspace_count` names workspace `i+1` "i+1") - an id
            // of `0` for the first workspace, with everything display-side
            // calling it "1", was a standing off-by-one between what a user
            // types/sees and the id `srd dispatch activate workspace <n>`
            // (and AGS's workspace switcher, which sends the same number it
            // shows) actually has to send. Matches how Hyprland's own
            // workspace ids already work (natively 1-based, no translation
            // layer needed) rather than niri's split id/idx or the
            // 0-based-plus-AGS-side-`+1` scheme this used to be - both
            // AGS integrations for those two compositors were checked
            // before choosing this, and neither needs hand-rolled offset
            // arithmetic the way srdwm's old 0-based ids forced `lib/
            // srdwm.ts` to.
            //
            // Rolling this out requires `crates/config`'s shipped default,
            // this user's own `~/.config/srd/keybindings.lua`, and AGS's
            // `lib/srdwm.ts`/`service/wsPreview.ts` to all agree with core
            // at the same time - they cannot update atomically with a
            // single srdwm restart, so whichever of AGS/srdwm is running
            // the *other* scheme during that window will visibly
            // misbehave (confirmed live: AGS's Overview padding
            // `workspace.count` slots and matching real workspaces onto
            // them by id showed one extra/unmatched slot while AGS's own
            // code had already been updated to assume 1-based ids but the
            // live srdwm process was still 0-based). AGS's side is
            // deliberately reverted back to its old `+1` offset for now,
            // matching the still-running old build, and must be re-applied
            // in the same breath as the next real srdwm restart - not
            // before.
            workspaces: vec![Workspace::new(1, "1", "dynamic")],
            current_workspace: 1,
            previous_workspace: 1,
            auto_back_and_forth: false,
            per_monitor_workspaces: false,
            primary_layout: String::new(),
            secondary_layout: String::new(),
            monitor_workspaces: HashMap::new(),
            next_workspace_id: 2,
            next_window_id: 1,
            layouts,
            tiling: TilingConfig::default(),
            placement: PlacementConfig::default(),
            next_cascade_step: std::cell::Cell::new(0),
            animations_enabled: true,
            animation_duration_ms: 200,
            shadows_enabled: true,
            close_focus_follows_workspace: false,
            resize_margin: RESIZE_MARGIN,
            rounded_corners_enabled: None,
            gpu_enabled: false,
            multi_cursor_enabled: false,
            phone_mode: false,
            desktop_icons_enabled: true,
            desktop_icons_all_monitors: true,
            reserve_top: 0,
            reserve_bottom: 0,
            reserve_left: 0,
            reserve_right: 0,
            file_manager: String::new(),
            desktop_icon_single_click: false,
            terminal: String::new(),
            color_filter: ColorFilter::None,
            focus_follows_mouse: false,
            auto_raise: false,
            theme: ThemeConfig::default(),
            lock: LockConfig::default(),
            keybindings: Vec::new(),
            maximize_covers_dock: true,
            refresh_requested: false,
            live_settings: std::collections::BTreeMap::new(),
            drag: None,
            resize: None,
            rules: Vec::new(),
            remembered_geometry: HashMap::new(),
            close_requests: Vec::new(),
            keyboard_layout: String::new(),
            keyboard_layout_cycle_requests: 0,
            pointer_monitor: None,
        }
    }

    /// Registers a window rule; on every subsequent `add_window`, the first
    /// rule whose matcher matches the new window has its actions applied.
    pub fn add_rule(&mut self, rule: WindowRule) {
        self.rules.push(rule);
    }

    pub fn register_layout(&mut self, name: impl Into<String>, layout: Box<dyn Layout>) {
        self.layouts.insert(name.into(), layout);
    }

    pub fn available_layouts(&self) -> Vec<&str> {
        self.layouts.keys().map(String::as_str).collect()
    }

}

mod capture;
mod dragresize;
mod fake_monitor;
mod focus;
mod hittest;
mod input_pin;
mod layout;
mod lock;
mod monitors;
mod windows;
mod winops;
mod workspaces;

pub use capture::CaptureRequest;

#[cfg(test)]
mod tests;
