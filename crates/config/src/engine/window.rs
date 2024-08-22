use super::*;
use super::support::{parse_direction, WindowAction};

impl Engine {
    // ---- srd.window.* ------------------------------------------------------

    pub(super) fn fn_window_focused(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, ()| {
            let wm = state.borrow().wm.clone();
            let wm = wm.borrow();
            let Some(w) = wm.focused_window() else { return Ok(Value::Nil) };
            let t = lua.create_table()?;
            t.set("id", w.id)?;
            t.set("title", w.title.clone())?;
            t.set("x", w.geometry.x)?;
            t.set("y", w.geometry.y)?;
            t.set("width", w.geometry.width)?;
            t.set("height", w.geometry.height)?;
            t.set("floating", w.floating)?;
            t.set("maximized", w.maximized)?;
            t.set("minimized", w.minimized)?;
            t.set("scratchpad", w.scratchpad)?;
            Ok(Value::Table(t))
        })?)
    }

    pub(super) fn fn_window_action(&self, action: WindowAction) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                match action {
                    WindowAction::Close => wm.close_window(id),
                    WindowAction::Minimize => wm.minimize_window(id),
                    WindowAction::Maximize => wm.toggle_maximize(id),
                    WindowAction::Fullscreen => wm.toggle_fullscreen(id),
                    WindowAction::ToggleFloating => wm.toggle_floating(id),
                    WindowAction::TogglePin => wm.toggle_always_on_top(id),
                    WindowAction::ScratchpadAdd => wm.scratchpad_add(id),
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_scratchpad_show(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            wm.borrow_mut().scratchpad_show();
            Ok(())
        })?)
    }

    /// `srd.window.move("left")` - swap the focused window with its
    /// neighbour in that direction (Hyprland's `movewindow l/r/u/d`).
    pub(super) fn fn_window_move_direction(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, direction: String| {
            let dir = parse_direction(&direction, "srd.window.move")?;
            let wm = state.borrow().wm.clone();
            wm.borrow_mut().move_window_direction(dir);
            Ok(())
        })?)
    }

    /// `srd.window.next()` / `srd.window.prev()` - cycle focus through the
    /// windows on the current workspace (Hyprland's `cyclenext`).
    pub(super) fn fn_window_cycle(&self, forward: bool) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if forward {
                wm.focus_next();
            } else {
                wm.focus_previous();
            }
            // Bring it to the top, matching the `bringactivetotop` the
            // Hyprland binding pairs with `cyclenext`.
            if let Some(id) = wm.focused_id() {
                wm.raise_window(id);
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_window_focus_direction(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, direction: String| {
            let dir = parse_direction(&direction, "srd.window.focus")?;
            let wm = state.borrow().wm.clone();
            wm.borrow_mut().focus_direction(dir);
            Ok(())
        })?)
    }

    pub(super) fn fn_window_set_decorations(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, enabled: bool| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.decorated = enabled;
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_window_set_border_color(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (r, g, b): (u8, u8, u8)| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.border_color = (r, g, b);
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_window_set_border_width(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, width: u32| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.border_width = width;
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_window_set_opacity(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, opacity: f32| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.opacity = opacity.clamp(0.0, 1.0);
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_window_set_resize_margin(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, margin: i32| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.resize_margin = Some(margin);
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_window_set_floating(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, floating: bool| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.floating = floating;
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_window_is_floating(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let wm = wm.borrow();
            Ok(wm.focused_id().map(|id| wm.is_floating(id)).unwrap_or(false))
        })?)
    }
}
