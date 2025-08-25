//! Right-click titlebar window menu - the one titlebar interaction
//! virtually every desktop WM has always offered. Backend-agnostic: this
//! is pure state and geometry (which rows exist, which one a point falls
//! on), not pixels. The Wayland backend renders it via
//! `decoration::render_context_menu`; the X11 backend draws it with raw
//! XCB calls the same way it already draws its own titlebar.
//!
//! Originally lived only in the Wayland crate; moved here once the X11
//! backend needed the same row set and hit-testing rather than a second,
//! drifting copy of the same logic - this data has no Wayland-specific
//! content at all, so duplicating it would just be two copies of the same
//! bug waiting to happen.

use crate::{WindowManager, WindowId, WorkspaceId, TITLEBAR_HEIGHT};

#[derive(Clone, Copy)]
pub enum MenuAction {
    Minimize,
    ToggleMaximize,
    ToggleFullscreen,
    ToggleFloating,
    ToggleAlwaysOnTop,
    MoveToWorkspace(WorkspaceId),
    Close,
    /// Not a real action - a purely visual divider row. `row_at` still
    /// resolves a click on one to `Some(index)` (it occupies a real row,
    /// same as any other), so the dispatch site is what actually no-ops
    /// on it, same "the row exists but does nothing" contract a real
    /// desktop's own menu separators have.
    Separator,
}

pub struct ContextMenu {
    pub window: WindowId,
    /// Top-left corner, in global (output-independent) space - same frame
    /// `Window.geometry` and every other rendered element's position uses.
    pub pos: (i32, i32),
    pub width: u32,
    pub row_height: u32,
    pub items: Vec<(&'static str, MenuAction)>,
}

const MENU_WIDTH: u32 = 170;

impl ContextMenu {
    /// Builds the menu for `window`, opening with its top-left corner at
    /// `pos` (wherever the right-click landed). Labels reflect the
    /// window's *current* state - "Maximize" flips to "Restore", "Always
    /// on Top" gets a checkmark prefix once pinned - same convention
    /// every native window menu uses, rather than a static label that
    /// silently means the opposite of what it says half the time.
    pub fn open(wm: &WindowManager, window: WindowId, pos: (i32, i32)) -> Option<Self> {
        let w = wm.window(window)?;
        let maximize_label = if w.maximized { "Restore" } else { "Maximize" };
        let fullscreen_label = if w.fullscreen { "Exit Fullscreen" } else { "Fullscreen" };
        let floating_label = if w.floating { "\u{2713} Floating" } else { "Floating" };
        let pin_label = if w.always_on_top { "\u{2713} Always on Top" } else { "Always on Top" };
        let mut items = vec![
            ("Minimize", MenuAction::Minimize),
            (maximize_label, MenuAction::ToggleMaximize),
            (fullscreen_label, MenuAction::ToggleFullscreen),
            (floating_label, MenuAction::ToggleFloating),
            (pin_label, MenuAction::ToggleAlwaysOnTop),
        ];
        // One row per *other* workspace - skips the window's own current
        // one, since "move to the workspace it's already on" isn't a real
        // action. Flattened rather than a real submenu - `workspace.count`
        // is small in practice (this project's own default config uses 6),
        // so the menu stays a reasonable height without one.
        let others: Vec<&crate::Workspace> = wm.workspaces().iter().filter(|ws| ws.id != w.workspace).collect();
        if !others.is_empty() {
            items.push(("\u{2500}\u{2500}\u{2500} Move to Workspace \u{2500}\u{2500}\u{2500}", MenuAction::Separator));
            for ws in others {
                // `Workspace.name` is `&'static`-incompatible (a real
                // `String`, user-configurable via `workspace.names`) --
                // `Box::leak` turns it into the `&'static str` this
                // struct's own `items` field is typed for.
                let label: &'static str = Box::leak(ws.name.clone().into_boxed_str());
                items.push((label, MenuAction::MoveToWorkspace(ws.id)));
            }
        }
        items.push(("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", MenuAction::Separator));
        items.push(("Close", MenuAction::Close));
        Some(Self { window, pos, width: MENU_WIDTH, row_height: TITLEBAR_HEIGHT, items })
    }

    pub fn height(&self) -> i32 {
        self.row_height as i32 * self.items.len() as i32
    }

    /// Which row (if any) global-space point `(x, y)` falls on.
    pub fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        if x < self.pos.0 || x >= self.pos.0 + self.width as i32 {
            return None;
        }
        let rel_y = y - self.pos.1;
        if rel_y < 0 || rel_y >= self.height() {
            return None;
        }
        Some((rel_y / self.row_height as i32) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Window;

    fn wm_with_window() -> (WindowManager, WindowId) {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![crate::Monitor::new(0, "primary", crate::Rect::new(0, 0, 1920, 1080))]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        (wm, id)
    }

    #[test]
    fn open_labels_maximize_action_by_current_state() {
        let (mut wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert_eq!(menu.items[1].0, "Maximize");

        wm.toggle_maximize(id);
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert_eq!(menu.items[1].0, "Restore");
    }

    #[test]
    fn open_marks_pinned_state_on_the_always_on_top_row() {
        let (mut wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        // Minimize, Maximize, Fullscreen, Floating, then Always on Top.
        assert_eq!(menu.items[4].0, "Always on Top");

        wm.toggle_always_on_top(id);
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(menu.items[4].0.starts_with('\u{2713}'), "pinned state must be visible on the label itself");
    }

    #[test]
    fn open_returns_none_for_a_window_that_no_longer_exists() {
        let (wm, id) = wm_with_window();
        assert!(ContextMenu::open(&wm, id + 999, (0, 0)).is_none());
    }

    #[test]
    fn single_workspace_gets_no_move_to_workspace_section() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(!menu.items.iter().any(|(label, _)| label.contains("Workspace")));
        assert_eq!(menu.items.len(), 7, "Minimize, Maximize, Fullscreen, Floating, Always on Top, one separator, Close");
    }

    #[test]
    fn a_second_workspace_adds_a_move_row_but_not_for_its_own_workspace() {
        let (mut wm, id) = wm_with_window();
        wm.add_workspace("2", "dynamic");
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        let workspace_rows: Vec<&str> = menu
            .items
            .iter()
            .filter_map(|(label, action)| matches!(action, MenuAction::MoveToWorkspace(_)).then_some(*label))
            .collect();
        assert_eq!(workspace_rows, vec!["2"], "only the OTHER workspace gets a row, not the window's own");
    }

    #[test]
    fn clicking_a_separator_row_is_distinguishable_from_a_real_action() {
        let (mut wm, id) = wm_with_window();
        wm.add_workspace("2", "dynamic");
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        let separator_count = menu.items.iter().filter(|(_, a)| matches!(a, MenuAction::Separator)).count();
        assert_eq!(separator_count, 2, "one before the workspace section, one before Close");
    }

    #[test]
    fn row_at_maps_a_point_to_the_right_row() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert_eq!(menu.row_at(150, 100), Some(0), "top of the first row");
        assert_eq!(menu.row_at(150, 100 + TITLEBAR_HEIGHT as i32 - 1), Some(0), "bottom of the first row");
        assert_eq!(menu.row_at(150, 100 + TITLEBAR_HEIGHT as i32), Some(1), "top of the second row");
        assert_eq!(menu.row_at(150, 100 + menu.height() - 1), Some(menu.items.len() - 1), "last row, last pixel");
    }

    #[test]
    fn row_at_is_none_outside_the_menus_bounds() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert_eq!(menu.row_at(99, 110), None, "just left of the menu");
        assert_eq!(menu.row_at(100 + MENU_WIDTH as i32, 110), None, "just right of the menu");
        assert_eq!(menu.row_at(150, 99), None, "just above the menu");
        assert_eq!(menu.row_at(150, 100 + menu.height()), None, "just below the menu");
    }
}
