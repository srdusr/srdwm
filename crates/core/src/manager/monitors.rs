//! Monitor list, hotplug rehoming, and lookups.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
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
            // Maximize and fullscreen target different rects now - see
            // `Monitor::maximize_geometry`'s doc comment for why a maximized
            // window still stops at a top bar while fullscreen does not.
            let target = if window.maximized { monitor.maximize_geometry } else { monitor.full_geometry };
            if window.geometry != target {
                window.geometry = target;
            }
        }
        self.apply_monitor_layouts();
    }

    /// Applies `primary_layout`/`secondary_layout` to whichever workspace
    /// [`Self::workspace_for_monitor`] resolves for the primary monitor,
    /// and for any other monitor that has *already* been given its own
    /// distinct workspace via an independent switch - see those fields'
    /// own doc comments for why this is a no-op outside `per_monitor_
    /// workspaces` mode. Deliberately skips a non-primary monitor still
    /// showing the same fallback workspace as the primary (nothing
    /// distinct to apply `secondary_layout` to yet without also
    /// clobbering what `primary_layout` just set on that same shared
    /// workspace).
    ///
    /// Runs on every `set_monitors` call (startup and every hotplug
    /// alike) rather than on every workspace switch - applying it
    /// continuously would fight a workspace's own manually-set layout
    /// every time a monitor switched back to it.
    fn apply_monitor_layouts(&mut self) {
        if !self.per_monitor_workspaces || (self.primary_layout.is_empty() && self.secondary_layout.is_empty()) {
            return;
        }
        let Some(primary_id) = self.primary_monitor().map(|m| m.id) else { return };
        let primary_ws = self.workspace_for_monitor(primary_id);
        let registered: Vec<String> = self.available_layouts().iter().map(|s| s.to_string()).collect();
        if !self.primary_layout.is_empty() && registered.contains(&self.primary_layout) {
            self.set_layout(primary_ws, self.primary_layout.clone());
        }
        if self.secondary_layout.is_empty() || !registered.contains(&self.secondary_layout) {
            return;
        }
        let monitors = self.monitors.clone();
        for m in &monitors {
            if m.id == primary_id {
                continue;
            }
            let ws = self.workspace_for_monitor(m.id);
            if ws == primary_ws {
                continue;
            }
            self.set_layout(ws, self.secondary_layout.clone());
        }
    }

    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    /// Queues a request to move output `id` to `(x, y)` in the shared
    /// global space - the primitive monitor mirroring (and any other
    /// output-arrangement UI) needs: position two outputs at the same
    /// coordinates and they show the same desktop region, no separate
    /// "mirror" concept required anywhere in this compositor. Core cannot
    /// apply this itself (it doesn't own real output hardware - see this
    /// field's own doc comment on `WindowManager`); the backend drains and
    /// applies it on its own next poll via `drain_output_position_requests`.
    ///
    /// Replaces (not accumulates) any still-pending request for the same
    /// `id`: only the *latest* requested position for a given output
    /// matters if several arrive before the backend's next drain, the same
    /// "last write wins" semantics `srd set`'s other live-config values
    /// already have.
    pub fn request_output_position(&mut self, id: MonitorId, x: i32, y: i32) {
        self.output_position_requests.retain(|(existing, _, _)| *existing != id);
        self.output_position_requests.push((id, x, y));
    }

    /// Takes every currently-queued output-position request, leaving the
    /// queue empty. The backend calls this once per poll pass; requests
    /// that arrive between two polls are still captured (nothing is lost
    /// between drains, unlike a single `Option`), just coalesced to one
    /// per output id per drain by `request_output_position`'s own
    /// replace-not-accumulate behaviour.
    pub fn drain_output_position_requests(&mut self) -> Vec<(MonitorId, i32, i32)> {
        std::mem::take(&mut self.output_position_requests)
    }

    /// Queues a request to enable or disable the output named `name` --
    /// "primary only"/a per-display toggle, the two AGS monitor-layout
    /// panel rows gated pending this. Same "core has no real output
    /// handle, the backend drains and applies on its own next poll" shape
    /// as `request_output_position` above, and the same reasoning:
    /// turning a real CRTC's power state on or off is backend/hardware
    /// work, not something this crate can do itself.
    ///
    /// By *name*, not `MonitorId` like `request_output_position` - a
    /// disabled output is administratively removed from `monitors()`
    /// entirely (the same real unplug/replug code path a genuine hotplug
    /// already goes through, see the udev platform's own drain site), so
    /// its id - an index into whatever's currently connected - stops
    /// meaning anything the moment it's disabled. The connector's own
    /// name survives the round trip; nothing else does.
    pub fn request_output_enabled(&mut self, name: String, enabled: bool) {
        self.output_enable_requests.retain(|(existing, _)| *existing != name);
        self.output_enable_requests.push((name, enabled));
    }

    /// [`Self::drain_output_position_requests`]'s counterpart for
    /// enable/disable requests.
    pub fn drain_output_enable_requests(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.output_enable_requests)
    }

    /// Reports (or updates) `name`'s last-known state as an
    /// administratively-disabled-but-still-connected output - called by
    /// the backend at the moment it disables a connector, purely so `srd
    /// monitors`/the `monitors` subscribe event can keep listing it (as
    /// requested directly by the AGS peer session: a control that removes
    /// its own target from view the moment it's used is one-way, not a
    /// toggle). Deliberately separate from `monitors`/`set_monitors` --
    /// see `DisabledMonitor`'s own doc comment for why this must never
    /// touch real placement.
    pub fn set_disabled_monitor(&mut self, name: String, geometry: Rect, full_geometry: Rect, primary: bool) {
        self.disabled_monitors.insert(name, DisabledMonitor { geometry, full_geometry, primary });
    }

    /// Clears `name`'s disabled-monitor record - called by the backend
    /// once it re-enables the connector (it's live again, `monitors()`
    /// itself will report it) or discovers it's been genuinely unplugged
    /// while disabled (nothing left to offer re-enabling at all; see
    /// `reprobe_outputs`'s own doc comment on why "off" and "not
    /// connected" have to be reported differently).
    pub fn clear_disabled_monitor(&mut self, name: &str) {
        self.disabled_monitors.remove(name);
    }

    /// Every currently-known disabled-but-connected output, by name - see
    /// `set_disabled_monitor`'s own doc comment.
    pub fn disabled_monitors(&self) -> impl Iterator<Item = (&str, &DisabledMonitor)> {
        self.disabled_monitors.iter().map(|(name, m)| (name.as_str(), m))
    }

    /// `srd.monitor.split(name, parts, direction)` - divides connector
    /// `name`'s real output into `parts` equal logical monitors from the
    /// next time a backend queries `monitors()`. `parts <= 1` clears any
    /// existing split for `name` rather than storing a meaningless
    /// one-part split.
    pub fn set_monitor_split(&mut self, name: String, parts: u32, rows: bool) {
        if parts <= 1 {
            self.monitor_splits.remove(&name);
        } else {
            self.monitor_splits.insert(name, MonitorSplit { parts, rows });
        }
    }

    /// `name`'s current split request, if any - read by a backend's own
    /// `monitors()` query.
    pub fn monitor_split(&self, name: &str) -> Option<MonitorSplit> {
        self.monitor_splits.get(name).copied()
    }

    /// Queues a live `srd dispatch set output split` request - see
    /// `monitor_split_requests`' own doc comment for why this can't just
    /// call `set_monitor_split` directly from the IPC dispatch handler.
    /// Same "replace, don't accumulate" per-name semantics as `request_
    /// output_position`.
    pub fn request_monitor_split(&mut self, name: String, parts: u32, rows: bool) {
        self.monitor_split_requests.retain(|(existing, _, _)| *existing != name);
        self.monitor_split_requests.push((name, parts, rows));
    }

    /// [`Self::drain_output_position_requests`]'s counterpart for split
    /// requests - the backend applies each via `set_monitor_split` and
    /// pushes its own "just go recompute" event afterward, same as that
    /// function's own drain site.
    pub fn drain_monitor_split_requests(&mut self) -> Vec<(String, u32, bool)> {
        std::mem::take(&mut self.monitor_split_requests)
    }

    /// `srd.monitor.scale(name, factor)` - a backend applies this the
    /// next time it brings connector `name`'s head up (startup, hotplug,
    /// or re-enable). `factor <= 0.0` clears any existing override rather
    /// than storing a meaningless non-positive scale.
    pub fn set_monitor_scale(&mut self, name: String, factor: f64) {
        if factor > 0.0 {
            self.monitor_scales.insert(name, factor);
        } else {
            self.monitor_scales.remove(&name);
        }
    }

    /// `name`'s current scale override, if any - read by a backend when
    /// bringing that connector's head up.
    pub fn monitor_scale(&self, name: &str) -> Option<f64> {
        self.monitor_scales.get(name).copied()
    }

    pub fn primary_monitor(&self) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.primary).or_else(|| self.monitors.first())
    }

    pub(super) fn monitor_for(&self, id: MonitorId) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.id == id).or_else(|| self.primary_monitor())
    }

    /// Records which monitor the pointer is over right now - see `pointer_
    /// monitor`'s own doc comment for why core needs to be told this rather
    /// than knowing it already, and `add_window`'s target-monitor fallback
    /// chain for the one thing it's actually used for. Called from a real
    /// backend's pointer-motion handler; never `srd`/IPC-driven (nothing
    /// external has a legitimate reason to claim where the pointer is).
    pub fn set_pointer_monitor(&mut self, id: Option<MonitorId>) {
        self.pointer_monitor = id;
    }

    /// The bounding rect of every registered monitor's own `full_geometry`
    /// combined - the whole multi-monitor desktop's real screen area, not
    /// just one output's. `None` only when there are no monitors at all
    /// (never true in practice once startup has run).
    ///
    /// Exists specifically so `update_drag` can clamp a dragged window to
    /// "somewhere on some real screen" instead of "within the one monitor
    /// it happened to start the drag on" - the latter (what this
    /// replaced) made it *mathematically impossible* to drag a window from
    /// one monitor to another at all: the clamp bounds were computed once,
    /// from `w.monitor` at drag-start, and never updated as the drag
    /// crossed into a different monitor's own screen space, so `new_geom.x`
    /// could never exceed the starting monitor's own right edge no matter
    /// how far or fast the pointer moved. Reported live: a second monitor
    /// connected and fully working at the compositor/DRM level (`srd
    /// monitors` listed it, hotplug brought it up) still couldn't receive
    /// a dragged window at all.
    pub(super) fn all_monitors_bounds(&self) -> Option<Rect> {
        self.monitors.iter().map(|m| m.full_geometry).reduce(|a, b| {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            let right = a.right().max(b.right());
            let bottom = a.bottom().max(b.bottom());
            Rect::new(x, y, (right - x) as u32, (bottom - y) as u32)
        })
    }

}
