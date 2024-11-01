//! Snap-Layouts flyout - a Windows-11-style "pick where this window goes"
//! grid, opened by right-clicking a titlebar's maximize button. Scoped to
//! picking a position for the *one* window whose button was clicked, not
//! the full Windows 11 multi-window arrangement picker (choosing a preset
//! that places several windows into complementary zones at once) - see
//! `srdwm_core::SnapZoneKind`'s own doc comment for why that bigger feature
//! is deliberately out of scope here.
//!
//! Deliberately plain, matching `context_menu.rs`'s own stated bar: a fixed
//! 3-column by 2-row grid of six labeled cells, no live preview thumbnails,
//! no drag-to-multi-zone. See `decoration::render_snap_flyout` for the
//! actual pixels.

use srdwm_core::{SnapZoneKind, WindowId};

pub(crate) struct SnapFlyout {
    pub(crate) window: WindowId,
    /// Top-left corner, in global (output-independent) space - same frame
    /// `Window.geometry` and every other `custom_elements` position uses.
    pub(crate) pos: (i32, i32),
    pub(crate) cell_width: u32,
    pub(crate) cell_height: u32,
}

const COLUMNS: i32 = 3;
const ROWS: i32 = 2;
const CELL_WIDTH: u32 = 90;
const CELL_HEIGHT: u32 = 60;

impl SnapFlyout {
    /// Opens the flyout for `window`, top-left corner at `pos` - by
    /// convention the maximize button's own titlebar position, matching
    /// `ContextMenu::open`'s "wherever the click landed" convention.
    pub(crate) fn open(window: WindowId, pos: (i32, i32)) -> Self {
        Self { window, pos, cell_width: CELL_WIDTH, cell_height: CELL_HEIGHT }
    }

    pub(crate) fn width(&self) -> u32 {
        self.cell_width * COLUMNS as u32
    }

    pub(crate) fn height(&self) -> u32 {
        self.cell_height * ROWS as u32
    }

    /// Grid cell order, left-to-right then top-to-bottom: halves on the
    /// left column pair, quarters filling the remaining two columns --
    /// [`SnapZoneKind::ALL`]'s own declared order.
    pub(crate) fn cells(&self) -> [SnapZoneKind; 6] {
        SnapZoneKind::ALL
    }

    /// Which zone (if any) global-space point `(x, y)` falls on.
    pub(crate) fn zone_at(&self, x: i32, y: i32) -> Option<SnapZoneKind> {
        let (rel_x, rel_y) = (x - self.pos.0, y - self.pos.1);
        if rel_x < 0 || rel_x >= self.width() as i32 || rel_y < 0 || rel_y >= self.height() as i32 {
            return None;
        }
        let col = rel_x / self.cell_width as i32;
        let row = rel_y / self.cell_height as i32;
        let index = (row * COLUMNS + col) as usize;
        self.cells().get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_at_top_left_cell_is_the_first_zone_in_reading_order() {
        let flyout = SnapFlyout::open(1, (100, 100));
        assert_eq!(flyout.zone_at(101, 101), Some(flyout.cells()[0]));
    }

    #[test]
    fn zone_at_maps_every_cell_to_a_distinct_zone() {
        let flyout = SnapFlyout::open(1, (0, 0));
        let mut seen = Vec::new();
        for row in 0..ROWS {
            for col in 0..COLUMNS {
                let x = col * CELL_WIDTH as i32 + 1;
                let y = row * CELL_HEIGHT as i32 + 1;
                let zone = flyout.zone_at(x, y).unwrap_or_else(|| panic!("no zone at ({x},{y})"));
                assert!(!seen.contains(&zone), "zone {zone:?} hit twice");
                seen.push(zone);
            }
        }
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn zone_at_is_none_outside_the_flyouts_bounds() {
        let flyout = SnapFlyout::open(1, (100, 100));
        assert_eq!(flyout.zone_at(99, 110), None, "just left of the flyout");
        assert_eq!(flyout.zone_at(100 + flyout.width() as i32, 110), None, "just right of the flyout");
        assert_eq!(flyout.zone_at(150, 99), None, "just above the flyout");
        assert_eq!(flyout.zone_at(150, 100 + flyout.height() as i32), None, "just below the flyout");
    }
}
