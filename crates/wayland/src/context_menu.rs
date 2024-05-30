//! Right-click titlebar window menu - the one titlebar interaction
//! virtually every desktop WM has always offered that srdwm never did.
//! Right-click on a titlebar previously did nothing at all (the only
//! right-button behaviour anywhere was the SUPER+right-drag resize
//! gesture, which needs the modifier held); this gives plain right-click
//! a real, discoverable action.
//!
//! Deliberately minimal: four fixed actions, no submenus, no live hover
//! highlight (a nice-to-have that would need the render buffer rebuilt on
//! every pointer-motion event over the menu - not worth the extra
//! per-frame cost for a first pass). See `decoration::render_context_menu`
//! for the actual pixels.

use srdwm_core::{WindowId, WindowManager, TITLEBAR_HEIGHT};

#[derive(Clone, Copy)]
pub(crate) enum MenuAction {
    Minimize,
    ToggleMaximize,
    ToggleAlwaysOnTop,
    Close,
}

pub(crate) struct ContextMenu {
    pub(crate) window: WindowId,
    /// Top-left corner, in global (output-independent) space - same frame
    /// `Window.geometry` and every other `custom_elements` position uses.
    pub(crate) pos: (i32, i32),
    pub(crate) width: u32,
    pub(crate) row_height: u32,
    pub(crate) items: Vec<(&'static str, MenuAction)>,
}

const MENU_WIDTH: u32 = 170;

impl ContextMenu {
    /// Builds the menu for `window`, opening with its top-left corner at
    /// `pos` (wherever the right-click landed). Labels reflect the
    /// window's *current* state - "Maximize" flips to "Restore", "Always
    /// on Top" gets a checkmark prefix once pinned - same convention
    /// every native window menu uses, rather than a static label that
    /// silently means the opposite of what it says half the time.
    pub(crate) fn open(wm: &WindowManager, window: WindowId, pos: (i32, i32)) -> Option<Self> {
        let w = wm.window(window)?;
        let maximize_label = if w.maximized { "Restore" } else { "Maximize" };
        let pin_label = if w.always_on_top { "\u{2713} Always on Top" } else { "Always on Top" };
        let items = vec![
            ("Minimize", MenuAction::Minimize),
            (maximize_label, MenuAction::ToggleMaximize),
            (pin_label, MenuAction::ToggleAlwaysOnTop),
            ("Close", MenuAction::Close),
        ];
        Some(Self { window, pos, width: MENU_WIDTH, row_height: TITLEBAR_HEIGHT, items })
    }

    pub(crate) fn height(&self) -> i32 {
        self.row_height as i32 * self.items.len() as i32
    }

    /// Which row (if any) global-space point `(x, y)` falls on.
    pub(crate) fn row_at(&self, x: i32, y: i32) -> Option<usize> {
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
    use srdwm_core::Window;

    fn wm_with_window() -> (WindowManager, WindowId) {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![srdwm_core::Monitor::new(0, "primary", srdwm_core::Rect::new(0, 0, 1920, 1080))]);
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
        assert_eq!(menu.items[2].0, "Always on Top");

        wm.toggle_always_on_top(id);
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(menu.items[2].0.starts_with('\u{2713}'), "pinned state must be visible on the label itself");
    }

    #[test]
    fn open_returns_none_for_a_window_that_no_longer_exists() {
        let (wm, id) = wm_with_window();
        assert!(ContextMenu::open(&wm, id + 999, (0, 0)).is_none());
    }

    #[test]
    fn row_at_maps_a_point_to_the_right_row() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert_eq!(menu.row_at(150, 100), Some(0), "top of the first row");
        assert_eq!(menu.row_at(150, 100 + TITLEBAR_HEIGHT as i32 - 1), Some(0), "bottom of the first row");
        assert_eq!(menu.row_at(150, 100 + TITLEBAR_HEIGHT as i32), Some(1), "top of the second row");
        assert_eq!(menu.row_at(150, 100 + menu.height() - 1), Some(3), "last row, last pixel");
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
