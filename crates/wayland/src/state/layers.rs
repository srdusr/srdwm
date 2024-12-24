use smithay::backend::renderer::utils::{with_renderer_surface_state, RendererSurfaceState};

use super::*;

impl CompState {
    /// Keeps a layer surface's exclusive-zone reservation honest against
    /// whether it currently has anything to show - called from `commit()`
    /// before `ensure_layer_initial_configure`, for every surface in the
    /// desktop (same cheap non-layer-surface early-out reasoning as that
    /// function's own doc comment).
    ///
    /// The protocol's own text (`wlr-layer-shell-unstable-v1.xml`):
    /// "Attaching a null buffer to a layer surface unmaps it." Nothing in
    /// smithay's `LayerMap` acts on that by itself - `arrange()` walks
    /// every layer in `self.layers` unconditionally, using each one's
    /// last-requested `exclusive_zone` regardless of whether it currently
    /// has a buffer. `layer_destroyed` (protocols.rs) already handles the
    /// *destroyed* case with its own zone_before/`unmap_layer`/zone_after
    /// diff; this is the same fix for a client that hides by committing a
    /// null buffer while keeping the `zwlr_layer_surface_v1` object alive
    /// - cheaper than destroying and recreating it, and exactly what
    /// AGS's dock does to hide itself for a fullscreen window. Without
    /// this, the dock's last-requested exclusive zone stayed reserved the
    /// entire time it was hidden - a real, empty, unexplained band at the
    /// screen edge, reported live by an AGS peer session's own measurement
    /// across a genuine fullscreen toggle (the dock's zone correctly
    /// dropped to 0 on a real *maximize*, ruling that path out).
    ///
    /// `hidden_layer_surfaces` (see its own doc comment) is what makes the
    /// reverse direction work: `unmap_layer` removes the surface from
    /// `LayerMap`'s own list, so there is no way to find it again via
    /// `layer_for_surface` once that happens - this is the only record
    /// of "this surface is mine to re-map" for when a real buffer comes
    /// back.
    pub(crate) fn sync_layer_visibility(&mut self, surface: &WlSurface) {
        let Some(initial_configure_sent) = with_states(surface, |states| {
            states.data_map.get::<LayerSurfaceData>().map(|d| d.lock().unwrap().initial_configure_sent)
        }) else {
            return;
        };
        // A surface's very first commit legitimately has no buffer yet --
        // that's the protocol's own required handshake (commit once with
        // nothing attached so the compositor can send the *first*
        // `configure`, only after which the client is allowed to attach
        // real content at all), not a client "hiding" anything. Treating it
        // as a hide (the bug this early-return fixes) called `unmap_layer`
        // before `ensure_layer_initial_configure` ever ran, which made that
        // function's own `layer_for_surface` lookup find nothing and skip
        // sending the configure entirely - every layer-shell client
        // (a bar, a dock, a wallpaper daemon) left waiting forever for
        // permission to draw it was never going to get, each eventually
        // giving up and destroying/recreating its surface in a loop.
        // Confirmed live: zero "sent initial configure" log lines across an
        // entire session, and a repeating ~2-minute create/destroy cycle for
        // every `gtk4-layer-shell` surface. Only a surface that has already
        // completed its initial handshake can meaningfully "hide" by
        // committing a null buffer later - that's the real case this
        // function still needs to handle, below.
        if !initial_configure_sent {
            return;
        }
        let has_buffer = with_renderer_surface_state(surface, |state: &mut RendererSurfaceState| state.buffer().is_some()).unwrap_or(false);

        // TEMPORARY diagnostic for the "AGS dock reserves its exclusive
        // zone but never paints" investigation, relayed from the AGS peer
        // session - this function's own `has_buffer`-based hide/show logic
        // (see the doc comment above) is the leading suspect: if a client
        // ever legitimately commits a null buffer for a reason other than
        // an intentional hide (an internal `Gtk.Revealer` transition
        // artifact, a resize-in-progress commit), this treats it as a hide,
        // `unmap_layer`s it, and requires a *later* has-buffer commit to
        // ever come back - which would look exactly like this symptom if
        // the client's own state machine doesn't expect the compositor to
        // have done that and never re-triggers one. Logs every call for
        // every layer surface, not just suspected ones, since which
        // surface is actually affected isn't confirmed yet. Remove once
        // resolved.
        log::warn!("LAYER-VIS-DIAG surface={:?} has_buffer={has_buffer} already_hidden={}", surface.id(), self.hidden_layer_surfaces.contains_key(surface));

        if has_buffer {
            let Some((output, layer)) = self.hidden_layer_surfaces.remove(surface) else { return };
            let mut map = layer_map_for_output(&output);
            let zone_before = map.non_exclusive_zone();
            let _ = map.map_layer(&layer);
            let zone_after = map.non_exclusive_zone();
            log::warn!("LAYER-VIS-DIAG re-mapped namespace={:?} zone_before={zone_before:?} zone_after={zone_after:?}", layer.namespace());
            if zone_after != zone_before {
                self.pending.borrow_mut().push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
            }
            return;
        }

        if self.hidden_layer_surfaces.contains_key(surface) {
            return;
        }
        for output in self.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let Some(layer) = map.layers().find(|l| l.wl_surface() == surface).cloned() else { continue };
            let zone_before = map.non_exclusive_zone();
            map.unmap_layer(&layer);
            let zone_after = map.non_exclusive_zone();
            log::warn!("LAYER-VIS-DIAG unmapped namespace={:?} zone_before={zone_before:?} zone_after={zone_after:?}", layer.namespace());
            if zone_after != zone_before {
                self.pending.borrow_mut().push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
            }
            drop(map);
            self.hidden_layer_surfaces.insert(surface.clone(), (output, layer));
            break;
        }
    }

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
