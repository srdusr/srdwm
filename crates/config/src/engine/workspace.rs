use super::*;

impl Engine {
    // ---- srd.workspace.* ---------------------------------------------------

    pub(super) fn fn_workspace_cycle(&self, forward: bool) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            let ids: Vec<_> = wm.workspaces().iter().map(|w| w.id).collect();
            if ids.is_empty() {
                return Ok(());
            }
            let cur = wm.current_workspace();
            let pos = ids.iter().position(|&id| id == cur).unwrap_or(0);
            let next = if forward { (pos + 1) % ids.len() } else { (pos + ids.len() - 1) % ids.len() };
            wm.switch_workspace(ids[next]);
            Ok(())
        })?)
    }

    pub(super) fn fn_workspace_switch(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, id: usize| {
            state.borrow().wm.borrow_mut().switch_workspace(id);
            Ok(())
        })?)
    }

    pub(super) fn fn_workspace_move_window(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, id: usize| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(focused) = wm.focused_id() {
                wm.move_window_to_workspace(focused, id);
            }
            Ok(())
        })?)
    }
}
