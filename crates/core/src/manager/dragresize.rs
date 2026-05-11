//! Interactive drag and resize session state.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
    // ---- Drag / resize ------------------------------------------------------

    pub fn start_drag(&mut self, id: WindowId, x: i32, y: i32) {
        if let Some(w) = self.windows.get(&id) {
            self.drag = Some(DragState { window: id, start_x: x, start_y: y, orig: w.geometry, last_x: x, last_y: y });
            self.focus_window(id);
        }
    }

    pub fn update_drag(&mut self, x: i32, y: i32) {
        if let Some(drag) = &mut self.drag {
            drag.last_x = x;
            drag.last_y = y;
        }
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

    /// Where the currently-dragged window would land if the button came up
    /// right now, or `None` when the drag is not in a snap zone.
    ///
    /// Deliberately calls the very same `SmartPlacement::snap_zone` that
    /// `end_drag` does, on the same inputs, rather than re-deriving the
    /// zones: a preview that can disagree with what release actually does
    /// is worse than no preview, and any future change to the zone
    /// geometry updates both at once by construction.
    ///
    /// Reported twice as missing: "if you move the window to absolute
    /// north it show you layout options" and "why do i still not see the
    /// windows layout or windows change layout when moved to areas of
    /// screen like in windows". Edge snapping itself already worked - it
    /// just committed silently on release with nothing shown beforehand,
    /// so there was no way to tell it was going to happen, or where.
    pub fn drag_snap_preview(&self) -> Option<Rect> {
        let drag = self.drag.as_ref()?;
        let w = self.windows.get(&drag.window)?;
        let m = self.monitor_for(w.monitor)?;
        SmartPlacement::snap_zone(w.geometry, m, &self.placement)
    }

    /// The monitor whose top edge the drag pointer is currently within
    /// [`SNAP_FLYOUT_EDGE`] of, or `None`.
    ///
    /// Measured against `full_geometry`, not `geometry`: the trigger band
    /// is the physical top of the screen, which is exactly where a bar
    /// usually sits. Using the exclusive-zone-shrunk rect would put the
    /// band *below* the bar, so on a machine with a top bar the gesture
    /// would only fire after the pointer had already travelled past it.
    ///
    /// The pointer, not the window's own top edge, because the two differ
    /// by however far down the titlebar the drag grabbed - and it is the
    /// pointer the user is actually aiming.
    ///
    /// Not `Rect::contains_point`: that would also reject a pointer *above*
    /// the monitor's top edge, which is the one direction this gesture is
    /// aimed in. A real seat clamps the cursor to the output, so `y < 0`
    /// should not arise on hardware - but `update_drag` clamps only the
    /// window, so nothing in this type's own API guarantees it, and
    /// "thrown past the edge" is the strongest possible form of the intent
    /// this is trying to detect. Horizontal containment is still required,
    /// as is being above the monitor's bottom, so a pointer on a different
    /// output never matches.
    pub fn drag_top_edge_monitor(&self) -> Option<&Monitor> {
        let drag = self.drag.as_ref()?;
        let (x, y) = (drag.last_x, drag.last_y);
        self.monitors.iter().find(|m| {
            let g = m.full_geometry;
            x >= g.x && x < g.right() && y < g.bottom() && y - g.y <= SNAP_FLYOUT_EDGE
        })
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The window the current drag is moving, if any.
    pub fn dragged_window(&self) -> Option<WindowId> {
        self.drag.as_ref().map(|d| d.window)
    }

    pub fn start_resize(&mut self, id: WindowId, edge: ResizeEdge, x: i32, y: i32) {
        if let Some(w) = self.windows.get(&id) {
            // Decided *before* `focus_window` below re-stacks `id` --
            // see `tiling_ratio_drag`'s own doc comment for why that
            // order is load-bearing, not stylistic.
            let ratio_drag_ids = self.tiling_ratio_drag(id, edge);
            let orig = w.geometry;
            self.resize = Some(ResizeState { window: id, edge, start_x: x, start_y: y, orig, orig_master_ratio: self.tiling.master_ratio, ratio_drag_ids });
            self.focus_window(id);
        }
    }

    pub fn update_resize(&mut self, x: i32, y: i32) {
        // Copied/cloned out rather than kept as a live `&self.resize`
        // borrow - the tiling branch below needs `&mut self`, which
        // can't coexist with a borrow of the field it's reading.
        let Some((window, edge, start_x, start_y, orig, orig_master_ratio, ratio_drag_ids)) =
            self.resize.as_ref().map(|r| (r.window, r.edge, r.start_x, r.start_y, r.orig, r.orig_master_ratio, r.ratio_drag_ids.clone()))
        else {
            return;
        };
        let (dx, dy) = (x - start_x, y - start_y);
        // Tiling's master/stack boundary is a live *ratio* the whole
        // column split is computed from, not one window's own rect - see
        // `adjust_master_ratio_for_drag`'s own doc comment for why a plain
        // geometry write here would just be silently discarded by the very
        // next `arrange_workspace` call anyway (reported live as "tiling
        // needs a lot of work": dragging a tiled window's border looked
        // like it resized, then snapped back the moment anything else
        // triggered a re-arrange). `ratio_drag_ids` was decided once, at
        // `start_resize` time, against the pre-focus membership - see
        // `tiling_ratio_drag`'s own doc comment for why that snapshot
        // (not a live re-derivation) is what has to be used here.
        if let Some(ids) = ratio_drag_ids {
            self.adjust_master_ratio_for_drag(window, &ids, dx, orig_master_ratio);
            return;
        }
        // This window's own minimum, not the one global floor - see
        // `Window::min_size`.
        let (min_w, min_h) = self.windows.get(&window).map(|w| w.min_size).unwrap_or((MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
        let mut new_geom = edge.apply_delta(orig, dx, dy, min_w, min_h);
        // `Window::aspect_ratio`'s own doc comment: a locked-ratio window
        // (the "phone monitor" case, concretely) re-derives one dimension
        // from the other here, on top of the ordinary delta above, rather
        // than needing a second, separate resize code path.
        if let Some(ratio) = self.windows.get(&window).and_then(|w| w.aspect_ratio) {
            new_geom = edge.apply_aspect_ratio(new_geom, ratio, min_w, min_h);
        }
        // Same live `w.monitor` correction as `update_drag`'s own doc
        // comment explains - a resize can cross a monitor boundary at
        // the edge being dragged just as easily as a drag can carry the
        // whole window across one, and `sync_geometry`'s per-tick scale
        // lookup doesn't care which kind of geometry change put the
        // window there.
        let now_on = self.monitors.iter().find(|m| m.geometry.overlaps(&new_geom)).map(|m| m.id);
        if let Some(w) = self.windows.get_mut(&window) {
            w.geometry = new_geom;
            if let Some(now_on_id) = now_on {
                w.monitor = now_on_id;
            }
        }
    }

    /// Whether resizing `id` along `edge` should live-adjust `self.tiling.
    /// master_ratio` instead of writing raw window geometry: `id` must be
    /// a non-floating, non-fullscreen member of a `"tiling"`-layout
    /// workspace's own master/stack arrangement, there must actually be a
    /// stack column to trade width with (a lone master-only window has
    /// nothing on the other side of the drag), and `edge` must be the
    /// shared boundary line between the two columns - the master
    /// column's own right edge, or any stack column window's own left
    /// edge, since both name the same physical boundary approached from
    /// either side. Anything else (a vertical edge, a floating window, a
    /// window under `dynamic`) falls through to the ordinary geometry
    /// resize unchanged.
    ///
    /// **Must be called before `focus_window` runs for this same
    /// interaction** - `start_resize`'s own call site is the only correct
    /// place, and the returned membership snapshot is what `start_resize`
    /// caches into `ResizeState` for `adjust_master_ratio_for_drag` to
    /// apply the layout against later, rather than that method re-deriving
    /// membership itself from `self.order` at *its* own, later point in
    /// time. `focus_window` raises its target to the *end* of `self.order`
    /// (`raise_window`), the exact list this membership is read from - so
    /// merely grabbing a master window to resize it re-stacks it into what
    /// looks like the stack's own last slot an instant later, and anything
    /// that re-derives membership after that point (including a first
    /// version of this whole feature that called `WindowManager::
    /// arrange_workspace` from inside the drag, which reads `self.order`
    /// itself fresh every time) silently applies the resulting ratio
    /// change to the *wrong* column: the window that's actually being
    /// dragged shrinks while its neighbour grows, backwards from what the
    /// mouse is doing. Caught by this method's own test coverage's fuller
    /// assertion (checking the *other* window's width too, not just the
    /// grabbed one), not by inspection.
    fn tiling_ratio_drag(&self, id: WindowId, edge: ResizeEdge) -> Option<Vec<WindowId>> {
        if !(edge.has_left() || edge.has_right()) {
            return None;
        }
        let w = self.windows.get(&id)?;
        if w.floating || w.fullscreen {
            return None;
        }
        if self.workspace(w.workspace).map(|ws| ws.layout.as_str()) != Some("tiling") {
            return None;
        }
        // Mirrors `arrange_workspace`'s own grouping exactly - the same
        // window set, same order, is what decides which windows are
        // "master" vs "stack" there, so this has to agree with it or the
        // ratio drag would trigger (or fail to) inconsistently with what
        // is actually on screen.
        let ids: Vec<WindowId> = self
            .order
            .iter()
            .copied()
            .filter(|&oid| self.windows.get(&oid).is_some_and(|ow| ow.workspace == w.workspace && ow.monitor == w.monitor && !ow.minimized && !ow.floating && !ow.fullscreen))
            .collect();
        let pos = ids.iter().position(|&oid| oid == id)?;
        let master_count = self.tiling.master_count.max(1).min(ids.len());
        if ids.len() <= master_count {
            return None;
        }
        ((pos < master_count && edge.has_right()) || (pos >= master_count && edge.has_left())).then_some(ids)
    }

    /// Applies a tiling ratio-drag's raw pixel delta `dx` (positive =
    /// dragged right = master column grows) against `orig_ratio` --
    /// `ResizeState::orig_master_ratio`, the ratio as it was when this
    /// resize *started*, not `self.tiling.master_ratio`'s own live value --
    /// the same "cumulative delta against a fixed starting snapshot"
    /// shape `update_drag`/`update_resize`'s own geometry math already
    /// uses for `orig`. Using the live value instead would compound: every
    /// tick would add the *whole* cumulative `dx` on top of whatever the
    /// previous tick already added, not just that tick's own incremental
    /// motion.
    ///
    /// Re-arranges every window in `ids` immediately against the new
    /// ratio, not just the grabbed one - the entire point of this being a
    /// *ratio* rather than one window's own rect is that every master and
    /// every stack window visibly resizes together, the same live
    /// feedback dwm/i3/Hyprland all give while dragging this exact
    /// boundary.
    ///
    /// Applies `MasterStackLayout` directly against `ids` - the frozen
    /// pre-focus snapshot `tiling_ratio_drag` returned - rather than
    /// calling `arrange_workspace`, which re-derives its own window list
    /// from `self.order` fresh every time it runs. By the time this method
    /// runs, `start_resize`'s own `focus_window` call has already raised
    /// `id` to the end of `self.order`; re-deriving membership from that
    /// live order here would silently apply the ratio change to
    /// whichever window *now* occupies the position `id` used to be in,
    /// not to `id` and its real neighbours - the exact bug this
    /// snapshot-based approach exists to avoid (see `tiling_ratio_drag`'s
    /// own doc comment for the full story, including how a first,
    /// `arrange_workspace`-based version of this method got caught by
    /// this file's own tests).
    fn adjust_master_ratio_for_drag(&mut self, id: WindowId, ids: &[WindowId], dx: i32, orig_ratio: f32) {
        let Some(w) = self.windows.get(&id) else { return };
        let Some(monitor) = self.monitor_for(w.monitor).cloned() else { return };
        let area_width = monitor.geometry.inset(self.tiling.gap_outer).width.max(1);
        let delta_ratio = dx as f32 / area_width as f32;
        // Clamped well short of 0.0/1.0 - either extreme would hand one
        // column all (or none) of the width, which `MasterStackLayout`
        // itself never guards against (a `0`-width stack column is a
        // degenerate, not-actually-tiled state, not a valid extreme of
        // the slider).
        self.tiling.master_ratio = (orig_ratio + delta_ratio).clamp(0.1, 0.9);
        for (placed_id, rect) in MasterStackLayout.arrange(ids, &monitor, &self.tiling) {
            if let Some(w) = self.windows.get_mut(&placed_id) {
                w.geometry = rect;
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
