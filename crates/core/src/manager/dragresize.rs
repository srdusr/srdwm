//! Interactive drag and resize session state.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
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

}
