//! Smart window placement: Windows-11-style grid placement, cascade fallback,
//! and Windows-Snap-style edge magnetism for drags.
//!
//! This reimplements the intent of the legacy C++ `SmartPlacement` class, but
//! fixes several bugs found in the original (see docs/PRIOR_ART.md):
//! - grid placement used a `static` round-robin counter that hardcoded a
//!   2-column layout and never tracked real cell occupancy; here we scan the
//!   actual grid for the first cell that doesn't overlap an existing window.
//! - cascade placement didn't cascade at all (it reused the first free-space
//!   sample); here new windows step diagonally by `cascade_offset` and wrap.
//! - snap-to-edge always returned a fixed centered rect; here it computes a
//!   real Windows-Snap-style half/quarter/maximize zone from drag position.

use crate::geometry::Rect;
use crate::monitor::Monitor;

pub const MIN_WINDOW_WIDTH: u32 = 200;
pub const MIN_WINDOW_HEIGHT: u32 = 150;

#[derive(Debug, Clone, Copy)]
pub struct PlacementConfig {
    pub grid_margin: u32,
    pub cascade_offset: i32,
    /// How close (in logical pixels) a dragged window's edge has to end up
    /// to a monitor edge on release before `snap_zone` triggers a
    /// half/quarter/maximize. A single edge match with no corner match
    /// (e.g. top-only) maximizes the *whole* window - see `snap_zone`'s
    /// `(false, false, true, false) => area` arm - so this value directly
    /// controls how easy it is to accidentally full-maximize a window while
    /// just repositioning it near the top of the screen, not only how
    /// generous the corner/half-snap zones are.
    pub snap_threshold: i32,
    pub max_grid: u32,
}

impl Default for PlacementConfig {
    fn default() -> Self {
        // `snap_threshold` was 50, then 20 - both live-tested and reported
        // as still snapping from an ordinary "move it near an edge" drag,
        // not just a deliberate release-at-the-edge one. `update_drag`'s
        // clamp used to also cap a dragged window's reach to the
        // exclusive-zone-shrunk usable area rather than the monitor's true
        // edge (see `Monitor::full_geometry`), which made this worse than
        // the number alone suggests: the window could get within 20px of
        // `snap_zone`'s comparison edge well before the cursor was
        // anywhere near the real screen edge. 8 keeps snapping reachable
        // (a window's own edge, not the cursor, is what's measured) while
        // requiring it to actually be at the edge, not just closer to it
        // than to the middle of the screen.
        Self { grid_margin: 10, cascade_offset: 30, snap_threshold: 8, max_grid: 4 }
    }
}

pub struct SmartPlacement;

impl SmartPlacement {
    /// Place a new window of `size` given the geometries of windows already
    /// occupying `monitor`. Tries a grid cell first, falling back to cascade.
    pub fn place(monitor: &Monitor, existing: &[Rect], size: (u32, u32), cfg: &PlacementConfig) -> Rect {
        Self::grid(monitor, existing, size, cfg).unwrap_or_else(|| Self::cascade(monitor, existing, size, cfg))
    }

    fn grid(monitor: &Monitor, existing: &[Rect], size: (u32, u32), cfg: &PlacementConfig) -> Option<Rect> {
        let count = existing.len() + 1;
        let grid_size = (count as f64).sqrt().ceil() as u32;
        let grid_size = grid_size.clamp(1, cfg.max_grid);
        let area = monitor.geometry;

        let margins = cfg.grid_margin * (grid_size + 1);
        if area.width <= margins || area.height <= margins {
            return None;
        }
        let cell_w = (area.width - margins) / grid_size;
        let cell_h = (area.height - margins) / grid_size;
        if cell_w < MIN_WINDOW_WIDTH || cell_h < MIN_WINDOW_HEIGHT {
            return None;
        }

        for gy in 0..grid_size {
            for gx in 0..grid_size {
                let x = area.x + cfg.grid_margin as i32 + (gx * (cell_w + cfg.grid_margin)) as i32;
                let y = area.y + cfg.grid_margin as i32 + (gy * (cell_h + cfg.grid_margin)) as i32;
                let candidate = Rect::new(x, y, cell_w, cell_h);
                if !existing.iter().any(|w| w.overlaps(&candidate)) {
                    return Some(Rect::new(x, y, size.0.min(cell_w), size.1.min(cell_h)));
                }
            }
        }
        None
    }

    /// Diagonal cascade, stepping by `cascade_offset` per already-placed
    /// window and wrapping back to the origin once it would run off the
    /// monitor.
    fn cascade(monitor: &Monitor, existing: &[Rect], size: (u32, u32), cfg: &PlacementConfig) -> Rect {
        let area = monitor.geometry;
        let width = size.0.min(area.width);
        let height = size.1.min(area.height);

        let max_steps_x = ((area.width as i32 - width as i32) / cfg.cascade_offset.max(1)).max(1);
        let max_steps_y = ((area.height as i32 - height as i32) / cfg.cascade_offset.max(1)).max(1);
        let max_steps = max_steps_x.min(max_steps_y).max(1);

        let step = existing.len() as i32 % max_steps;
        let x = (area.x + cfg.cascade_offset + step * cfg.cascade_offset).min(area.right() - width as i32).max(area.x);
        let y = (area.y + cfg.cascade_offset + step * cfg.cascade_offset).min(area.bottom() - height as i32).max(area.y);
        Rect::new(x, y, width, height)
    }

    /// Given a window being dragged (its live geometry) and the monitor it's
    /// on, returns the Windows-Snap zone it should resize to if it's within
    /// `snap_threshold` pixels of a screen edge or corner, or `None` if it's
    /// not near any snap zone.
    pub fn snap_zone(dragged: Rect, monitor: &Monitor, cfg: &PlacementConfig) -> Option<Rect> {
        let area = monitor.geometry;
        let t = cfg.snap_threshold;
        let near_left = (dragged.x - area.x).abs() <= t;
        let near_right = (area.right() - dragged.right()).abs() <= t;
        let near_top = (dragged.y - area.y).abs() <= t;
        let near_bottom = (area.bottom() - dragged.bottom()).abs() <= t;

        let half_w = area.width / 2;
        let half_h = area.height / 2;

        Some(match (near_left, near_right, near_top, near_bottom) {
            (true, false, true, false) => Rect::new(area.x, area.y, half_w, half_h),
            (false, true, true, false) => Rect::new(area.x + half_w as i32, area.y, half_w, half_h),
            (true, false, false, true) => Rect::new(area.x, area.y + half_h as i32, half_w, half_h),
            (false, true, false, true) => Rect::new(area.x + half_w as i32, area.y + half_h as i32, half_w, half_h),
            (true, false, false, false) => Rect::new(area.x, area.y, half_w, area.height),
            (false, true, false, false) => Rect::new(area.x + half_w as i32, area.y, half_w, area.height),
            (false, false, true, false) => area,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Monitor {
        Monitor::new(0, "test", Rect::new(0, 0, 1920, 1080))
    }

    #[test]
    fn first_window_goes_in_top_left_grid_cell() {
        let cfg = PlacementConfig::default();
        let r = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg);
        assert_eq!(r.x, cfg.grid_margin as i32);
        assert_eq!(r.y, cfg.grid_margin as i32);
    }

    #[test]
    fn grid_avoids_occupied_cells() {
        let cfg = PlacementConfig::default();
        let first = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg);
        let second = SmartPlacement::place(&monitor(), &[first], (400, 300), &cfg);
        assert!(!first.overlaps(&second), "second window must not overlap the first: {first:?} vs {second:?}");
    }

    #[test]
    fn cascade_kicks_in_once_grid_is_full() {
        let cfg = PlacementConfig { max_grid: 1, ..Default::default() };
        // max_grid=1 means the grid is always a single cell, so a second
        // window can never find a free grid cell and must cascade.
        let first = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg);
        let second = SmartPlacement::place(&monitor(), &[first], (400, 300), &cfg);
        assert_ne!(first, second);
        // First window is grid-placed (offset by grid_margin); the second no
        // longer fits any grid cell and falls back to cascade, which steps
        // from the monitor origin by `cascade_offset` per already-placed window.
        assert_eq!(second.x, cfg.cascade_offset * 2);
        assert_eq!(second.y, cfg.cascade_offset * 2);
    }

    #[test]
    fn snap_left_edge_yields_left_half() {
        let cfg = PlacementConfig::default();
        let dragged = Rect::new(2, 100, 400, 300); // x=2 is within threshold of left edge
        let zone = SmartPlacement::snap_zone(dragged, &monitor(), &cfg).unwrap();
        assert_eq!(zone, Rect::new(0, 0, 960, 1080));
    }

    #[test]
    fn snap_top_edge_yields_maximize() {
        let cfg = PlacementConfig::default();
        let dragged = Rect::new(500, 1, 400, 300);
        let zone = SmartPlacement::snap_zone(dragged, &monitor(), &cfg).unwrap();
        assert_eq!(zone, monitor().geometry);
    }

    #[test]
    fn snap_top_left_corner_yields_quarter() {
        let cfg = PlacementConfig::default();
        let dragged = Rect::new(1, 1, 400, 300);
        let zone = SmartPlacement::snap_zone(dragged, &monitor(), &cfg).unwrap();
        assert_eq!(zone, Rect::new(0, 0, 960, 540));
    }

    #[test]
    fn no_snap_away_from_edges() {
        let cfg = PlacementConfig::default();
        let dragged = Rect::new(700, 400, 400, 300);
        assert!(SmartPlacement::snap_zone(dragged, &monitor(), &cfg).is_none());
    }
}
