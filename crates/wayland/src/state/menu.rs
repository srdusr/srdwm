use super::*;

impl CompState {

    /// Opens the right-click titlebar window menu for `window`, top-left
    /// corner at `pos` (global space, wherever the click landed). Rebuilds
    /// and caches the rasterised buffer once here rather than per frame --
    /// same reasoning as `redraw_decoration_buffer`.
    pub(crate) fn open_context_menu(&mut self, window: WindowId, pos: (i32, i32)) {
        let Some(menu) = ({
            let wm = self.wm.borrow();
            crate::context_menu::ContextMenu::open(&wm, window, pos)
        }) else {
            return;
        };
        let theme = self.wm.borrow().theme;
        let items: Vec<(&str, bool)> = menu.items.iter().map(|&(label, _)| (label, false)).collect();
        let data = decoration::render_context_menu(menu.width, menu.row_height, &items, theme.titlebar_bg, theme.titlebar_fg_focused, theme.titlebar_fg_unfocused, theme.default_border_color);
        let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (menu.width as i32, menu.height()), 1, Transform::Normal, None);
        self.context_menu_buffer = Some(buffer);
        self.context_menu = Some(menu);
    }

    pub(crate) fn close_context_menu(&mut self) {
        self.context_menu = None;
        self.context_menu_buffer = None;
    }

    /// Runs whichever action a click on `row` of the currently-open context
    /// menu selected. Takes the menu's `window`/action rather than reading
    /// `self.context_menu` itself so the caller can close the menu first
    /// (clearing the borrow) before this runs - several of these actions
    /// (`sync_geometry`, `redraw_decoration_buffer`) need `&mut self` in
    /// ways that would otherwise conflict with an active `self.context_menu`
    /// borrow.
    pub(crate) fn run_context_menu_action(&mut self, window: WindowId, action: crate::context_menu::MenuAction) {
        use crate::context_menu::MenuAction;
        match action {
            MenuAction::Minimize => {
                self.wm.borrow_mut().minimize_window(window);
                foreign_toplevel::send_state(self, window);
            }
            MenuAction::ToggleMaximize => {
                self.wm.borrow_mut().toggle_maximize(window);
                self.sync_geometry(window);
                foreign_toplevel::send_state(self, window);
            }
            MenuAction::ToggleAlwaysOnTop => {
                self.wm.borrow_mut().toggle_always_on_top(window);
            }
            MenuAction::Close => {
                if let Some(w) = self.id_to_window.get(&window) {
                    crate::input::close_dwindow(w);
                }
            }
        }
    }

    /// True when this titlebar press is the second of a double-click on the
    /// same window. Threshold is the usual 400ms.
    pub(crate) fn is_double_click(&mut self, id: WindowId, time: u32) -> bool {
        const DOUBLE_CLICK_MS: u32 = 400;
        let doubled = match self.last_titlebar_click {
            Some((last_id, last_time)) => last_id == id && time.saturating_sub(last_time) <= DOUBLE_CLICK_MS,
            None => false,
        };
        // Reset after a double, so a third click starts a fresh pair rather
        // than counting as another double.
        self.last_titlebar_click = if doubled { None } else { Some((id, time)) };
        doubled
    }
}
