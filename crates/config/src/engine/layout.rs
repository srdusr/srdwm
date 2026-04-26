use super::*;
use super::support::flatten_table_into;

impl Engine {
    // ---- srd.layout.* ------------------------------------------------------

    pub(super) fn fn_layout_set(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, name: String| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            let ws = wm.current_workspace();
            wm.set_layout(ws, name);
            wm.arrange_workspace(ws);
            Ok(())
        })?)
    }

    /// `srd.layout.get()` - the name of the layout the *current workspace*
    /// is using right now.
    ///
    /// Deliberately not `srd.get("workspace.layout")`, which reads the
    /// config values table: that holds the configured default layout for
    /// new workspaces, not what the workspace in front of you switched to
    /// since. A toggle written against that key works exactly once and
    /// then sticks, because the value it reads never changes.
    pub(super) fn fn_layout_get(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, ()| {
            let wm = state.borrow().wm.clone();
            let wm = wm.borrow();
            let ws = wm.current_workspace();
            let name = wm.workspace(ws).map(|w| w.layout.clone()).unwrap_or_default();
            Ok(Value::String(lua.create_string(&name)?))
        })?)
    }

    pub(super) fn fn_layout_configure(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (name, table): (String, Table)| {
            let mut s = state.borrow_mut();
            flatten_table_into(&format!("layout.{name}"), &table, &mut s.values)?;
            let master_ratio = s.values.get(&format!("layout.{name}.master_ratio")).and_then(|v| v.as_f64());
            drop(s);
            if let Some(ratio) = master_ratio {
                state.borrow().wm.borrow_mut().tiling.master_ratio = ratio as f32;
            }
            Ok(())
        })?)
    }
}
