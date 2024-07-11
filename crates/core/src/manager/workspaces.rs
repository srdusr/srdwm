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
        let fallback = self.workspaces.iter().map(|w| w.id).find(|&w| w != id).unwrap_or(0);
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
        }
    }

    pub fn current_workspace(&self) -> WorkspaceId {
        self.current_workspace
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

    /// Windows that should currently be shown to the user: those on the
    /// active workspace of whichever monitor they're assigned to, and not minimized.
    pub fn visible_windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values().filter(|w| w.workspace == self.current_workspace && !w.minimized)
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
        self.order.iter().rev().filter_map(|id| self.windows.get(id)).filter(|w| w.workspace == self.current_workspace && !w.minimized)
    }

}
