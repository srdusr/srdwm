use super::*;

impl CompState {

    /// Layer surfaces need a configure sent in direct response to their
    /// first commit (sending it any earlier violates the protocol - see
    /// `smithay::desktop::LayerMap::arrange`'s doc comment on why `arrange`
    /// itself deliberately won't send one). Also the point at which an
    /// `Exclusive`-interactivity layer (e.g. a lock screen, or a launcher
    /// configured to grab all keyboard input) claims keyboard focus, since
    /// its `keyboard_interactivity` isn't reliably known until the client's
    /// state has actually committed.
    pub(crate) fn ensure_layer_initial_configure(&mut self, surface: &WlSurface) {
        // Called unconditionally from `commit()` for every surface in the
        // whole desktop, on every single commit - so before doing anything
        // that scales with output/layer count, a cheap O(1) check: has this
        // surface ever gone through `get_layer_surface` at all? Only that
        // request ever inserts `LayerSurfaceData` into a surface's
        // `data_map` (smithay's own `handlers.rs`), so this is `None` for
        // every ordinary xdg-toplevel/subsurface commit - the overwhelming
        // majority of commits on any real desktop. Skipping straight past
        // the per-output `layer_for_surface` surface-tree walk for all of
        // those is the difference between this function costing something
        // on every single frame any window renders versus only on commits
        // from the handful of surfaces that were ever layer surfaces.
        if with_states(surface, |states| states.data_map.get::<LayerSurfaceData>().is_none()) {
            return;
        }
        // A layer surface lives in exactly one output's `LayerMap` (whichever
        // one `new_layer_surface` mapped it into), so find that output rather
        // than assuming a single global one.
        let found = self.outputs().find_map(|output| {
            let layer = layer_map_for_output(output).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned();
            layer.map(|l| (output.clone(), l))
        });
        // Not a layer surface (or a destroyed one - see `new_surface`'s
        // pre-commit-hook workaround, which is what stops this from being
        // a protocol error). Every ordinary commit from every window in
        // the desktop passes through here and takes this branch, so this
        // used to log unconditionally during the P0 investigation
        // (docs/PANEL_SUPPORT_TODO.md) - diagnostic purpose long since
        // served, and left running it logged upwards of 15k lines in a few
        // minutes of normal use (every Firefox frame, every terminal
        // redraw, ...), which is real wasted I/O, not just noise.
        let Some((output, layer)) = found else {
            return;
        };

        // Recompute geometry from whatever the client just committed
        // (`set_size`/`set_anchor`/`set_margin`/`set_exclusive_zone` are all
        // double-buffered, applied on this commit) *before* looking at
        // `initial_configure_sent` - `map_layer`'s own `arrange()` call ran
        // before the client had sent any of that, so without this, the
        // first configure would carry stale, pre-request-processed geometry
        // (verified live: wofi's `set_size(420, 550)` was otherwise ignored
        // and it got stuck at the half-output fallback size instead). Every
        // later commit needs the same treatment for live resizes/anchor
        // changes; `arrange()` only actually sends a configure when
        // something changed, so this is a no-op on a commit that didn't
        // touch layer-shell state.
        let zone_before = layer_map_for_output(&output).non_exclusive_zone();
        layer_map_for_output(&output).arrange();
        let zone_after = layer_map_for_output(&output).non_exclusive_zone();
        if zone_after != zone_before {
            // A bar/dock claiming (or releasing) an exclusive zone changes
            // the area core's placement/tiling should actually use --
            // without this, `WindowManager`'s notion of the monitor rect
            // is whatever `Platform::monitors()` returned once at startup
            // (before any layer-shell client had connected and set a real
            // exclusive zone), so every window keeps being placed across
            // the *whole* output including the strip a bar now occupies:
            // new windows spawn with their titlebar directly under the bar,
            // rendered beneath it and unreachable to drag. Reusing
            // `MonitorAdded` here rather than a new event type: main.rs's
            // handler for it already re-queries the full monitor list from
            // the platform rather than trusting the event's payload (see
            // its own comment on why), which is exactly "go recompute the
            // usable area" - the placeholder `Monitor` below is discarded
            // unread on that path.
            self.pending.borrow_mut().push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
        }

        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|d| d.lock().unwrap().initial_configure_sent)
                .unwrap_or(false)
        });
        if !initial_configure_sent {
            layer.layer_surface().send_configure();
            log::debug!("layer-shell: sent initial configure for surface {:?}", surface.id());
        }

        // Checked on every commit, not just the first: a client can flip
        // `keyboard_interactivity` to `Exclusive` after already being
        // mapped (and this is also, in practice, where a freshly-mapped
        // `Exclusive` surface - e.g. wofi, which requests it from the very
        // first commit - actually gets focus, since `set_keyboard_focus`
        // is idempotent against a surface that's already focused).
        if layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive {
            self.set_keyboard_focus(Some(surface.clone()));
        }
    }
}
