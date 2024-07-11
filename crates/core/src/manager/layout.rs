//! Layout selection and running a workspace's active layout to produce final geometries.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
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
