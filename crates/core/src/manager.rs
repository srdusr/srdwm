use crate::geometry::Rect;
use crate::layout::{Layout, MasterStackLayout, NoOpLayout, TilingConfig};
use crate::monitor::{Monitor, MonitorId};
use crate::placement::{PlacementConfig, SmartPlacement, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};
use crate::window::{ResizeEdge, TitlebarHit, Window, WindowId};
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
    next_workspace_id: WorkspaceId,
    next_window_id: WindowId,
    layouts: HashMap<String, Box<dyn Layout>>,
    pub tiling: TilingConfig,
    pub placement: PlacementConfig,
    drag: Option<DragState>,
    resize: Option<ResizeState>,
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
            next_workspace_id: 1,
            next_window_id: 1,
            layouts,
            tiling: TilingConfig::default(),
            placement: PlacementConfig::default(),
            drag: None,
            resize: None,
        }
    }

    pub fn register_layout(&mut self, name: impl Into<String>, layout: Box<dyn Layout>) {
        self.layouts.insert(name.into(), layout);
    }

    pub fn available_layouts(&self) -> Vec<&str> {
        self.layouts.keys().map(String::as_str).collect()
    }

    // ---- Monitors ----------------------------------------------------

    pub fn set_monitors(&mut self, monitors: Vec<Monitor>) {
        self.monitors = monitors;
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
        window.workspace = self.current_workspace;
        if let Some(monitor) = self.primary_monitor() {
            window.monitor = monitor.id;
            let layout_name = self.workspace(self.current_workspace).map(|w| w.layout.clone()).unwrap_or_default();
            if layout_name != "tiling" {
                let existing: Vec<Rect> = self.windows_on_workspace(self.current_workspace).map(|w| w.geometry).collect();
                let size = (window.geometry.width, window.geometry.height);
                window.geometry = SmartPlacement::place(monitor, &existing, size, &self.placement);
            }
        }
        self.windows.insert(id, window);
        self.order.push(id);
        self.focused = Some(id);
        id
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
    pub fn focus_direction(&mut self, dir: Direction) -> Option<WindowId> {
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
            // Distance biased toward the requested axis so a window that's
            // mostly to the left (small |dy|) beats one that's diagonally
            // placed, matching how i3/sway-style directional focus feels.
            let (primary, secondary) = match dir {
                Direction::Left | Direction::Right => (dx, dy),
                Direction::Up | Direction::Down => (dy, dx),
            };
            let dist = primary * primary + secondary * secondary * 4;
            if best.is_none_or(|(_, d)| dist < d) {
                best = Some((w.id, dist));
            }
        }
        let target = best.map(|(id, _)| id);
        if let Some(id) = target {
            self.focus_window(id);
        }
        target
    }

    // ---- Window operations ----------------------------------------------

    pub fn close_window(&mut self, id: WindowId) {
        log::info!("close_window({id})");
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

    pub fn toggle_maximize(&mut self, id: WindowId) {
        let monitor_geom = self.windows.get(&id).and_then(|w| self.monitor_for(w.monitor)).map(|m| m.geometry);
        let Some(w) = self.windows.get_mut(&id) else { return };
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
            if let Some(hit) = ResizeEdge::hit_test(w.geometry, x, y) {
                return Some((w.id, hit));
            }
        }
        None
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

        let monitor_bounds = self.windows.get(&drag.window).and_then(|w| self.monitor_for(w.monitor)).map(|m| m.geometry);
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

    // ---- Workspaces -----------------------------------------------------

    pub fn add_workspace(&mut self, name: impl Into<String>, layout: impl Into<String>) -> WorkspaceId {
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        self.workspaces.push(Workspace::new(id, name, layout));
        id
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

    pub fn switch_workspace(&mut self, id: WorkspaceId) {
        if self.workspaces.iter().any(|w| w.id == id) {
            self.current_workspace = id;
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
            if w.workspace == workspace && !w.minimized && !w.floating {
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
        wm.update_drag(20, 510); // drag far left, within snap threshold of edge 0
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
}
