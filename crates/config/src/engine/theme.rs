use super::*;
use super::support::flatten_table_into;

impl Engine {
    // ---- srd.theme.* -------------------------------------------------------

    pub(super) fn fn_theme_set(&self, prefix: &str) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        let prefix = prefix.to_string();
        Ok(self.lua.create_function(move |_, table: Table| {
            let mut s = state.borrow_mut();
            flatten_table_into(&prefix, &table, &mut s.values)?;
            Ok(())
        })?)
    }
}
