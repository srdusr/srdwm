use crate::geometry::Rect;
use crate::layout::{Layout, MasterStackLayout, NoOpLayout, TilingConfig};
use crate::monitor::{Monitor, MonitorId};
use crate::placement::{PlacementConfig, SmartPlacement, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};
use crate::rules::WindowRule;
use crate::theme::ThemeConfig;
use crate::window::{ResizeEdge, TitlebarHit, Window, WindowId, RESIZE_MARGIN};
use crate::workspace::{Workspace, WorkspaceId};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

struct DragState {
    window: WindowId,
    start_x: i32,
    start_y: i32,
    orig: Rect,
}

struct ResizeState {
    window: WindowId,
    edge: ResizeEdge,
    start_x: i32,
    start_y: i32,
    orig: Rect,
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
    workspaces: Vec<Workspace>,
    current_workspace: WorkspaceId,
    /// Whichever workspace was current immediately before the current one
    /// became current - see `switch_workspace`'s doc comment.
    previous_workspace: WorkspaceId,
    /// Read from `workspace.auto_back_and_forth`. When set, switching to
    /// the workspace that's already active switches to `previous_workspace`
    /// instead - sway's `workspace_auto_back_and_forth` behavior, a quick
    /// "jump back to whatever I was just on" toggle on a single keybinding.
    pub auto_back_and_forth: bool,
    next_workspace_id: WorkspaceId,
    next_window_id: WindowId,
    layouts: HashMap<String, Box<dyn Layout>>,
    pub tiling: TilingConfig,
    pub placement: PlacementConfig,
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
    /// Width, in pixels, of the resize grab band along a window's edges,
    /// read from `general.resize_margin`. See [`crate::window::RESIZE_MARGIN`]'s
    /// doc comment for the default and why it's what it is.
    pub resize_margin: i32,
    /// Whether a decorated window's content rounds its bottom two corners
    /// to match the titlebar's own curve (an undecorated/CSD window rounds
    /// all four). Read from `general.rounded_corners`. GLES-backend-only
    /// in practice - see `crates/wayland/src/rounded_corners.rs` - so
    /// this is a no-op on the udev/Pixman backend regardless of its value.
    pub rounded_corners_enabled: bool,
    /// Default decoration colours and border width, read from `theme.colors.*`/
    /// `theme.decorations.*`. See `ThemeConfig`'s own doc comment.
    pub theme: ThemeConfig,
    drag: Option<DragState>,
    resize: Option<ResizeState>,
    rules: Vec<WindowRule>,
    /// Windows a client-close was requested for, drained once per tick by
    /// `main.rs`'s event loop and forwarded to `Platform::close`. Needed
    /// because `WindowManager` is platform-agnostic and has no way to send
    /// a client its close request directly - see `close_window`.
    close_requests: Vec<WindowId>,
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
            workspaces: vec![Workspace::new(0, "1", "dynamic")],
            current_workspace: 0,
            previous_workspace: 0,
            auto_back_and_forth: false,
            next_workspace_id: 1,
            next_window_id: 1,
            layouts,
            tiling: TilingConfig::default(),
            placement: PlacementConfig::default(),
            animations_enabled: true,
            animation_duration_ms: 200,
            shadows_enabled: true,
            resize_margin: RESIZE_MARGIN,
            rounded_corners_enabled: true,
            theme: ThemeConfig::default(),
            drag: None,
            resize: None,
            rules: Vec::new(),
            close_requests: Vec::new(),
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

    // ---- Monitors ----------------------------------------------------

    /// Replaces the monitor list, rehoming any window left stranded.
    ///
    /// Called at startup and again on every hotplug. Unplugging a monitor
    /// would otherwise leave its windows pointing at a `monitor` id that no
    /// longer exists: `arrange_workspace` skips those (it looks the monitor
    /// up to get a rectangle), so they would stop being tiled, and a
    /// floating window would sit at coordinates that are no longer on any
    /// screen - unreachable, with no way to drag it back.
    ///
    /// Stranded windows are moved to the primary monitor and, if their
    /// geometry falls outside it, nudged back inside.
    ///
    /// This keys off **geometry**, not just the `monitor` field. That field
    /// records which monitor a window was *assigned* at creation and does
    /// not track where the window actually is: a floating window dragged --
    /// or placed by a rule - onto a second monitor keeps `monitor`
    /// pointing at the first. Trusting the field alone left such a window
    /// at coordinates that no longer existed once its real monitor was
    /// unplugged: off-screen and unreachable, with no way to drag it back.
    /// Found by unplugging a monitor out from under a window in the QEMU VM
    /// and watching it vanish; the field-only check had passed its unit
    /// tests because those set `monitor` explicitly.
    pub fn set_monitors(&mut self, monitors: Vec<Monitor>) {
        self.monitors = monitors;

        let Some(primary) = self.primary_monitor().cloned() else {
            // No monitors at all (every output unplugged): leave windows
            // as-is rather than collapsing them onto nothing, so they are
            // restored intact when an output comes back.
            return;
        };
        let live = self.monitors.clone();
        for window in self.windows.values_mut() {
            let visible_on = live.iter().find(|m| m.geometry.overlaps(&window.geometry));
            match visible_on {
                // Still on screen: just make sure its monitor id points at a
                // monitor that exists, so tiling keeps working.
                Some(monitor) => {
                    if !live.iter().any(|m| m.id == window.monitor) {
                        window.monitor = monitor.id;
                    }
                }
                // Nothing on screen shows this window any more.
                None => {
                    window.geometry = window.geometry.clamped_into(primary.geometry);
                    window.monitor = primary.id;
                }
            }
        }
        // A maximized/fullscreen window's geometry was set to a snapshot of
        // its monitor's usable/full rect at the moment it was toggled on --
        // it is not live-bound to that rect afterward. Without this, a bar
        // or dock changing its exclusive zone while a window is maximized
        // (the live case: a dock dropping its reservation to 0 so a
        // maximized window can cover its area) grows or shrinks `Monitor::
        // geometry`/`full_geometry` here, but the already-maximized window
        // keeps its stale pre-change size until manually un-maximized and
        // re-maximized - reported as "maximize does not extend past the
        // dock" even though the dock's own zone change took effect
        // immediately in every other respect (new windows placed correctly,
        // `Monitor::geometry` itself correct if queried fresh).
        for window in self.windows.values_mut() {
            if !window.maximized && !window.fullscreen {
                continue;
            }
            let Some(monitor) = live.iter().find(|m| m.id == window.monitor) else { continue };
            let target = if window.fullscreen { monitor.full_geometry } else { monitor.geometry };
            if window.geometry != target {
                window.geometry = target;
            }
        }
    }

    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    pub fn primary_monitor(&self) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.primary).or_else(|| self.monitors.first())
    }

    fn monitor_for(&self, id: MonitorId) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.id == id).or_else(|| self.primary_monitor())
    }

    // ---- Windows -------------------------------------------------------

    pub fn alloc_window_id(&mut self) -> WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    /// Registers a window that a backend has already created. If the current
    /// workspace's layout doesn't auto-tile ("dynamic"/"floating"), the
    /// window's initial geometry is chosen via [`SmartPlacement`]; otherwise
    /// it's left for the next `arrange_workspace` call to place.
    pub fn add_window(&mut self, mut window: Window) -> WindowId {
        let id = window.id;
        // Applied before rule matching below, which still wins when a rule
        // sets its own `border_color`/`border_width` - this only replaces
        // whatever a backend's `Window::new` happened to hardcode.
        window.border_color = self.theme.default_border_color;
        window.border_width = self.theme.default_border_width;
        let actions = self.rules.iter().find(|r| r.matcher.matches(&window)).map(|r| r.actions.clone());
        // See `Window::rules_applied`'s doc comment: a native Wayland window
        // still has empty title/app_id at this point, so a real (if
        // inconclusive) match attempt needs to wait for `reapply_rules_if_pending`.
        window.rules_applied = actions.is_some() || !(window.title.is_empty() && window.app_id.is_empty());

        let workspace = actions.as_ref().and_then(|a| a.workspace).unwrap_or(self.current_workspace);
        window.workspace = workspace;
        if let Some(a) = &actions {
            if let Some(floating) = a.floating {
                window.floating = floating;
            }
            if let Some(decorated) = a.decorated {
                window.decorated = decorated;
            }
            if let Some(color) = a.border_color {
                window.border_color = color;
            }
            if let Some(width) = a.border_width {
                window.border_width = width;
            }
            if let Some(pinned) = a.pinned {
                window.always_on_top = pinned;
            }
            if let Some(opacity) = a.opacity {
                window.opacity = opacity.clamp(0.0, 1.0);
            }
        }

        if let Some(monitor) = self.primary_monitor() {
            window.monitor = monitor.id;
            let layout_name = self.workspace(workspace).map(|w| w.layout.clone()).unwrap_or_default();
            if layout_name != "tiling" {
                let existing: Vec<Rect> = self.windows_on_workspace(workspace).map(|w| w.geometry).collect();
                let size = (window.geometry.width, window.geometry.height);
                window.geometry = SmartPlacement::place(monitor, &existing, size, &self.placement);
            }
        }
        if let Some(geometry) = actions.as_ref().and_then(|a| a.geometry) {
            window.geometry = geometry;
        }
        let maximize = actions.as_ref().and_then(|a| a.maximized).unwrap_or(false);

        self.windows.insert(id, window);
        self.order.push(id);
        self.focused = Some(id);
        // A new window goes on top, but must not cover a pinned one.
        self.restack_pinned();
        if maximize {
            self.toggle_maximize(id);
        }
        id
    }

    /// Retries rule matching for a window `add_window` couldn't conclusively
    /// match yet (see `Window::rules_applied`'s doc comment) - a backend
    /// calls this once a native Wayland window's real `title`/`app_id`
    /// become known, typically on its first real commit. A no-op once
    /// `rules_applied` is already `true`, so this is safe to call on every
    /// subsequent metadata change without rules re-applying repeatedly.
    ///
    /// Returns whether a rule actually matched and was applied - distinct
    /// from simply "ran" (this is a no-op past the first call regardless).
    /// A backend uses this to decide whether a follow-up geometry/decoration
    /// sync is warranted: `sync_geometry` re-stacks the window to the top
    /// via smithay's `Space::map_element` as a side effect of updating its
    /// tracked position (`map_element` always does this, `activate` or
    /// not - there is no "move without restacking" in this smithay
    /// version), so calling it on *every* title/app_id change - which
    /// happens constantly for perfectly ordinary reasons (a browser tab
    /// finishing a page load) long after the window's own creation - would
    /// silently yank an unfocused, unrelated window back to the front any
    /// time its title happened to update. Reported live as exactly that:
    /// an older window jumping in front of a newer, focused one with no
    /// user action to explain it.
    pub fn reapply_rules_if_pending(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else { return false };
        if window.rules_applied || (window.title.is_empty() && window.app_id.is_empty()) {
            return false;
        }
        let actions = self.rules.iter().find(|r| r.matcher.matches(window)).map(|r| r.actions.clone());
        let Some(window) = self.windows.get_mut(&id) else { return false };
        window.rules_applied = true;
        let Some(actions) = actions else { return false };
        if let Some(floating) = actions.floating {
            window.floating = floating;
        }
        if let Some(decorated) = actions.decorated {
            window.decorated = decorated;
        }
        if let Some(color) = actions.border_color {
            window.border_color = color;
        }
        if let Some(width) = actions.border_width {
            window.border_width = width;
        }
        if let Some(pinned) = actions.pinned {
            window.always_on_top = pinned;
        }
        if let Some(opacity) = actions.opacity {
            window.opacity = opacity.clamp(0.0, 1.0);
        }
        if let Some(geometry) = actions.geometry {
            window.geometry = geometry;
        }
        if let Some(workspace) = actions.workspace {
            self.move_window_to_workspace(id, workspace);
        }
        if actions.maximized.unwrap_or(false) {
            self.toggle_maximize(id);
        }
        true
    }

    pub fn remove_window(&mut self, id: WindowId) -> Option<Window> {
        self.order.retain(|&w| w != id);
        if self.focused == Some(id) {
            self.focused = self.order.last().copied();
        }
        self.windows.remove(&id)
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values()
    }

    /// Windows in stacking order, topmost (most recently raised) last.
    pub fn stacking_order(&self) -> impl Iterator<Item = &Window> {
        self.order.iter().filter_map(|id| self.windows.get(id))
    }

    fn windows_on_workspace(&self, workspace: WorkspaceId) -> impl Iterator<Item = &Window> {
        self.windows.values().filter(move |w| w.workspace == workspace)
    }

    pub fn raise_window(&mut self, id: WindowId) {
        if let Some(pos) = self.order.iter().position(|&w| w == id) {
            let id = self.order.remove(pos);
            self.order.push(id);
        }
        self.restack_pinned();
    }

    /// Sends a window to the back of the stack - the middle-click-titlebar
    /// convention most X11 WMs (twm, fvwm, IceWM) have always had and this
    /// one never did. Doesn't touch focus: lowering the window you're
    /// currently looking at out from under the pointer without also moving
    /// keyboard focus elsewhere would leave input going to a window that's
    /// no longer visible under the cursor, which is more surprising than
    /// useful. `restack_pinned` still runs afterward so a pinned window
    /// can't accidentally end up buried by this either.
    pub fn lower_window(&mut self, id: WindowId) {
        if let Some(pos) = self.order.iter().position(|&w| w == id) {
            let id = self.order.remove(pos);
            self.order.insert(0, id);
        }
        self.restack_pinned();
    }

    /// Toggles "always on top" for a window (Hyprland's `pin`), used for
    /// picture-in-picture and small HUD overlays that must stay visible
    /// while you work in something else.
    pub fn toggle_always_on_top(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.always_on_top = !w.always_on_top;
        }
        self.restack_pinned();
    }

    pub fn is_always_on_top(&self, id: WindowId) -> bool {
        self.windows.get(&id).map(|w| w.always_on_top).unwrap_or(false)
    }

    /// Moves every always-on-top window to the top of the stack, keeping
    /// their relative order.
    ///
    /// `order` is the stacking order (last = topmost), so pinning is not a
    /// property the renderer checks - it is maintained here, which means
    /// every existing consumer of `stacking_order` gets it for free and
    /// cannot forget to honour it.
    fn restack_pinned(&mut self) {
        if !self.windows.values().any(|w| w.always_on_top) {
            return;
        }
        let (pinned, rest): (Vec<_>, Vec<_>) = self
            .order
            .iter()
            .partition(|id| self.windows.get(id).is_some_and(|w| w.always_on_top));
        self.order = rest.into_iter().chain(pinned).collect();
    }

    // ---- Focus ----------------------------------------------------------

    pub fn focused_window(&self) -> Option<&Window> {
        self.focused.and_then(|id| self.windows.get(&id))
    }

    pub fn focused_id(&self) -> Option<WindowId> {
        self.focused
    }

    pub fn focus_window(&mut self, id: WindowId) {
        if self.windows.contains_key(&id) {
            self.focused = Some(id);
            self.raise_window(id);
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        let ids: Vec<WindowId> = self.windows_on_workspace(self.current_workspace).filter(|w| !w.minimized).map(|w| w.id).collect();
        if ids.is_empty() {
            self.focused = None;
            return;
        }
        let cur_pos = self.focused.and_then(|f| ids.iter().position(|&i| i == f));
        let next = match cur_pos {
            None => 0,
            Some(p) if forward => (p + 1) % ids.len(),
            Some(p) => (p + ids.len() - 1) % ids.len(),
        };
        self.focus_window(ids[next]);
    }

    pub fn focus_next(&mut self) {
        self.cycle_focus(true);
    }

    pub fn focus_previous(&mut self) {
        self.cycle_focus(false);
    }

    /// Vim-style directional focus: picks the nearest window whose center
    /// lies in `dir` relative to the focused window's center, on the same
    /// workspace. Returns the newly focused window, if any.
    /// Nearest window to the focused one in `dir`, by a distance biased
    /// toward the requested axis so a window that's mostly to the left
    /// (small |dy|) beats a diagonally-placed one - matching how
    /// i3/sway-style directional focus feels.
    ///
    /// Shared by [`Self::focus_direction`] and [`Self::move_window_direction`]
    /// so "the window to the left" means the same thing whether you're
    /// focusing it or swapping with it.
    pub fn neighbour_in(&self, dir: Direction) -> Option<WindowId> {
        let (fx, fy, fid) = {
            let focused = self.focused_window()?;
            let (fx, fy) = focused.geometry.center();
            (fx, fy, focused.id)
        };
        let workspace = self.current_workspace;
        let mut best: Option<(WindowId, i64)> = None;
        for w in self.windows_on_workspace(workspace).filter(|w| w.id != fid && !w.minimized) {
            let (cx, cy) = w.geometry.center();
            let (dx, dy) = ((cx - fx) as i64, (cy - fy) as i64);
            let matches = match dir {
                Direction::Left => dx < 0,
                Direction::Right => dx > 0,
                Direction::Up => dy < 0,
                Direction::Down => dy > 0,
            };
            if !matches {
                continue;
            }
            let (primary, secondary) = match dir {
                Direction::Left | Direction::Right => (dx, dy),
                Direction::Up | Direction::Down => (dy, dx),
            };
            let dist = primary * primary + secondary * secondary * 4;
            if best.is_none_or(|(_, d)| dist < d) {
                best = Some((w.id, dist));
            }
        }
        best.map(|(id, _)| id)
    }

    pub fn focus_direction(&mut self, dir: Direction) -> Option<WindowId> {
        let target = self.neighbour_in(dir);
        if let Some(id) = target {
            self.focus_window(id);
        }
        target
    }

    /// Moves the focused window in `dir` by swapping places with its
    /// neighbour there - the `movewindow l/r/u/d` gesture.
    ///
    /// Swapping (rather than nudging by a fixed step) is what makes this
    /// useful in both of srdwm's modes: under tiling it reorders the layout,
    /// and in dynamic/floating mode two windows trade positions, which is
    /// predictable either way. With no neighbour in that direction the
    /// window is pushed to the corresponding edge of its monitor instead, so
    /// the key still does something sensible.
    pub fn move_window_direction(&mut self, dir: Direction) -> Option<WindowId> {
        let focused = self.focused_id()?;
        match self.neighbour_in(dir) {
            Some(other) => {
                let a = self.windows.get(&focused)?.geometry;
                let b = self.windows.get(&other)?.geometry;
                if let Some(w) = self.windows.get_mut(&focused) {
                    w.geometry = b;
                }
                if let Some(w) = self.windows.get_mut(&other) {
                    w.geometry = a;
                }
                // Keep stacking order in step so a tiling layout, which
                // assigns slots from `order`, actually reflects the swap.
                let (ia, ib) = (
                    self.order.iter().position(|&id| id == focused)?,
                    self.order.iter().position(|&id| id == other)?,
                );
                self.order.swap(ia, ib);
                Some(other)
            }
            None => {
                let mon = self.windows.get(&focused).and_then(|w| self.monitor_for(w.monitor))?.geometry;
                let w = self.windows.get_mut(&focused)?;
                match dir {
                    Direction::Left => w.geometry.x = mon.x,
                    Direction::Right => w.geometry.x = mon.right() - w.geometry.width as i32,
                    Direction::Up => w.geometry.y = mon.y,
                    Direction::Down => w.geometry.y = mon.bottom() - w.geometry.height as i32,
                }
                None
            }
        }
    }

    // ---- Window operations ----------------------------------------------

    pub fn close_window(&mut self, id: WindowId) {
        log::info!("close_window({id})");
        self.close_requests.push(id);
    }

    /// Drains windows queued by `close_window` since the last call. Core
    /// has no way to reach a client itself - the caller (`main.rs`) is
    /// expected to forward each id to `Platform::close`.
    pub fn take_close_requests(&mut self) -> Vec<WindowId> {
        std::mem::take(&mut self.close_requests)
    }

    pub fn minimize_window(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.minimized = true;
        }
        if self.focused == Some(id) {
            self.cycle_focus(true);
        }
    }

    pub fn restore_window(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.minimized = false;
        }
    }

    /// Moves a window into the scratchpad pool, hiding it immediately --
    /// sway's `move scratchpad`. The single most-used "quick terminal"
    /// pattern in tiling window managers, and srdwm had no equivalent at
    /// all before this.
    ///
    /// Also floats the window: tiling something that's meant to pop in and
    /// out on demand doesn't make sense, and would otherwise fight
    /// `arrange_workspace` every time it's shown. Reuses `minimized` for
    /// the actual show/hide gating rather than introducing a second
    /// visibility flag - `scratchpad` here is purely a marker of *pool
    /// membership*, kept separate so `scratchpad_show` knows which hidden
    /// windows are its own to bring back, as opposed to an ordinarily
    /// minimized one.
    pub fn scratchpad_add(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.scratchpad = true;
            w.floating = true;
        }
        self.minimize_window(id);
    }

    /// Removes a window from the scratchpad pool without changing its
    /// current visibility - for a rule or script that wants to opt a
    /// window back into ordinary window management.
    pub fn scratchpad_remove(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.scratchpad = false;
        }
    }

    /// Toggles the scratchpad - sway's `scratchpad show`, meant for one
    /// keybinding a user presses repeatedly. If the focused window is
    /// itself a currently-shown scratchpad window, hides it; otherwise
    /// shows (and focuses) the most recently added hidden scratchpad
    /// window, if any, moving it onto whichever workspace is current so it
    /// follows the user rather than staying pinned to wherever it was
    /// added from - sway's own behavior. "Most recently added" is `id`
    /// order, since ids are allocated monotonically and no separate
    /// timestamp is tracked; only ever one window is shown/hidden per
    /// call, deliberately not sway's full multi-window cycling, which
    /// needs its own remembered order and is a rarer need than a single
    /// scratchpad window covers.
    pub fn scratchpad_show(&mut self) {
        if let Some(id) = self.focused {
            if self.windows.get(&id).is_some_and(|w| w.scratchpad && !w.minimized) {
                self.minimize_window(id);
                return;
            }
        }
        let Some(id) = self.windows.values().filter(|w| w.scratchpad && w.minimized).map(|w| w.id).max() else { return };
        if let Some(w) = self.windows.get_mut(&id) {
            w.workspace = self.current_workspace;
        }
        self.restore_window(id);
        self.focus_window(id);
    }

    pub fn toggle_maximize(&mut self, id: WindowId) {
        let monitor_geom = self.windows.get(&id).and_then(|w| self.monitor_for(w.monitor)).map(|m| m.geometry);
        let animations_enabled = self.animations_enabled;
        let Some(w) = self.windows.get_mut(&id) else { return };
        let from = w.geometry;
        if w.maximized {
            if let Some(restore) = w.restore_geometry.take() {
                w.geometry = restore;
            }
            w.maximized = false;
        } else if let Some(geom) = monitor_geom {
            w.restore_geometry = Some(w.geometry);
            w.geometry = geom;
            w.maximized = true;
        }
        if animations_enabled && w.geometry != from {
            w.anim_from = Some(from);
        }
    }

    /// Fullscreen: the window covers its whole monitor with no decoration.
    ///
    /// Distinct from [`Self::toggle_maximize`], which keeps the titlebar (and
    /// is what a maximise button does). Both share `restore_geometry`, so
    /// they are mutually exclusive - toggling one off restores whatever the
    /// window's geometry was before *either* was applied, and entering
    /// fullscreen from a maximised window doesn't lose the original size.
    ///
    /// `decorated` is saved and restored the same way, via
    /// `restore_decorated` - exiting used to hardcode `w.decorated = true`
    /// unconditionally, which is only correct for a window that was
    /// decorated to begin with. Any window a rule sets `decorated = false`
    /// for (client-side-decorated apps like Firefox, matched via
    /// `srd.rule({ class = "firefox" }, { decorated = false })`) that ever
    /// goes fullscreen - an HTML5 video, a PDF presentation, plain F11 --
    /// came back from it permanently `decorated = true`, with no further
    /// event to ever set it back. Since border/titlebar redraw fresh from
    /// live `Window.decorated` every frame but the *hit-testing* band this
    /// wrongly turned on doesn't correspond to anything the client is
    /// actually drawing there, every click in what srdwm now (incorrectly)
    /// treats as the titlebar band got swallowed as a drag/button hit
    /// instead of ever reaching the client - reported live as a click on
    /// Firefox's back button minimizing the window instead.
    pub fn toggle_fullscreen(&mut self, id: WindowId) {
        // Unlike `toggle_maximize`, fullscreen uses the monitor's true
        // full rect, not the exclusive-zone-shrunk usable area - a
        // fullscreen window should cover (or go under) a bar/dock like
        // everywhere else, not stop short of it. See `Monitor::
        // full_geometry`'s doc comment.
        let monitor_geom = self.windows.get(&id).and_then(|w| self.monitor_for(w.monitor)).map(|m| m.full_geometry);
        let animations_enabled = self.animations_enabled;
        let Some(w) = self.windows.get_mut(&id) else { return };
        let from = w.geometry;
        if w.fullscreen {
            if let Some(restore) = w.restore_geometry.take() {
                w.geometry = restore;
            }
            w.fullscreen = false;
            w.decorated = w.restore_decorated.take().unwrap_or(true);
        } else if let Some(geom) = monitor_geom {
            // Only remember the pre-fullscreen geometry if we aren't already
            // maximised, otherwise the monitor rect would overwrite the real
            // restore point and the window could never get its size back.
            if !w.maximized {
                w.restore_geometry = Some(w.geometry);
            }
            w.maximized = false;
            w.geometry = geom;
            w.fullscreen = true;
            w.restore_decorated = Some(w.decorated);
            w.decorated = false;
        }
        if animations_enabled && w.geometry != from {
            w.anim_from = Some(from);
        }
    }

    pub fn is_fullscreen(&self, id: WindowId) -> bool {
        self.windows.get(&id).map(|w| w.fullscreen).unwrap_or(false)
    }

    pub fn toggle_floating(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.floating = !w.floating;
        }
    }

    pub fn is_floating(&self, id: WindowId) -> bool {
        self.windows.get(&id).map(|w| w.floating).unwrap_or(false)
    }

    pub fn move_window(&mut self, id: WindowId, x: i32, y: i32) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.geometry.x = x;
            w.geometry.y = y;
        }
    }

    pub fn resize_window(&mut self, id: WindowId, width: u32, height: u32) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.geometry.width = width.max(MIN_WINDOW_WIDTH);
            w.geometry.height = height.max(MIN_WINDOW_HEIGHT);
        }
    }

    // ---- Hit testing ------------------------------------------------------

    /// Topmost window whose frame contains `(x, y)`, along with what part of
    /// its titlebar/border was hit (button, drag area, resize edge).
    pub fn hit_test(&self, x: i32, y: i32) -> Option<(WindowId, TitlebarHit)> {
        for w in self.order.iter().rev().filter_map(|id| self.windows.get(id)) {
            if w.minimized {
                continue;
            }
            if let Some(hit) = ResizeEdge::hit_test(w.geometry, x, y, w.decorated, w.border_width, self.resize_margin) {
                return Some((w.id, hit));
            }
        }
        None
    }

    /// Topmost non-minimised window containing a point, ignoring
    /// decorations. Used for modifier+drag, where the grab applies anywhere
    /// in the window rather than only on the titlebar (`hit_test`).
    pub fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
        self.order
            .iter()
            .rev()
            .filter_map(|id| self.windows.get(id))
            .find(|w| !w.minimized && w.geometry.contains_point(x, y))
            .map(|w| w.id)
    }

    /// The corner of `id` nearest a point, for modifier+right-drag resize:
    /// grabbing the closest corner is what makes the gesture feel like it
    /// pulls the edge you aimed at (matching Hyprland's `resizewindow`).
    pub fn nearest_corner(&self, id: WindowId, x: i32, y: i32) -> ResizeEdge {
        let Some(w) = self.windows.get(&id) else { return ResizeEdge::BottomRight };
        let (cx, cy) = w.geometry.center();
        match (x < cx, y < cy) {
            (true, true) => ResizeEdge::TopLeft,
            (false, true) => ResizeEdge::TopRight,
            (true, false) => ResizeEdge::BottomLeft,
            (false, false) => ResizeEdge::BottomRight,
        }
    }

    // ---- Drag / resize ------------------------------------------------------

    pub fn start_drag(&mut self, id: WindowId, x: i32, y: i32) {
        if let Some(w) = self.windows.get(&id) {
            self.drag = Some(DragState { window: id, start_x: x, start_y: y, orig: w.geometry });
            self.focus_window(id);
        }
    }

    pub fn update_drag(&mut self, x: i32, y: i32) {
        let Some(drag) = &self.drag else { return };
        let (dx, dy) = (x - drag.start_x, y - drag.start_y);
        let mut new_geom = drag.orig;
        new_geom.x += dx;
        new_geom.y += dy;

        // `full_geometry`, not `geometry`: a floating window being dragged
        // must be able to cross into (or land under/over) the strip a
        // bar/dock reserves - only *placement* of a brand-new window and
        // maximize avoid it. Clamping a drag to the shrunk usable area
        // made it physically impossible to ever drag a window past a
        // dock, at any speed or angle.
        let monitor_bounds = self.windows.get(&drag.window).and_then(|w| self.monitor_for(w.monitor)).map(|m| m.full_geometry);
        if let Some(bounds) = monitor_bounds {
            new_geom.x = new_geom.x.clamp(bounds.x - new_geom.width as i32 + 40, bounds.right() - 40);
            new_geom.y = new_geom.y.clamp(bounds.y, bounds.bottom() - 40);
        }

        if let Some(w) = self.windows.get_mut(&drag.window) {
            w.geometry = new_geom;
        }
    }

    /// Ends a drag, snapping into a Windows-Snap zone if the pointer ended up
    /// near a monitor edge.
    pub fn end_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            let snapped = self.windows.get(&drag.window).and_then(|w| {
                self.monitor_for(w.monitor).and_then(|m| SmartPlacement::snap_zone(w.geometry, m, &self.placement))
            });
            if let (Some(zone), Some(w)) = (snapped, self.windows.get_mut(&drag.window)) {
                w.geometry = zone;
            }
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn start_resize(&mut self, id: WindowId, edge: ResizeEdge, x: i32, y: i32) {
        if let Some(w) = self.windows.get(&id) {
            self.resize = Some(ResizeState { window: id, edge, start_x: x, start_y: y, orig: w.geometry });
            self.focus_window(id);
        }
    }

    pub fn update_resize(&mut self, x: i32, y: i32) {
        let Some(r) = &self.resize else { return };
        let (dx, dy) = (x - r.start_x, y - r.start_y);
        let new_geom = r.edge.apply_delta(r.orig, dx, dy, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);
        if let Some(w) = self.windows.get_mut(&r.window) {
            w.geometry = new_geom;
        }
    }

    pub fn end_resize(&mut self) {
        self.resize = None;
    }

    pub fn is_resizing(&self) -> bool {
        self.resize.is_some()
    }

    /// The edge currently being dragged, if a resize is in progress - so a
    /// backend can keep showing the matching resize cursor for the whole
    /// drag, not just while the pointer happens to still be hovering that
    /// exact edge (which it usually isn't, once the drag is actually
    /// underway).
    pub fn resize_edge(&self) -> Option<ResizeEdge> {
        self.resize.as_ref().map(|r| r.edge)
    }

    // ---- Workspaces -----------------------------------------------------

    pub fn add_workspace(&mut self, name: impl Into<String>, layout: impl Into<String>) -> WorkspaceId {
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        self.workspaces.push(Workspace::new(id, name, layout));
        id
    }

    /// Sets a workspace's display name - used to apply `workspace.names`
    /// at startup (`crates/srdwm/src/main.rs`'s `apply_workspace_count`),
    /// since `WindowManager::new`/`add_workspace` otherwise leave every
    /// workspace named after its own 1-based index regardless of what a
    /// config asked for. A no-op if `id` doesn't exist.
    pub fn rename_workspace(&mut self, id: WorkspaceId, name: impl Into<String>) {
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == id) {
            w.name = name.into();
        }
    }

    pub fn remove_workspace(&mut self, id: WorkspaceId) {
        if self.workspaces.len() <= 1 {
            return;
        }
        let fallback = self.workspaces.iter().map(|w| w.id).find(|&w| w != id).unwrap_or(0);
        for w in self.windows.values_mut().filter(|w| w.workspace == id) {
            w.workspace = fallback;
        }
        self.workspaces.retain(|w| w.id != id);
        if self.current_workspace == id {
            self.current_workspace = fallback;
        }
    }

    /// Switches to `id`, unless `auto_back_and_forth` is set and `id` is
    /// already the current workspace - in which case this jumps to
    /// `previous_workspace` instead, sway's `workspace_auto_back_and_forth`
    /// behavior. `previous_workspace` itself always tracks "whatever was
    /// current right before this call changed it", updated on every real
    /// switch regardless of the setting, so turning the setting on later
    /// (or a client-driven switch, e.g. `ext_workspace_v1`'s `activate`)
    /// doesn't need its own separate bookkeeping.
    pub fn switch_workspace(&mut self, id: WorkspaceId) {
        let target = if self.auto_back_and_forth && id == self.current_workspace { self.previous_workspace } else { id };
        if self.workspaces.iter().any(|w| w.id == target) && target != self.current_workspace {
            self.previous_workspace = self.current_workspace;
            self.current_workspace = target;
        }
    }

    pub fn current_workspace(&self) -> WorkspaceId {
        self.current_workspace
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn move_window_to_workspace(&mut self, id: WindowId, workspace: WorkspaceId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.workspace = workspace;
        }
    }

    /// Windows that should currently be shown to the user: those on the
    /// active workspace of whichever monitor they're assigned to, and not minimized.
    pub fn visible_windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values().filter(|w| w.workspace == self.current_workspace && !w.minimized)
    }

    /// Same windows as [`Self::visible_windows`], but in real front-to-back
    /// stacking order (topmost first) instead of arbitrary `HashMap`
    /// iteration order. Needed anywhere a backend composites more than one
    /// window's elements (content, decoration, border) together and their
    /// relative order across *different* windows actually matters - unlike
    /// `visible_windows`, which is fine for anything per-window in
    /// isolation (border color, geometry) where order never came up.
    /// `self.order` reversed is the same "topmost first" convention
    /// `hit_test`/`window_at` already use.
    pub fn visible_windows_front_to_back(&self) -> impl Iterator<Item = &Window> {
        self.order.iter().rev().filter_map(|id| self.windows.get(id)).filter(|w| w.workspace == self.current_workspace && !w.minimized)
    }

    // ---- Layout -----------------------------------------------------------

    pub fn set_layout(&mut self, workspace: WorkspaceId, layout_name: impl Into<String>) {
        let layout_name = layout_name.into();
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == workspace) {
            w.layout = layout_name;
        }
    }

    pub fn layout_name(&self, workspace: WorkspaceId) -> Option<&str> {
        self.workspace(workspace).map(|w| w.layout.as_str())
    }

    /// Recomputes geometry for all non-floating, non-minimized windows on
    /// `workspace`, grouped by the monitor each window is assigned to, and
    /// applies the results in place. Returns the changed `(id, Rect)` pairs
    /// so a backend can push them to real surfaces.
    pub fn arrange_workspace(&mut self, workspace: WorkspaceId) -> Vec<(WindowId, Rect)> {
        let Some(layout_name) = self.workspace(workspace).map(|w| w.layout.clone()) else {
            return Vec::new();
        };
        let Some(layout) = self.layouts.get(&layout_name) else {
            log::warn!("unknown layout '{layout_name}' for workspace {workspace}");
            return Vec::new();
        };

        // Grouped via `self.order` (insertion/stacking order), not
        // `self.windows.values()`: HashMap iteration order is randomized
        // per-process, which would make master/stack assignment reshuffle
        // unpredictably every time this runs (it runs on every window
        // create/destroy/keybinding).
        let mut by_monitor: HashMap<MonitorId, Vec<WindowId>> = HashMap::new();
        for &id in &self.order {
            let Some(w) = self.windows.get(&id) else { continue };
            // Fullscreen windows own their whole monitor, so tiling must
            // leave them alone, exactly as it does floating ones.
            if w.workspace == workspace && !w.minimized && !w.floating && !w.fullscreen {
                by_monitor.entry(w.monitor).or_default().push(id);
            }
        }

        let mut monitor_ids: Vec<MonitorId> = by_monitor.keys().copied().collect();
        monitor_ids.sort_unstable();

        let mut changes = Vec::new();
        for monitor_id in monitor_ids {
            let ids = &by_monitor[&monitor_id];
            let Some(monitor) = self.monitor_for(monitor_id).cloned() else { continue };
            let placements = layout.arrange(ids, &monitor, &self.tiling);
            for (id, rect) in placements {
                if let Some(w) = self.windows.get_mut(&id) {
                    w.geometry = rect;
                }
                changes.push((id, rect));
            }
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wm_with_monitor() -> WindowManager {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![{
            let mut m = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1080));
            m.primary = true;
            m
        }]);
        wm
    }

    #[test]
    fn new_window_on_dynamic_workspace_uses_smart_placement() {
        let mut wm = wm_with_monitor();
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "first");
        w.geometry = Rect::new(0, 0, 400, 300);
        wm.add_window(w);
        let placed = wm.window(id).unwrap().geometry;
        // Grid placement starts at grid_margin, not (0,0).
        assert_eq!(placed.x, wm.placement.grid_margin as i32);
    }

    #[test]
    fn tiling_workspace_arranges_two_windows_side_by_side() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));

        wm.arrange_workspace(wm.current_workspace());
        let ra = wm.window(a).unwrap().geometry;
        let rb = wm.window(b).unwrap().geometry;
        assert!(!ra.overlaps(&rb));
        assert_eq!(ra.y, rb.y);
        assert!(ra.x < rb.x);
    }

    #[test]
    fn floating_window_is_skipped_by_tiling_arrange() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        wm.toggle_floating(a);
        let before = wm.window(a).unwrap().geometry;
        wm.arrange_workspace(wm.current_workspace());
        assert_eq!(wm.window(a).unwrap().geometry, before);
    }

    #[test]
    fn focus_cycles_forward_and_wraps() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        // `b` was added last, so it's focused.
        assert_eq!(wm.focused_id(), Some(b));
        wm.focus_next();
        assert_eq!(wm.focused_id(), Some(a));
        wm.focus_next();
        assert_eq!(wm.focused_id(), Some(b));
    }

    #[test]
    fn minimized_window_is_skipped_by_focus_cycling() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        wm.minimize_window(a);
        wm.focus_window(b);
        wm.focus_next();
        assert_eq!(wm.focused_id(), Some(b), "only unminimized window should ever be focused");
    }

    #[test]
    fn drag_moves_window_by_pointer_delta() {
        let mut wm = wm_with_monitor();
        // "tiling" layout leaves add_window's requested geometry alone;
        // "dynamic"/"floating" would override it via SmartPlacement, which
        // these tests aren't exercising.
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(300, 300, 400, 300);
        wm.add_window(w);
        wm.start_drag(a, 310, 310);
        wm.update_drag(360, 340);
        let g = wm.window(a).unwrap().geometry;
        assert_eq!((g.x, g.y), (350, 330));
        wm.end_drag();
        assert!(!wm.is_dragging());
    }

    #[test]
    fn drag_ending_near_edge_snaps_to_half_screen() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(500, 500, 400, 300);
        wm.add_window(w);
        wm.start_drag(a, 510, 510);
        wm.update_drag(15, 510); // drag far left, landing within snap_threshold (8px) of edge 0
        wm.end_drag();
        let g = wm.window(a).unwrap().geometry;
        assert_eq!(g, Rect::new(0, 0, 960, 1080));
    }

    #[test]
    fn resize_from_bottom_right_grows_size_only() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.start_resize(a, ResizeEdge::BottomRight, 400, 300);
        wm.update_resize(450, 340);
        let g = wm.window(a).unwrap().geometry;
        assert_eq!(g, Rect::new(100, 100, 350, 240));
        wm.end_resize();
        assert!(!wm.is_resizing());
    }

    #[test]
    fn toggle_maximize_restores_original_geometry() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        let original = wm.window(a).unwrap().geometry;
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().geometry, Rect::new(0, 0, 1920, 1080));
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().geometry, original);
    }

    #[test]
    fn maximize_records_anim_from_when_animations_enabled() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        let placed = wm.window(a).unwrap().geometry;
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().anim_from, Some(placed));
    }

    #[test]
    fn maximize_does_not_record_anim_from_when_animations_disabled() {
        let mut wm = wm_with_monitor();
        wm.animations_enabled = false;
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().anim_from, None);
    }

    #[test]
    fn fullscreen_records_anim_from_covering_the_full_monitor() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        let placed = wm.window(a).unwrap().geometry;
        wm.toggle_fullscreen(a);
        assert_eq!(wm.window(a).unwrap().anim_from, Some(placed));
    }

    #[test]
    fn directional_focus_picks_nearest_window_in_that_direction() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let center = wm.alloc_window_id();
        let mut wc = Window::new(center, "center");
        wc.geometry = Rect::new(500, 500, 100, 100);
        wm.add_window(wc);
        let left = wm.alloc_window_id();
        let mut wl = Window::new(left, "left");
        wl.geometry = Rect::new(0, 500, 100, 100);
        wm.add_window(wl);
        let right = wm.alloc_window_id();
        let mut wr = Window::new(right, "right");
        wr.geometry = Rect::new(1000, 500, 100, 100);
        wm.add_window(wr);

        wm.focus_window(center);
        assert_eq!(wm.focus_direction(Direction::Left), Some(left));
        assert_eq!(wm.focused_id(), Some(left));

        wm.focus_window(center);
        assert_eq!(wm.focus_direction(Direction::Right), Some(right));
    }

    #[test]
    fn hit_test_prefers_topmost_window() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut wa = Window::new(a, "a");
        wa.geometry = Rect::new(0, 0, 400, 300);
        wm.add_window(wa);
        let b = wm.alloc_window_id();
        let mut wb = Window::new(b, "b");
        wb.geometry = Rect::new(0, 0, 400, 300); // fully overlapping, added later -> on top
        wm.add_window(wb);

        let (hit_id, hit) = wm.hit_test(200, 10).unwrap();
        assert_eq!(hit_id, b);
        assert_eq!(hit, TitlebarHit::Drag);
    }

    #[test]
    fn moving_window_to_another_workspace_removes_it_from_current() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.move_window_to_workspace(a, ws2);
        assert_eq!(wm.visible_windows().count(), 0);
        wm.switch_workspace(ws2);
        assert_eq!(wm.visible_windows().count(), 1);
    }

    #[test]
    fn matching_rule_floats_new_window_on_add() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { title_contains: Some("calculator".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { floating: Some(true), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "Calculator"));
        assert!(wm.is_floating(id));
    }

    #[test]
    fn non_matching_rule_leaves_window_untouched() {
        let mut wm = wm_with_monitor();
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { title_contains: Some("calculator".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { floating: Some(true), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "Terminal"));
        assert!(!wm.is_floating(id));
    }

    #[test]
    fn rule_assigns_window_to_target_workspace() {
        let mut wm = wm_with_monitor();
        let target = wm.add_workspace("scratch", "dynamic");
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("scratchpad".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { workspace: Some(target), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "notes");
        w.app_id = "scratchpad".into();
        wm.add_window(w);
        assert_eq!(wm.window(id).unwrap().workspace, target);
    }

    #[test]
    fn removing_a_workspace_reassigns_its_windows() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        wm.remove_workspace(ws2);
        assert_ne!(wm.window(a).unwrap().workspace, ws2);
        assert!(wm.workspace(ws2).is_none());
    }

    #[test]
    fn rename_workspace_changes_the_display_name() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.rename_workspace(ws2, "code");
        assert_eq!(wm.workspace(ws2).unwrap().name, "code");
    }

    #[test]
    fn auto_back_and_forth_jumps_to_the_previous_workspace_when_reselecting_the_active_one() {
        let mut wm = wm_with_monitor();
        wm.auto_back_and_forth = true;
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), ws2);
        // Re-selecting the already-active workspace jumps back to 0, the
        // one that was active right before.
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), 0);
    }

    #[test]
    fn without_auto_back_and_forth_reselecting_the_active_workspace_is_a_plain_no_op() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), ws2);
    }

    #[test]
    fn switching_to_a_nonexistent_workspace_does_not_move_or_touch_previous() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        wm.switch_workspace(9999);
        assert_eq!(wm.current_workspace(), ws2);
        // The failed switch must not have overwritten `previous_workspace`
        // either - auto_back_and_forth would otherwise jump to a
        // workspace id that was never really visited.
        wm.auto_back_and_forth = true;
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), 0);
    }

    #[test]
    fn rename_workspace_is_a_no_op_for_an_id_that_does_not_exist() {
        let mut wm = wm_with_monitor();
        wm.rename_workspace(9999, "ghost");
        assert!(wm.workspaces().iter().all(|w| w.name != "ghost"));
    }

    // ---- Scratchpad --------------------------------------------------------

    #[test]
    fn scratchpad_add_hides_the_window_and_marks_pool_membership() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        let w = wm.window(a).unwrap();
        assert!(w.scratchpad);
        assert!(w.minimized);
        assert!(w.floating);
        assert!(!wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_brings_back_the_hidden_window_and_focuses_it() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        wm.scratchpad_show();
        let w = wm.window(a).unwrap();
        assert!(!w.minimized);
        assert_eq!(wm.focused_id(), Some(a));
        assert!(wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_hides_again_when_the_shown_scratchpad_window_is_focused() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        wm.scratchpad_show(); // shows + focuses
        wm.scratchpad_show(); // toggles back off
        assert!(wm.window(a).unwrap().minimized);
        assert!(!wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_moves_the_window_onto_the_current_workspace() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        wm.scratchpad_show();
        assert_eq!(wm.window(a).unwrap().workspace, ws2);
        assert!(wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_with_no_scratchpad_windows_is_a_no_op() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "normal"));
        wm.scratchpad_show();
        assert_eq!(wm.focused_id(), Some(a));
        assert!(!wm.window(a).unwrap().minimized);
    }

    #[test]
    fn scratchpad_show_picks_the_most_recently_added_hidden_window() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "old"));
        wm.scratchpad_add(a);
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "new"));
        wm.scratchpad_add(b);
        wm.scratchpad_show();
        assert_eq!(wm.focused_id(), Some(b));
        assert!(wm.window(a).unwrap().minimized);
    }

    #[test]
    fn scratchpad_remove_leaves_current_visibility_untouched_but_drops_pool_membership() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        wm.scratchpad_remove(a);
        assert!(!wm.window(a).unwrap().scratchpad);
        assert!(wm.window(a).unwrap().minimized);
        // No longer scratchpad-managed, so a later `scratchpad_show` must
        // not touch it.
        wm.scratchpad_show();
        assert!(wm.window(a).unwrap().minimized);
    }

    // ---- Monitor hotplug -------------------------------------------------

    fn two_monitors() -> Vec<Monitor> {
        let mut a = Monitor::new(0, "primary", Rect::new(0, 0, 1280, 800));
        a.primary = true;
        let b = Monitor::new(1, "secondary", Rect::new(1280, 0, 1920, 1080));
        vec![a, b]
    }

    #[test]
    fn unplugging_a_monitor_rehomes_its_windows_to_the_primary() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "on-second-monitor");
        w.geometry = Rect::new(1500, 200, 600, 400); // inside monitor 1 only
        wm.add_window(w);
        wm.window_mut(id).unwrap().monitor = 1;

        // Monitor 1 goes away.
        wm.set_monitors(vec![two_monitors().remove(0)]);

        let w = wm.window(id).unwrap();
        assert_eq!(w.monitor, 0, "window should be rehomed to the primary monitor");
        assert!(
            Rect::new(0, 0, 1280, 800).overlaps(&w.geometry),
            "rehomed window should be on-screen, got {:?}",
            w.geometry
        );
    }

    #[test]
    fn windows_already_on_a_surviving_monitor_are_left_alone() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "on-primary");
        w.geometry = Rect::new(10, 20, 300, 200);
        wm.add_window(w);
        wm.window_mut(id).unwrap().monitor = 0;
        wm.window_mut(id).unwrap().geometry = Rect::new(10, 20, 300, 200);

        wm.set_monitors(vec![two_monitors().remove(0)]);

        let w = wm.window(id).unwrap();
        assert_eq!(w.monitor, 0);
        assert_eq!(w.geometry, Rect::new(10, 20, 300, 200), "untouched window must not move");
    }

    #[test]
    fn a_window_still_overlapping_the_primary_keeps_its_geometry() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "straddling");
        wm.add_window(w.clone());
        // Straddles the boundary, so it still overlaps the primary.
        w.geometry = Rect::new(1200, 100, 400, 300);
        wm.window_mut(id).unwrap().monitor = 1;
        wm.window_mut(id).unwrap().geometry = w.geometry;

        wm.set_monitors(vec![two_monitors().remove(0)]);

        let got = wm.window(id).unwrap();
        assert_eq!(got.monitor, 0, "monitor id must still be remapped");
        assert_eq!(got.geometry, Rect::new(1200, 100, 400, 300), "already-visible geometry should be kept");
    }

    #[test]
    fn losing_every_monitor_leaves_windows_intact_for_when_one_returns() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "orphan");
        w.geometry = Rect::new(1500, 200, 600, 400);
        wm.add_window(w);
        wm.window_mut(id).unwrap().monitor = 1;
        wm.window_mut(id).unwrap().geometry = Rect::new(1500, 200, 600, 400);

        wm.set_monitors(Vec::new());

        let got = wm.window(id).unwrap();
        assert_eq!(got.geometry, Rect::new(1500, 200, 600, 400));
        assert_eq!(got.monitor, 1);
    }

    #[test]
    fn a_window_whose_monitor_field_is_stale_is_still_rescued() {
        // Regression: `add_window` assigns `monitor` from the *primary*
        // monitor, so a window placed on the second monitor by a rule (or
        // dragged there) keeps `monitor == 0`. Rehoming that keyed off the
        // field alone skipped this window entirely and left it off-screen.
        // Reproduced live by unplugging a monitor out from under an xterm.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let w = Window::new(id, "placed-by-rule");
        wm.add_window(w);
        // Geometry on monitor 1, but `monitor` still says 0 - exactly what
        // add_window + a geometry rule produce.
        wm.window_mut(id).unwrap().geometry = Rect::new(1500, 200, 600, 400);
        assert_eq!(wm.window(id).unwrap().monitor, 0, "precondition: stale field");

        wm.set_monitors(vec![two_monitors().remove(0)]);

        let got = wm.window(id).unwrap();
        assert!(
            Rect::new(0, 0, 1280, 800).overlaps(&got.geometry),
            "window must be pulled back on-screen, got {:?}",
            got.geometry
        );
    }

    // ---- Fullscreen ------------------------------------------------------

    #[test]
    fn fullscreen_covers_the_monitor_and_restores_the_original_geometry() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "app");
        w.geometry = Rect::new(100, 100, 400, 300);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(100, 100, 400, 300);

        wm.toggle_fullscreen(id);
        let got = wm.window(id).unwrap();
        assert!(got.fullscreen);
        assert_eq!(got.geometry, Rect::new(0, 0, 1280, 800), "should cover the whole monitor");
        assert!(!got.decorated, "fullscreen must drop the titlebar");

        wm.toggle_fullscreen(id);
        let got = wm.window(id).unwrap();
        assert!(!got.fullscreen);
        assert_eq!(got.geometry, Rect::new(100, 100, 400, 300));
        assert!(got.decorated);
    }

    #[test]
    fn fullscreen_round_trip_restores_a_client_side_decorated_window_to_undecorated() {
        // Regression test: exiting fullscreen used to hardcode
        // `decorated = true` unconditionally, which is only correct for a
        // window that was decorated to begin with. A window a rule sets
        // `decorated = false` for (client-side-decorated apps like
        // Firefox) that goes fullscreen and back used to come back
        // permanently `decorated = true` - with nothing to ever set it
        // back, since the client only negotiates its decoration mode once.
        // Since border/titlebar hit-testing is keyed off `Window.decorated`
        // directly, this made srdwm swallow every click near the top of
        // the window as a fake titlebar hit instead of forwarding it to
        // the client.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "firefox");
        w.geometry = Rect::new(100, 100, 400, 300);
        w.decorated = false;
        wm.add_window(w);

        wm.toggle_fullscreen(id);
        assert!(!wm.window(id).unwrap().decorated, "fullscreen itself must still drop the titlebar");

        wm.toggle_fullscreen(id);
        assert!(!wm.window(id).unwrap().decorated, "must restore the pre-fullscreen decorated=false, not default to true");
    }

    /// A monitor whose usable `geometry` is shrunk by a bottom dock's
    /// exclusive zone, distinct from its true `full_geometry` - the shape
    /// every real backend reports once a bar/dock has claimed space (see
    /// `Monitor::full_geometry`'s doc comment).
    fn monitor_with_dock() -> Monitor {
        let mut m = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1020));
        m.full_geometry = Rect::new(0, 0, 1920, 1080);
        m.primary = true;
        m
    }

    #[test]
    fn fullscreen_covers_the_full_monitor_ignoring_a_dock_reservation() {
        // Regression test: fullscreen used to target `Monitor::geometry`
        // (the usable, exclusive-zone-shrunk area), the same field maximize
        // correctly uses - so a fullscreened window stopped short of a
        // dock's reserved strip instead of covering (or going under) it
        // like fullscreen does everywhere else. `full_geometry` is what
        // fixes that; `geometry` must stay untouched so maximize keeps
        // respecting the dock.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.toggle_fullscreen(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080), "fullscreen must reach the true monitor edge, past the dock");
    }

    #[test]
    fn maximize_still_respects_the_dock_reservation() {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.toggle_maximize(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1020), "maximize must still stop at the dock, unlike fullscreen");
    }

    #[test]
    fn maximized_window_grows_when_the_dock_drops_its_reservation_live() {
        // Regression test: a dock that hides/reduces its exclusive zone
        // while a window is already maximized (an auto-hide dock reacting
        // to monocle/maximize, exactly the scenario an AGS peer session hit
        // live) used to leave that window stuck at its stale, dock-shrunk
        // size - `set_monitors` updated `Monitor::geometry` correctly but
        // never touched already-maximized/fullscreen windows' `geometry`,
        // so nothing re-grew until the window was manually un-maximized and
        // re-maximized.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        wm.toggle_maximize(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1020));

        // The dock drops its exclusive zone to 0.
        let mut freed = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1080));
        freed.full_geometry = Rect::new(0, 0, 1920, 1080);
        freed.primary = true;
        wm.set_monitors(vec![freed]);

        assert_eq!(
            wm.window(id).unwrap().geometry,
            Rect::new(0, 0, 1920, 1080),
            "an already-maximized window must live-track a monitor geometry change, not just windows placed afterward"
        );
    }

    #[test]
    fn fullscreen_window_also_live_tracks_a_monitor_geometry_change() {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        wm.toggle_fullscreen(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080));

        let mut resized = Monitor::new(0, "primary", Rect::new(0, 0, 2560, 1420));
        resized.full_geometry = Rect::new(0, 0, 2560, 1440);
        resized.primary = true;
        wm.set_monitors(vec![resized]);

        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 2560, 1440), "fullscreen must live-track the true full rect, not the usable one");
    }

    #[test]
    fn a_non_maximized_window_is_left_alone_by_a_monitor_geometry_change() {
        // set_monitors' new re-sync pass is gated on maximized/fullscreen --
        // must not clobber an ordinary floating/tiled window's geometry just
        // because the monitor rect changed underneath it.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        w.geometry = Rect::new(100, 100, 400, 300);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(100, 100, 400, 300);

        let mut freed = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1080));
        freed.full_geometry = Rect::new(0, 0, 1920, 1080);
        freed.primary = true;
        wm.set_monitors(vec![freed]);

        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(100, 100, 400, 300));
    }

    #[test]
    fn dragging_a_window_can_cross_into_the_dock_reserved_strip() {
        // Regression test: `update_drag`'s clamp used to also use
        // `Monitor::geometry` (the shrunk usable area), which made it
        // physically impossible to ever drag a floating window into the
        // strip a dock reserves - not just discouraged, genuinely
        // unreachable at any drag speed or angle. `full_geometry` is what
        // makes that space reachable again; the dock still renders on top
        // as an overlay, same as it does everywhere else.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        w.geometry = Rect::new(500, 500, 200, 200);
        wm.add_window(w);

        wm.start_drag(id, 600, 600);
        // Drag far down - past the old usable-area bottom (1020) and
        // toward the true monitor bottom (1080).
        wm.update_drag(600, 5000);
        let g = wm.window(id).unwrap().geometry;
        // Old behavior (clamped to `geometry`, bottom 1020) would stop at
        // y=980; clamped to `full_geometry` (bottom 1080), it reaches 1040.
        assert_eq!(g.y, 1040, "must clamp against the true monitor bottom, not the dock-shrunk usable area");
    }

    #[test]
    fn class_rule_applies_once_app_id_is_known_after_creation() {
        // Regression test: `add_window` matches rules against whatever
        // `app_id`/`title` the window already has - for a native Wayland
        // client those are still empty at that moment (the real values
        // only arrive on a later commit, well after `new_toplevel`), so
        // every class-based rule - including `srd.rule({ class =
        // "firefox" }, { decorated = false })`, meant to stop srdwm
        // drawing a second titlebar over Firefox's own - silently never
        // matched. `reapply_rules_if_pending` is the retry a backend calls
        // once the real app_id is known.
        let mut wm = wm_with_monitor();
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("firefox".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { decorated: Some(false), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        // Empty app_id, exactly as a fresh native Wayland toplevel has it.
        wm.add_window(Window::new(id, ""));
        assert!(wm.window(id).unwrap().decorated, "no app_id yet, so no match - must not have flipped early");

        let w = wm.window_mut(id).unwrap();
        w.app_id = "firefox".into();
        wm.reapply_rules_if_pending(id);
        assert!(!wm.window(id).unwrap().decorated, "app_id now known - the rule must apply on retry");

        // A later, unrelated title change (e.g. a browser tab switching)
        // must not re-match and re-apply - rule actions apply once.
        let w = wm.window_mut(id).unwrap();
        w.decorated = true;
        w.title = "a new tab title".into();
        wm.reapply_rules_if_pending(id);
        assert!(wm.window(id).unwrap().decorated, "rules_applied is already true - must not re-run the match");
    }

    #[test]
    fn opacity_rule_applies_on_the_deferred_retry_same_as_other_actions() {
        // Regression test: `opacity` was added to `add_window`'s own rule
        // application but missed here, in the deferred retry
        // `reapply_rules_if_pending` - confirmed live: a rule like
        // `srd.rule({ class = "Alacritty" }, { opacity = 0.4 })` never took
        // effect for any real native Wayland client, since (per the test
        // above) that's the *only* path a class-based rule actually
        // matches through for one of those - `add_window`'s own match
        // attempt always fails first, against an as-yet-empty `app_id`.
        let mut wm = wm_with_monitor();
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("alacritty".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { opacity: Some(0.4), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, ""));
        assert_eq!(wm.window(id).unwrap().opacity, 1.0, "no app_id yet, so no match - must not have applied early");

        let w = wm.window_mut(id).unwrap();
        w.app_id = "Alacritty".into();
        wm.reapply_rules_if_pending(id);
        assert_eq!(wm.window(id).unwrap().opacity, 0.4, "app_id now known - the rule must apply on retry");
    }

    #[test]
    fn fullscreen_from_maximized_still_restores_the_pre_maximize_size() {
        // Both share `restore_geometry`; entering fullscreen from a
        // maximised window must not overwrite it with the monitor rect, or
        // the window could never get its real size back.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "app");
        w.geometry = Rect::new(50, 60, 300, 200);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(50, 60, 300, 200);

        wm.toggle_maximize(id);
        wm.toggle_fullscreen(id);
        assert!(wm.is_fullscreen(id));
        assert!(!wm.window(id).unwrap().maximized, "the two states are mutually exclusive");

        wm.toggle_fullscreen(id);
        assert_eq!(
            wm.window(id).unwrap().geometry,
            Rect::new(50, 60, 300, 200),
            "must restore the size from before maximise, not the monitor rect"
        );
    }

    #[test]
    fn tiling_leaves_fullscreen_windows_alone() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "tiled"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "full"));
        wm.toggle_fullscreen(b);

        let changes = wm.arrange_workspace(wm.current_workspace());
        assert!(
            !changes.iter().any(|(id, _)| *id == b),
            "a fullscreen window must not be re-tiled"
        );
        assert_eq!(wm.window(b).unwrap().geometry, Rect::new(0, 0, 1280, 800));
    }

    // ---- Directional move ------------------------------------------------

    #[test]
    fn moving_a_window_swaps_it_with_its_neighbour() {
        let mut wm = wm_with_monitor();
        let left = wm.alloc_window_id();
        let mut a = Window::new(left, "left");
        a.geometry = Rect::new(0, 0, 400, 400);
        wm.add_window(a);
        wm.window_mut(left).unwrap().geometry = Rect::new(0, 0, 400, 400);

        let right = wm.alloc_window_id();
        let mut b = Window::new(right, "right");
        b.geometry = Rect::new(600, 0, 400, 400);
        wm.add_window(b);
        wm.window_mut(right).unwrap().geometry = Rect::new(600, 0, 400, 400);

        wm.focus_window(left);
        let swapped = wm.move_window_direction(Direction::Right);

        assert_eq!(swapped, Some(right));
        assert_eq!(wm.window(left).unwrap().geometry, Rect::new(600, 0, 400, 400));
        assert_eq!(wm.window(right).unwrap().geometry, Rect::new(0, 0, 400, 400));
    }

    #[test]
    fn moving_with_no_neighbour_pushes_to_the_monitor_edge() {
        let mut wm = wm_with_monitor();
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "only");
        w.geometry = Rect::new(500, 300, 200, 150);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(500, 300, 200, 150);
        wm.focus_window(id);

        assert_eq!(wm.move_window_direction(Direction::Left), None);
        assert_eq!(wm.window(id).unwrap().geometry.x, 0, "should hug the left edge");

        wm.move_window_direction(Direction::Down);
        let g = wm.window(id).unwrap().geometry;
        let mon = wm.primary_monitor().unwrap().geometry;
        assert_eq!(g.bottom(), mon.bottom(), "should hug the bottom edge");
    }

    #[test]
    fn swapping_also_reorders_the_stack_so_tiling_follows() {
        // Under tiling the layout assigns slots from `order`, so a swap that
        // only exchanged geometry would be undone by the next arrange.
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        wm.arrange_workspace(wm.current_workspace());

        // Snapshot *after* focusing: `focus_window` raises, which reorders
        // on its own and would otherwise mask what the move did.
        wm.focus_window(a);
        let order_before: Vec<_> = wm.stacking_order().map(|w| w.id).collect();
        wm.move_window_direction(Direction::Right);
        let order_after: Vec<_> = wm.stacking_order().map(|w| w.id).collect();

        assert_ne!(order_before, order_after, "stacking order must reflect the swap");
        assert_eq!(
            order_after,
            order_before.iter().rev().copied().collect::<Vec<_>>(),
            "the two windows should have traded places in the stack"
        );
    }

    // ---- Always on top ---------------------------------------------------

    #[test]
    fn pinned_windows_stay_above_newly_raised_ones() {
        let mut wm = wm_with_monitor();
        let pinned = wm.alloc_window_id();
        wm.add_window(Window::new(pinned, "pip"));
        let other = wm.alloc_window_id();
        wm.add_window(Window::new(other, "normal"));

        wm.toggle_always_on_top(pinned);
        assert!(wm.is_always_on_top(pinned));
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned));

        // Raising a normal window must not bury the pinned one.
        wm.raise_window(other);
        assert_eq!(
            wm.stacking_order().last().map(|w| w.id),
            Some(pinned),
            "pinned window must remain topmost after another is raised"
        );
    }

    #[test]
    fn a_new_window_does_not_cover_a_pinned_one() {
        let mut wm = wm_with_monitor();
        let pinned = wm.alloc_window_id();
        wm.add_window(Window::new(pinned, "pip"));
        wm.toggle_always_on_top(pinned);

        let fresh = wm.alloc_window_id();
        wm.add_window(Window::new(fresh, "just opened"));

        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned));
    }

    #[test]
    fn unpinning_lets_a_window_fall_back_into_the_normal_stack() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));

        wm.toggle_always_on_top(a);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(a));
        wm.toggle_always_on_top(a);
        wm.raise_window(b);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(b));
    }

    #[test]
    fn lower_window_sends_it_to_the_back_of_the_stack() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        let c = wm.alloc_window_id();
        wm.add_window(Window::new(c, "c"));
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(c), "precondition: c is on top after being added last");

        wm.lower_window(c);
        let order: Vec<_> = wm.stacking_order().map(|w| w.id).collect();
        assert_eq!(order, vec![c, a, b], "c must be at the very back, a/b unchanged relative to each other");
    }

    #[test]
    fn lower_window_never_buries_a_pinned_window() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let pinned = wm.alloc_window_id();
        wm.add_window(Window::new(pinned, "pinned"));
        wm.toggle_always_on_top(pinned);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned));

        wm.lower_window(a);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned), "a pinned window must stay on top even after an unrelated lower_window call");
    }
}
