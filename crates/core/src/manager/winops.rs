//! Per-window lifecycle: close, minimize, restore, scratchpad, maximize, fullscreen, floating, and plain move/resize.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
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

    /// Queues a real layout cycle - see `keyboard_layout_cycle_requests`'s
    /// own doc comment for why this is a count `main.rs` drains rather than
    /// something core does itself.
    pub fn request_keyboard_layout_cycle(&mut self) {
        self.keyboard_layout_cycle_requests += 1;
    }

    /// Drains the count queued by `request_keyboard_layout_cycle` since the
    /// last call. The caller (`main.rs`) is expected to call `Platform::
    /// cycle_keyboard_layout` this many times, then report the real result
    /// back via `set_keyboard_layout`.
    pub fn take_keyboard_layout_cycle_requests(&mut self) -> u32 {
        std::mem::take(&mut self.keyboard_layout_cycle_requests)
    }

    /// Sets `keyboard_layout` to whatever the platform actually reports --
    /// called once at startup and again after every real cycle, never
    /// guessed at from within core, which has no seat/keyboard of its own.
    pub fn set_keyboard_layout(&mut self, name: impl Into<String>) {
        self.keyboard_layout = name.into();
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
        // `maximize_geometry`, not `geometry` or `full_geometry`: maximize
        // covers the whole monitor past a dock's reserved zone, same as
        // `toggle_fullscreen` - but still stops at a top bar's, unlike
        // fullscreen. Previously targeted `full_geometry` outright (past
        // both), on the user's own request specifically about the dock;
        // that also silently pulled maximize past the top bar, which
        // wasn't part of that request and was reported back as its own
        // bug once live-tested. See `Monitor::maximize_geometry`'s own doc
        // comment for the exact rect this now is. The only remaining
        // difference from fullscreen is `decorated` (maximize keeps
        // whatever decoration state the window already had; fullscreen
        // forces it off).
        let monitor_geom = self.windows.get(&id).and_then(|w| self.monitor_for(w.monitor)).map(|m| m.maximize_geometry);
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

    /// Applies one of the Snap-Layouts flyout's fixed half/quarter
    /// positions directly (`crates/wayland/src/snap_flyout.rs`) - the
    /// click-driven equivalent of dragging the window to that same edge or
    /// corner and releasing near it, which is what `SmartPlacement::
    /// snap_zone` (used by `end_drag`) already computes from a live drag
    /// position instead of an explicit choice.
    ///
    /// Clears `maximized`/`fullscreen` first if either was set - opening
    /// the flyout from an already-maximized window (via its own maximize
    /// button) and picking a half is a real, expected use, and without this
    /// the window would keep reporting itself maximized while visually only
    /// occupying half the screen. `restore_geometry` is cleared alongside
    /// rather than left stale: it only means anything while `maximized` is
    /// still true, and the next real `toggle_maximize` sets it fresh anyway.
    pub fn apply_snap_zone(&mut self, id: WindowId, zone: SnapZoneKind) {
        let monitor_geom = self.windows.get(&id).and_then(|w| self.monitor_for(w.monitor)).map(|m| m.geometry);
        let animations_enabled = self.animations_enabled;
        let Some(area) = monitor_geom else { return };
        let target = zone.rect(area);
        let Some(w) = self.windows.get_mut(&id) else { return };
        let from = w.geometry;
        if w.maximized || w.fullscreen {
            w.maximized = false;
            w.fullscreen = false;
            w.restore_geometry = None;
        }
        w.geometry = target;
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
            w.geometry.width = width.max(w.min_size.0);
            w.geometry.height = height.max(w.min_size.1);
        }
    }

}
