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
        //
        // `all_monitors_bounds`, not `monitor_for(w.monitor)` (the window's
        // own *starting* monitor, looked up once and never updated as the
        // drag moves) - see that function's own doc comment for the real
        // multi-monitor bug this fixes: the old single-monitor clamp made
        // it mathematically impossible to ever drag a window from one
        // monitor onto another, confirmed live with two real monitors
        // connected, one of them otherwise fully working at the
        // compositor/DRM level.
        let monitor_bounds = self.all_monitors_bounds();
        if let Some(bounds) = monitor_bounds {
            new_geom.x = new_geom.x.clamp(bounds.x - new_geom.width as i32 + 40, bounds.right() - 40);
            new_geom.y = new_geom.y.clamp(bounds.y, bounds.bottom() - 40);
        }

        // Live, every motion tick - not just once at `end_drag`, which
        // used to be the only place this got corrected (see its own doc
        // comment on why `w.monitor` goes stale at all). Between here and
        // there, `state/geometry.rs::sync_geometry` - called on every one
        // of these same motion ticks while a drag is active - reads this
        // exact field to pick which monitor's `scale` converts the
        // client's real physical size into the logical points `xdg_
        // toplevel::configure` sends it. Two real monitors at genuinely
        // different scales (confirmed live: `1.0` and `~0.84`), a window
        // dragged from one onto the other kept computing every mid-drag
        // configure against the *origin* monitor's scale for the drag's
        // entire remaining duration - the client resizing itself to a
        // logical size that doesn't match the physical footprint the
        // border/decoration are actually drawing around it, only self-
        // correcting the instant the button came up. Reported live as a
        // dragged window "looking very messed up" on the other monitor.
        let now_on = self.monitors.iter().find(|m| m.geometry.overlaps(&new_geom)).map(|m| m.id);

        if let Some(w) = self.windows.get_mut(&drag.window) {
            w.geometry = new_geom;
            if let Some(now_on_id) = now_on {
                w.monitor = now_on_id;
            }
        }
    }

    /// Ends a drag, snapping into a Windows-Snap zone if the pointer ended up
    /// near a monitor edge.
    pub fn end_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            // `update_drag` above already keeps `w.monitor` live on every
            // motion tick now, so this is normally just confirming what's
            // already current - kept anyway as the final word before
            // computing the snap zone below (a drag that starts and ends
            // between two motion ticks, however unlikely, would otherwise
            // check the *wrong* monitor's snap zones), the same bug this
            // was originally fixing for maximize/fullscreen one level up.
            if let Some(w) = self.windows.get(&drag.window) {
                if let Some(now_on) = self.monitors.iter().find(|m| m.geometry.overlaps(&w.geometry)) {
                    let now_on_id = now_on.id;
                    if let Some(w) = self.windows.get_mut(&drag.window) {
                        w.monitor = now_on_id;
                    }
                }
            }
            let snapped = self.windows.get(&drag.window).and_then(|w| {
                self.monitor_for(w.monitor).and_then(|m| SmartPlacement::snap_zone(w.geometry, m, &self.placement))
            });
            if let (Some(zone), Some(w)) = (snapped, self.windows.get_mut(&drag.window)) {
                w.geometry = zone;
            }
            // Remembers this app's new position (not just `end_resize`'s
            // size) for its *next* window - see `remembered_geometry`'s
            // own doc comment. Deliberately reads geometry *after* the
            // snap-zone check just above: a drag that ends in a snap
            // remembers the snapped position/size, matching what the user
            // actually sees settle, not the raw pre-snap drop point.
            if let Some(w) = self.windows.get(&drag.window) {
                if !w.app_id.is_empty() {
                    self.remembered_geometry.insert(w.app_id.clone(), (w.geometry.x, w.geometry.y, w.geometry.width, w.geometry.height));
                }
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
        let mut new_geom = r.edge.apply_delta(r.orig, dx, dy, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);
        // `Window::aspect_ratio`'s own doc comment: a locked-ratio window
        // (the "phone monitor" case, concretely) re-derives one dimension
        // from the other here, on top of the ordinary delta above, rather
        // than needing a second, separate resize code path.
        if let Some(ratio) = self.windows.get(&r.window).and_then(|w| w.aspect_ratio) {
            new_geom = r.edge.apply_aspect_ratio(new_geom, ratio, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);
        }
        // Same live `w.monitor` correction as `update_drag`'s own doc
        // comment explains - a resize can cross a monitor boundary at
        // the edge being dragged just as easily as a drag can carry the
        // whole window across one, and `sync_geometry`'s per-tick scale
        // lookup doesn't care which kind of geometry change put the
        // window there.
        let now_on = self.monitors.iter().find(|m| m.geometry.overlaps(&new_geom)).map(|m| m.id);
        if let Some(w) = self.windows.get_mut(&r.window) {
            w.geometry = new_geom;
            if let Some(now_on_id) = now_on {
                w.monitor = now_on_id;
            }
        }
    }

    pub fn end_resize(&mut self) {
        // Remembers this app's new size for its *next* window - see
        // `remembered_sizes`' own doc comment for why this is the one
        // resize-ending path that updates it (not maximize/fullscreen, not
        // a drag-to-edge snap). Keyed by `app_id`, so a window that never
        // got one (a backend/client that hasn't reported it yet) simply
        // isn't remembered - no worse than today, and consistent with how
        // window rules already treat an empty `app_id` as unmatchable.
        if let Some(r) = &self.resize {
            if let Some(w) = self.windows.get(&r.window) {
                if !w.app_id.is_empty() {
                    self.remembered_geometry.insert(w.app_id.clone(), (w.geometry.x, w.geometry.y, w.geometry.width, w.geometry.height));
                }
            }
        }
        self.resize = None;
    }

    /// The remembered position+size for `app_id`, if any - read by
    /// `add_window` when placing a fresh window, and by `crates/wayland/
    /// src/window_memory.rs` to decide what still needs persisting after a
    /// live update. See `remembered_geometry`'s own doc comment.
    pub fn remembered_geometry(&self, app_id: &str) -> Option<(i32, i32, u32, u32)> {
        self.remembered_geometry.get(app_id).copied()
    }

    /// Seeds (or overwrites) the remembered position+size for `app_id`
    /// directly, bypassing the normal "only an interactive drag/resize
    /// updates this" rule - the one legitimate reason to do that is
    /// `crates/wayland/src/window_memory.rs` restoring what was persisted
    /// from a *previous* session at startup, before any real drag/resize
    /// has happened this run.
    pub fn set_remembered_geometry(&mut self, app_id: String, geometry: (i32, i32, u32, u32)) {
        self.remembered_geometry.insert(app_id, geometry);
    }

    /// Every remembered `app_id` and its geometry - what `window_memory.rs`
    /// iterates to persist the full table (e.g. on a clean shutdown), not
    /// just whatever changed most recently.
    pub fn all_remembered_geometry(&self) -> impl Iterator<Item = (&str, (i32, i32, u32, u32))> {
        self.remembered_geometry.iter().map(|(k, &v)| (k.as_str(), v))
    }

    pub fn is_resizing(&self) -> bool {
        self.resize.is_some()
    }

    /// Which window is currently being interactively resized, if any - so
    /// a backend can skip an expensive-but-cosmetic per-window effect
    /// (content corner-masking, concretely - see its own call site's
    /// comment) for just that one window while its content is reflowing
    /// on every single frame, without touching every *other* window's own
    /// masking.
    pub fn resizing_window(&self) -> Option<WindowId> {
        self.resize.as_ref().map(|r| r.window)
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
