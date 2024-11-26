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

    pub fn primary_monitor(&self) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.primary).or_else(|| self.monitors.first())
    }

    pub(super) fn monitor_for(&self, id: MonitorId) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.id == id).or_else(|| self.primary_monitor())
    }

}
