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
//!
//! Redesigned after being reported live as "looks very ugly and some of
//! it doesn't make sense": every row, including a separator, used to be a
//! full `TITLEBAR_HEIGHT`-tall slot (a hairline sitting in the middle of
//! 32px of empty space), and "Move to Workspace" faked a section header
//! by embedding box-drawing characters directly in a clickable-looking
//! item label (`"─── Move to Workspace ───"`) rather than being a real,
//! non-interactive row. Both are fixed below: separators and headers get
//! their own, much smaller row heights, and [`MenuAction::Header`] is a
//! real non-clickable row type instead of a label-text hack.

use crate::{WindowManager, WindowId, WorkspaceId};

#[derive(Clone, Copy)]
pub enum MenuAction {
    Minimize,
    ToggleMaximize,
    ToggleFullscreen,
    ToggleFloating,
    ToggleAlwaysOnTop,
    MoveToWorkspace(WorkspaceId),
    /// Cycles `ThemeConfig::traffic_light_buttons` - the same live knob
    /// `srd set button_style` flips, but applied here with an immediate
    /// redraw of every open window's titlebar (see `run_context_menu_
    /// action`'s own doc comment for why that's only possible from
    /// backend-specific code, not the generic IPC path). A theme setting
    /// is shared across every window by definition, not a per-window
    /// property - cycling it from one window's menu restyles all of
    /// them, the same way changing it in a settings app would.
    CycleButtonStyle,
    /// Cycles `ThemeConfig::buttons_left` - same shape as `CycleButtonStyle`.
    CycleButtonSide,
    Close,
    /// A purely visual divider row - no label, no click behaviour. `row_
    /// at` still resolves a click on one to `Some(index)` (it occupies
    /// real space, same as any other row), so the dispatch site is what
    /// actually no-ops on it, same "the row exists but does nothing"
    /// contract a real desktop's own menu separators have.
    Separator,
    /// A non-interactive section label (`"Move to Workspace"`, `"Customize"`)
    /// - dimmer, smaller text, never highlighted, click is a no-op just
    ///   like [`Self::Separator`]. Replaces an earlier hack that embedded
    ///   box-drawing characters directly in an ordinary item's label, which
    ///   rendered (and behaved, right up until the dispatch site's own
    ///   special-case) exactly like a clickable row that happened to do
    ///   nothing - confusing on both counts.
    Header,
}

impl MenuAction {
    /// Whether this row can ever be the target of a real click - shared
    /// by both backends' click-dispatch sites so `Separator`/`Header`
    /// can't drift out of sync with each other on which rows are inert.
    pub fn is_interactive(&self) -> bool {
        !matches!(self, MenuAction::Separator | MenuAction::Header)
    }
}

pub struct ContextMenu {
    pub window: WindowId,
    /// Top-left corner, in global (output-independent) space - same frame
    /// `Window.geometry` and every other rendered element's position uses.
    pub pos: (i32, i32),
    pub width: u32,
    /// A real, clickable item's own row height. `Separator`/`Header` rows
    /// use [`SEPARATOR_HEIGHT`]/[`HEADER_HEIGHT`] instead, regardless of
    /// this value - see [`Self::row_height_for`].
    pub row_height: u32,
    pub items: Vec<(&'static str, MenuAction)>,
}

const MENU_WIDTH: u32 = 170;
/// A real item's own row height, in logical pixels. Its own constant
/// rather than reusing `TITLEBAR_HEIGHT`: this is a stand-alone popup, not
/// a titlebar, and the two heights never needed to match - they just
/// happened to, which made a hairline separator take a full 32px slot to
/// draw a 1px line in. Sized against GNOME/KDE's own popover menu row
/// height (32-36px and 28-30px respectively), not this compositor's own
/// titlebar band.
pub const MENU_ROW_HEIGHT: u32 = 28;
/// A divider's own row height - just enough room for the hairline plus a
/// little breathing room on each side, not a full item row.
pub const SEPARATOR_HEIGHT: u32 = 9;
/// A section-label row's own height - shorter than a real item (it holds
/// smaller, dimmer text with nothing to click), taller than a plain
/// separator (it has to fit that text).
pub const HEADER_HEIGHT: u32 = 22;

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
        let pin_label = if w.always_on_top { "\u{2713} Always on Top" } else { "Always on Top" };
        let mut items = vec![("Minimize", MenuAction::Minimize), (maximize_label, MenuAction::ToggleMaximize), (fullscreen_label, MenuAction::ToggleFullscreen)];
        // "Floating" only means something under a layout that actually
        // tiles - `arrange_workspace` is the only reader of `Window::
        // floating` anywhere in this compositor, and it only runs for the
        // "tiling" layout (`"dynamic"`/`"floating"` never reposition
        // anyone regardless of this flag). Reported live as "doesn't make
        // sense": toggling it while running this project's own default
        // dynamic layout visibly does nothing at all, which reads as a
        // broken menu item rather than an inapplicable one. Shown only
        // when it would actually change something on screen.
        let layout_is_tiling = wm.workspace(w.workspace).map(|ws| ws.layout == "tiling").unwrap_or(false);
        if layout_is_tiling {
            let floating_label = if w.floating { "\u{2713} Floating" } else { "Floating" };
            items.push((floating_label, MenuAction::ToggleFloating));
        }
        items.push((pin_label, MenuAction::ToggleAlwaysOnTop));
        // One row per *other* workspace - skips the window's own current
        // one, since "move to the workspace it's already on" isn't a real
        // action. Flattened rather than a real submenu - `workspace.count`
        // is small in practice (this project's own default config uses 6),
        // so the menu stays a reasonable height without one.
        let others: Vec<&crate::Workspace> = wm.workspaces().iter().filter(|ws| ws.id != w.workspace).collect();
        if !others.is_empty() {
            items.push(("", MenuAction::Separator));
            items.push(("Move to Workspace", MenuAction::Header));
            for ws in others {
                // `Workspace.name` is `&'static`-incompatible (a real
                // `String`, user-configurable via `workspace.names`) --
                // `Box::leak` turns it into the `&'static str` this
                // struct's own `items` field is typed for.
                let label: &'static str = Box::leak(ws.name.clone().into_boxed_str());
                items.push((label, MenuAction::MoveToWorkspace(ws.id)));
            }
        }
        // Quick-access theme customization - reported live as a direct
        // ask ("allow customizing from there as well"), scoped to the two
        // knobs the same live request already named by name (traffic-
        // light vs. traditional buttons, which side they sit on): both
        // are shared theme settings, not per-window ones, so cycling
        // either restyles every open window's titlebar at once, same as
        // changing it in a config reload would - see `CycleButtonStyle`/
        // `CycleButtonSide`'s own doc comments for why that's still an
        // *immediate* redraw here rather than "takes effect next time you
        // open a window" the way the plain `srd set` path is scoped to.
        items.push(("", MenuAction::Separator));
        items.push(("Customize", MenuAction::Header));
        let button_style_label: &'static str = if wm.theme.traffic_light_buttons { "Button Style: Traffic Lights" } else { "Button Style: Traditional" };
        let button_side_label: &'static str = if wm.theme.buttons_left { "Button Side: Left" } else { "Button Side: Right" };
        items.push((button_style_label, MenuAction::CycleButtonStyle));
        items.push((button_side_label, MenuAction::CycleButtonSide));
        items.push(("", MenuAction::Separator));
        items.push(("Close", MenuAction::Close));
        Some(Self { window, pos, width: MENU_WIDTH, row_height: MENU_ROW_HEIGHT, items })
    }

    /// The height of row `index`, or `self.row_height` (a real item's own
    /// height) for an out-of-range index - callers that already know
    /// `index` is valid (every real one does) never hit that fallback; it
    /// exists so this can't panic if a future caller ever gets it wrong.
    pub fn row_height_for(&self, index: usize) -> u32 {
        match self.items.get(index) {
            Some((_, MenuAction::Separator)) => SEPARATOR_HEIGHT,
            Some((_, MenuAction::Header)) => HEADER_HEIGHT,
            _ => self.row_height,
        }
    }

    /// The y-offset row `index` starts at, relative to the menu's own top
    /// - every row height up to (not including) `index`, summed. `row_
    /// at`/rendering both walk rows this same way, so a mismatch between
    ///   "where a row is drawn" and "where a click resolves to" can't creep
    ///   in from computing the two differently.
    pub fn row_y(&self, index: usize) -> i32 {
        (0..index.min(self.items.len())).map(|i| self.row_height_for(i)).sum::<u32>() as i32
    }

    pub fn height(&self) -> i32 {
        self.row_y(self.items.len())
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
        let mut top = 0;
        for (i, _) in self.items.iter().enumerate() {
            let h = self.row_height_for(i) as i32;
            if rel_y < top + h {
                return Some(i);
            }
            top += h;
        }
        None
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

    fn wm_with_tiling_window() -> (WindowManager, WindowId) {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![crate::Monitor::new(0, "primary", crate::Rect::new(0, 0, 1920, 1080))]);
        wm.set_layout(wm.current_workspace(), "tiling");
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
    fn floating_row_is_hidden_outside_tiling_layout() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(!menu.items.iter().any(|(label, _)| label.contains("Floating")), "dynamic layout: toggling floating has no visible effect, so it must not be offered");
    }

    #[test]
    fn floating_row_is_shown_under_tiling_layout() {
        let (wm, id) = wm_with_tiling_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(menu.items.iter().any(|(label, _)| label.contains("Floating")), "tiling layout: toggling floating genuinely changes the window's geometry");
    }

    #[test]
    fn open_marks_pinned_state_on_the_always_on_top_row() {
        let (mut wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        // Minimize, Maximize, Fullscreen (no Floating - dynamic layout), Always on Top.
        assert_eq!(menu.items[3].0, "Always on Top");

        wm.toggle_always_on_top(id);
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(menu.items[3].0.starts_with('\u{2713}'), "pinned state must be visible on the label itself");
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
    fn header_rows_are_not_interactive() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        let headers: Vec<_> = menu.items.iter().filter(|(_, a)| matches!(a, MenuAction::Header)).collect();
        assert!(!headers.is_empty());
        for (_, action) in headers {
            assert!(!action.is_interactive());
        }
    }

    #[test]
    fn customize_section_reflects_current_theme_state() {
        let (mut wm, id) = wm_with_window();
        wm.theme.traffic_light_buttons = true;
        wm.theme.buttons_left = false;
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(menu.items.iter().any(|(label, _)| *label == "Button Style: Traffic Lights"));
        assert!(menu.items.iter().any(|(label, _)| *label == "Button Side: Right"));

        wm.theme.traffic_light_buttons = false;
        wm.theme.buttons_left = true;
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert!(menu.items.iter().any(|(label, _)| *label == "Button Style: Traditional"));
        assert!(menu.items.iter().any(|(label, _)| *label == "Button Side: Left"));
    }

    #[test]
    fn separators_and_headers_are_shorter_than_a_real_item_row() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        for (i, (_, action)) in menu.items.iter().enumerate() {
            let h = menu.row_height_for(i);
            match action {
                MenuAction::Separator => assert_eq!(h, SEPARATOR_HEIGHT),
                MenuAction::Header => assert_eq!(h, HEADER_HEIGHT),
                _ => assert_eq!(h, MENU_ROW_HEIGHT),
            }
        }
    }

    #[test]
    fn height_is_the_sum_of_every_rows_own_height() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        let expected: i32 = (0..menu.items.len()).map(|i| menu.row_height_for(i) as i32).sum();
        assert_eq!(menu.height(), expected);
    }

    #[test]
    fn row_at_maps_a_point_to_the_right_row_with_variable_heights() {
        let (wm, id) = wm_with_window();
        let menu = ContextMenu::open(&wm, id, (100, 100)).unwrap();
        assert_eq!(menu.row_at(150, 100), Some(0), "top of the first row");
        assert_eq!(menu.row_at(150, 100 + menu.row_height as i32 - 1), Some(0), "bottom of the first row");
        assert_eq!(menu.row_at(150, 100 + menu.row_height as i32), Some(1), "top of the second row");
        assert_eq!(menu.row_at(150, 100 + menu.height() - 1), Some(menu.items.len() - 1), "last row, last pixel");
        // Every row boundary in between must resolve to exactly one row --
        // the real regression this test guards against is a gap or an
        // overlap between two rows of different heights.
        for y in 0..menu.height() {
            assert!(menu.row_at(150, 100 + y).is_some(), "no gap at offset {y}");
        }
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
