use super::*;

impl CompState {

    /// Opens the right-click titlebar window menu for `window`, top-left
    /// corner at `pos` (global space, wherever the click landed). Rebuilds
    /// and caches the rasterised buffer once here rather than per frame --
    /// same reasoning as `redraw_decoration_buffer`.
    pub(crate) fn open_context_menu(&mut self, window: WindowId, pos: (i32, i32)) {
        let Some(mut menu) = ({
            let wm = self.wm.borrow();
            crate::context_menu::ContextMenu::open(&wm, window, pos)
        }) else {
            return;
        };
        // `ContextMenu::width` is a backend-agnostic placeholder - `core`
        // has no font of its own to measure real text against, so it can
        // only ever pick a fixed guess. Widened here to whatever this
        // menu's own widest real label actually needs, or reported live
        // as "text goes out of view": the old fixed width comfortably fit
        // every label back when this menu only listed short ones
        // ("Minimize", "Close"), but a longer one added since ("Button
        // Style: Traffic Lights", or a user-configurable workspace name)
        // just ran past the panel's own right edge, silently cut off
        // mid-character by `render_context_menu`'s own overflow guard.
        // Only ever grows the width, never shrinks it below the built-in
        // minimum `ContextMenu::open` already picked.
        let font = decoration::find_system_font();
        let widest_label = menu.items.iter().map(|&(label, _)| decoration::measure_text_width(&font, label, decoration::FONT_PIXELS)).fold(0.0_f32, f32::max);
        let content_width = (widest_label + decoration::TEXT_LEFT_PADDING * 2.0).ceil() as u32;
        menu.width = menu.width.max(content_width);
        let theme = self.wm.borrow().theme;
        let rows: Vec<(&str, bool, u32, decoration::MenuRowKind)> = menu
            .items
            .iter()
            .enumerate()
            .map(|(i, &(label, action))| {
                let kind = match action {
                    crate::context_menu::MenuAction::Separator => decoration::MenuRowKind::Separator,
                    crate::context_menu::MenuAction::Header => decoration::MenuRowKind::Header,
                    _ => decoration::MenuRowKind::Item,
                };
                (label, false, menu.row_height_for(i), kind)
            })
            .collect();
        let data = decoration::render_context_menu(menu.width, &rows, theme.titlebar_bg, theme.titlebar_fg_focused, theme.titlebar_fg_unfocused, theme.default_border_color);
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
            MenuAction::ToggleFullscreen => {
                self.wm.borrow_mut().toggle_fullscreen(window);
                self.sync_geometry(window);
                foreign_toplevel::send_state(self, window);
            }
            MenuAction::ToggleFloating => {
                self.wm.borrow_mut().toggle_floating(window);
                self.sync_geometry(window);
            }
            MenuAction::ToggleAlwaysOnTop => {
                self.wm.borrow_mut().toggle_always_on_top(window);
            }
            MenuAction::MoveToWorkspace(workspace) => {
                self.wm.borrow_mut().move_window_to_workspace(window, workspace);
            }
            MenuAction::CycleButtonStyle => {
                let mut wm = self.wm.borrow_mut();
                wm.theme.traffic_light_buttons = !wm.theme.traffic_light_buttons;
                drop(wm);
                self.redraw_every_decoration();
            }
            MenuAction::CycleButtonSide => {
                let mut wm = self.wm.borrow_mut();
                wm.theme.buttons_left = !wm.theme.buttons_left;
                drop(wm);
                self.redraw_every_decoration();
            }
            MenuAction::Close => {
                if let Some(w) = self.id_to_window.get(&window) {
                    crate::input::close_dwindow(w);
                }
            }
            // Never actually reached: the click-dispatch site
            // (`input/pointer.rs`) intercepts `Separator`/`Header` before
            // calling this function at all. Handled here too so this
            // match stays exhaustive without a catch-all that would
            // silently swallow a real future variant added without
            // updating this function.
            MenuAction::Separator | MenuAction::Header => {}
        }
    }

    /// Rebuilds every open window's titlebar/border bitmap against
    /// whatever `wm.theme` currently holds - what `CycleButtonStyle`/
    /// `CycleButtonSide` need to actually show their effect immediately.
    ///
    /// `srd set button_style`/`button_side` (`crates/platform/src/ipc/
    /// dispatch.rs`) change the exact same `ThemeConfig` fields but are
    /// deliberately scoped to "only affects windows created (or
    /// redecorated) after this call" - that crate is backend-agnostic
    /// and has no way to reach into a Wayland-specific redraw. A titlebar
    /// menu action has no such excuse: it's *this* backend's own code,
    /// already holding `&mut self`, and a "customize" action that doesn't
    /// visibly change the very titlebar you clicked would be exactly the
    /// kind of "doesn't make sense" this menu was reported for in the
    /// first place. `redraw_decoration_buffer` already no-ops on any
    /// window whose decoration signature didn't actually change, so
    /// calling it for every window here costs nothing for the (common)
    /// case where most of them don't use server-side decoration at all.
    fn redraw_every_decoration(&mut self) {
        let ids: Vec<WindowId> = self.wm.borrow().windows().map(|w| w.id).collect();
        for id in ids {
            self.redraw_decoration_buffer(id);
        }
    }

    /// Opens the Snap-Layouts flyout for `window`, top-left corner at `pos`
    /// (global space, by convention the maximize button's own titlebar
    /// position). Same build-once-on-open pattern as `open_context_menu`.
    pub(crate) fn open_snap_flyout(&mut self, window: WindowId, pos: (i32, i32)) {
        let flyout = crate::snap_flyout::SnapFlyout::open(window, pos);
        let theme = self.wm.borrow().theme;
        let labels: Vec<&str> = flyout.cells().iter().map(|z| z.label()).collect();
        let data = decoration::render_snap_flyout(3, flyout.cell_width, flyout.cell_height, &labels, theme.titlebar_bg, theme.titlebar_fg_focused, theme.default_border_color);
        let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (flyout.width() as i32, flyout.height() as i32), 1, Transform::Normal, None);
        self.snap_flyout_buffer = Some(buffer);
        self.snap_flyout = Some(flyout);
    }

    pub(crate) fn close_snap_flyout(&mut self) {
        self.snap_flyout = None;
        self.snap_flyout_buffer = None;
    }

    /// Applies `zone` to `window` - the flyout's own equivalent of
    /// `run_context_menu_action`, taking the target explicitly rather than
    /// reading `self.snap_flyout` for the same borrow-conflict reason
    /// documented on that function.
    pub(crate) fn run_snap_flyout_action(&mut self, window: WindowId, zone: SnapZoneKind) {
        self.wm.borrow_mut().apply_snap_zone(window, zone);
        self.sync_geometry(window);
        foreign_toplevel::send_state(self, window);
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
