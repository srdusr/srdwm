//! Workspace management: create/rename/remove/switch, and per-workspace window queries.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
    // ---- Workspaces -----------------------------------------------------

    pub fn add_workspace(&mut self, name: impl Into<String>, layout: impl Into<String>) -> WorkspaceId {
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        self.workspaces.push(Workspace::new(id, name, layout));
        id
    }

    /// Sets a workspace's display name - used to apply `workspace.names`
    /// at startup (`crates/srdwm/src/main.rs`'s `apply_workspace_count`),
    /// since `WindowManager::new`/`add_workspace` otherwise leave every
    /// workspace named after its own 1-based index regardless of what a
    /// config asked for. A no-op if `id` doesn't exist.
    pub fn rename_workspace(&mut self, id: WorkspaceId, name: impl Into<String>) {
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == id) {
            w.name = name.into();
        }
    }

    pub fn remove_workspace(&mut self, id: WorkspaceId) {
        if self.workspaces.len() <= 1 {
            return;
        }
        // `unwrap_or(1)` is unreachable in practice - the `len() <= 1`
        // guard above means `find` always has at least one other workspace
        // to return - but `1`, not `0`, since workspace ids are 1-based
        // (see `WindowManager::new`'s own doc comment) and `0` is no longer
        // a real workspace id this could plausibly fall back to.
        let fallback = self.workspaces.iter().map(|w| w.id).find(|&w| w != id).unwrap_or(1);
        for w in self.windows.values_mut().filter(|w| w.workspace == id) {
            w.workspace = fallback;
        }
        self.workspaces.retain(|w| w.id != id);
        if self.current_workspace == id {
            self.current_workspace = fallback;
        }
    }

    /// Switches to `id`, unless `auto_back_and_forth` is set and `id` is
    /// already the current workspace - in which case this jumps to
    /// `previous_workspace` instead, sway's `workspace_auto_back_and_forth`
    /// behavior. `previous_workspace` itself always tracks "whatever was
    /// current right before this call changed it", updated on every real
    /// switch regardless of the setting, so turning the setting on later
    /// (or a client-driven switch, e.g. `ext_workspace_v1`'s `activate`)
    /// doesn't need its own separate bookkeeping.
    pub fn switch_workspace(&mut self, id: WorkspaceId) {
        let target = if self.auto_back_and_forth && id == self.current_workspace { self.previous_workspace } else { id };
        if self.workspaces.iter().any(|w| w.id == target) && target != self.current_workspace {
            self.previous_workspace = self.current_workspace;
            self.current_workspace = target;
            // Keyboard focus otherwise stayed on whatever was focused
            // *before* the switch - this function only ever touched
            // `current_workspace`, never `self.focused` - so real input
            // kept going to a window that had just gone invisible while
            // whatever's now on screen, if anything, received nothing.
            // Reported live: switching to a workspace with an open window
            // left that window unfocused and the previous workspace's
            // window still receiving keystrokes. Only reassigns focus when
            // the currently-focused window isn't actually on the new
            // workspace - an already-correct focus (e.g. `focus_window`'s
            // own workspace-follow call into this function, where the
            // target window IS what should end up focused) must not get
            // silently overridden by "pick the topmost window instead".
            let focus_still_valid = self.focused.and_then(|id| self.windows.get(&id)).is_some_and(|w| w.workspace == target);
            if !focus_still_valid {
                self.focused = self.window_ids_on_workspace_front_to_back(target).into_iter().next();
            }
        }
    }

    pub fn current_workspace(&self) -> WorkspaceId {
        self.current_workspace
    }

    /// The workspace actually showing on `monitor` right now - `current_
    /// workspace` directly when `per_monitor_workspaces` is `false` (every
    /// monitor always agrees, by construction, since only `switch_
    /// workspace` - never `switch_workspace_on_monitor` - can run in that
    /// mode); otherwise this monitor's own independently-switched
    /// workspace, or `current_workspace` as the fallback for a monitor
    /// that has never had one switched independently yet (freshly
    /// connected, or the mode was just turned on).
    pub fn workspace_for_monitor(&self, monitor: MonitorId) -> WorkspaceId {
        if self.per_monitor_workspaces {
            self.monitor_workspaces.get(&monitor).copied().unwrap_or(self.current_workspace)
        } else {
            self.current_workspace
        }
    }

    /// Whether `id` is showing on *any* currently-connected monitor right
    /// now - what `srd workspaces`/AGS's own workspace pills should treat
    /// as "active" (`crates/platform/src/ipc.rs::workspace_snapshot`).
    /// Structurally allows more than one workspace to be active at once,
    /// which only actually happens in `per_monitor_workspaces` mode with
    /// two monitors on different workspaces - shared mode (the default)
    /// always has exactly one, same as before this existed.
    pub fn is_workspace_visible(&self, id: WorkspaceId) -> bool {
        if self.per_monitor_workspaces {
            self.monitors.iter().any(|m| self.workspace_for_monitor(m.id) == id)
        } else {
            id == self.current_workspace
        }
    }

    /// The `per_monitor_workspaces`-aware counterpart to `switch_
    /// workspace`: switches `monitor`'s own workspace to `id` without
    /// affecting any other monitor, when the mode is on. Falls straight
    /// through to the ordinary shared-mode `switch_workspace` (ignoring
    /// `monitor` entirely) when it's off, so a caller can always use this
    /// one entry point regardless of which mode is active rather than
    /// branching on the config flag itself - see its own call site in
    /// `crates/platform/src/ipc.rs`'s `activate_workspace` handler.
    ///
    /// `monitor` is "whichever monitor this switch should apply to", not
    /// necessarily where the pointer is - the caller decides that (the
    /// focused window's own monitor, in practice), same as real per-output
    /// keybinding routing in Hyprland/niri.
    pub fn switch_workspace_on_monitor(&mut self, id: WorkspaceId, monitor: MonitorId) {
        if !self.per_monitor_workspaces {
            self.switch_workspace(id);
            return;
        }
        let current = self.workspace_for_monitor(monitor);
        let target = if self.auto_back_and_forth && id == current {
            self.monitor_workspaces.get(&monitor).copied().unwrap_or(self.previous_workspace)
        } else {
            id
        };
        if !self.workspaces.iter().any(|w| w.id == target) || target == current {
            return;
        }
        self.previous_workspace = current;
        self.monitor_workspaces.insert(monitor, target);
        // Same reasoning as `switch_workspace`'s own matching comment:
        // reassign focus only when the currently-focused window isn't
        // already correctly on the new workspace, so an already-correct
        // focus assignment from elsewhere doesn't get silently overridden.
        let focus_still_valid = self.focused.and_then(|id| self.windows.get(&id)).is_some_and(|w| w.workspace == target);
        if !focus_still_valid {
            self.focused = self.window_ids_on_workspace_front_to_back(target).into_iter().next();
        }
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

    /// Windows that should currently be shown to the user: those on
    /// whichever workspace their own monitor is currently showing, and not
    /// minimized.
    ///
    /// In shared mode (`per_monitor_workspaces` off, the default) every
    /// monitor is always showing `current_workspace`, so this reduces to
    /// exactly the original single-shared-workspace filter and `w.monitor`
    /// plays no part in it - switching workspace still changes what's
    /// shown on every screen at once. In per-monitor mode, each window is
    /// checked against its *own* monitor's independently-switched
    /// workspace (`workspace_for_monitor`) instead, so two monitors on two
    /// different workspaces each correctly show only their own.
    pub fn visible_windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values().filter(|w| w.workspace == self.workspace_for_monitor(w.monitor) && !w.minimized)
    }

    /// Same windows as [`Self::visible_windows`], but in real front-to-back
    /// stacking order (topmost first) instead of arbitrary `HashMap`
    /// iteration order. Needed anywhere a backend composites more than one
    /// window's elements (content, decoration, border) together and their
    /// relative order across *different* windows actually matters - unlike
    /// `visible_windows`, which is fine for anything per-window in
    /// isolation (border color, geometry) where order never came up.
    /// `self.order` reversed is the same "topmost first" convention
    /// `hit_test`/`window_at` already use.
    pub fn visible_windows_front_to_back(&self) -> impl Iterator<Item = &Window> {
        self.order.iter().rev().filter_map(|id| self.windows.get(id)).filter(|w| w.workspace == self.workspace_for_monitor(w.monitor) && !w.minimized)
    }

}
