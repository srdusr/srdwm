use super::*;
use super::support::WindowAction;

impl Engine {
    pub(super) fn register_srd_module(&self) -> Result<()> {
        let lua = &self.lua;
        let srd = lua.create_table()?;

        srd.set("set", self.fn_set()?)?;
        srd.set("get", self.fn_get()?)?;
        srd.set("reset", self.fn_reset()?)?;
        srd.set("reset_all", self.fn_reset_all()?)?;
        srd.set("reset_category", self.fn_reset_category()?)?;
        srd.set("bind", self.fn_bind()?)?;
        srd.set("bind_repeat", self.fn_bind_repeat()?)?;
        srd.set("on", self.fn_on()?)?;
        srd.set("rule", self.fn_rule()?)?;
        srd.set("load", self.fn_load()?)?;
        srd.set("spawn", self.fn_spawn()?)?;
        srd.set("setenv", self.fn_setenv()?)?;
        srd.set("notify", self.fn_notify()?)?;
        srd.set("quit", self.fn_quit()?)?;
        srd.set("reload", self.fn_reload()?)?;
        srd.set("validate_config", self.fn_validate_config()?)?;

        let debug = lua.create_table()?;
        debug.set("config_status", self.fn_debug_config_status()?)?;
        debug.set("validate_config", self.fn_validate_config()?)?;
        debug.set("show_settings", self.fn_debug_show_settings()?)?;
        debug.set("profile_start", self.fn_debug_profile_start()?)?;
        debug.set("profile_stop", self.fn_debug_profile_stop()?)?;
        srd.set("debug", debug)?;

        let window = lua.create_table()?;
        window.set("focused", self.fn_window_focused()?)?;
        window.set("close", self.fn_window_action(WindowAction::Close)?)?;
        window.set("minimize", self.fn_window_action(WindowAction::Minimize)?)?;
        window.set("maximize", self.fn_window_action(WindowAction::Maximize)?)?;
        window.set("fullscreen", self.fn_window_action(WindowAction::Fullscreen)?)?;
        window.set("toggle_pin", self.fn_window_action(WindowAction::TogglePin)?)?;
        window.set("focus", self.fn_window_focus_direction()?)?;
        window.set("move", self.fn_window_move_direction()?)?;
        window.set("next", self.fn_window_cycle(true)?)?;
        window.set("prev", self.fn_window_cycle(false)?)?;
        window.set("set_decorations", self.fn_window_set_decorations()?)?;
        window.set("set_border_color", self.fn_window_set_border_color()?)?;
        window.set("set_border_width", self.fn_window_set_border_width()?)?;
        window.set("set_corner_radius", self.fn_window_set_corner_radius()?)?;
        window.set("set_opacity", self.fn_window_set_opacity()?)?;
        window.set("set_resize_margin", self.fn_window_set_resize_margin()?)?;
        window.set("set_floating", self.fn_window_set_floating()?)?;
        window.set("toggle_floating", self.fn_window_action(WindowAction::ToggleFloating)?)?;
        window.set("is_floating", self.fn_window_is_floating()?)?;
        // `srd.window.scratchpad()` moves the *focused* window into the
        // scratchpad pool, hiding it (sway's `move scratchpad`).
        // `srd.window.scratchpad_show()` toggles pool visibility and takes
        // no target of its own - deliberately not routed through
        // `fn_window_action`'s focused-window gate, since showing a hidden
        // scratchpad window has to work even when nothing is currently
        // focused (an empty workspace, or focus on a different monitor).
        // See `WindowManager::scratchpad_show`'s doc comment.
        window.set("scratchpad", self.fn_window_action(WindowAction::ScratchpadAdd)?)?;
        window.set("scratchpad_show", self.fn_scratchpad_show()?)?;
        srd.set("window", window)?;

        let layout = lua.create_table()?;
        layout.set("set", self.fn_layout_set()?)?;
        layout.set("configure", self.fn_layout_configure()?)?;
        srd.set("layout", layout)?;

        let workspace = lua.create_table()?;
        workspace.set("next", self.fn_workspace_cycle(true)?)?;
        workspace.set("prev", self.fn_workspace_cycle(false)?)?;
        workspace.set("switch", self.fn_workspace_switch()?)?;
        workspace.set("move_window", self.fn_workspace_move_window()?)?;
        srd.set("workspace", workspace)?;

        let monitor = lua.create_table()?;
        monitor.set("split", self.fn_monitor_split()?)?;
        monitor.set("scale", self.fn_monitor_scale()?)?;
        srd.set("monitor", monitor)?;

        let theme = lua.create_table()?;
        theme.set("set_colors", self.fn_theme_set("theme.colors")?)?;
        theme.set("set_decorations", self.fn_theme_set("theme.decorations")?)?;
        srd.set("theme", theme)?;

        // Expose `srd` both as a global and as a `require("srd")`-able
        // module: `require` resolves through `package.preload`/`package.path`,
        // never through globals, so config files that (reasonably) write
        // `local srd = require("srd")` would otherwise get a "module not
        // found" error despite `srd` existing as a global.
        let package: Table = lua.globals().get("package")?;
        let preload: Table = package.get("preload")?;
        preload.set(
            "srd",
            lua.create_function(|lua, ()| {
                let srd: Table = lua.globals().get("srd")?;
                Ok(srd)
            })?,
        )?;
        lua.globals().set("srd", srd)?;
        Ok(())
    }
}
