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

/// How close to a monitor's top edge the drag pointer has to get before
/// the Snap-Layouts flyout drops down, in logical pixels.
///
/// Much larger than [`PlacementConfig::snap_threshold`] (8) on purpose, and
/// they measure different things: `snap_threshold` measures the dragged
/// *window's* edge against the screen edge and decides whether to commit a
/// snap, so it has to be tight or an ordinary reposition near the top
/// silently maximizes. This measures the *pointer* and only decides whether
/// to offer a menu, which costs nothing if ignored - the user throws the
/// cursor at the top of the screen, the way Windows 11's own gesture works,
/// and a tight band would just make it feel unreliable.
pub const SNAP_FLYOUT_EDGE: i32 = 12;

/// The six fixed screen positions offered by the Snap-Layouts flyout
/// (`crates/wayland/src/snap_flyout.rs`, opened by right-clicking a
/// titlebar's maximize button) - the click-driven equivalent of dragging a
/// window to that same edge/corner and releasing near it, addressed
/// directly by name instead of by proximity to a screen edge. Deliberately
/// only this subset of what `SmartPlacement::snap_zone` below already
/// computes from a drag position: full-maximize is excluded since it is
/// already the maximize button's own direct left-click action one click
/// away, and this scopes the flyout to "where should *this* window go" the
/// way most third-party snap tools (e.g. macOS's Rectangle) work, rather
/// than the full Windows 11 multi-window arrangement picker - a
/// meaningfully bigger feature (choosing a preset that places *several*
/// windows into complementary zones at once) that was not what was asked
/// for here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapZoneKind {
    LeftHalf,
    RightHalf,
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
}

impl SnapZoneKind {
    /// Grid order the flyout lays its cells out in - see
    /// `snap_flyout.rs`'s own doc comment for the actual layout.
    pub const ALL: [SnapZoneKind; 6] = [
        SnapZoneKind::LeftHalf,
        SnapZoneKind::RightHalf,
        SnapZoneKind::TopLeftQuarter,
        SnapZoneKind::TopRightQuarter,
        SnapZoneKind::BottomLeftQuarter,
        SnapZoneKind::BottomRightQuarter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SnapZoneKind::LeftHalf => "Left Half",
            SnapZoneKind::RightHalf => "Right Half",
            SnapZoneKind::TopLeftQuarter => "Top Left",
            SnapZoneKind::TopRightQuarter => "Top Right",
            SnapZoneKind::BottomLeftQuarter => "Bottom Left",
            SnapZoneKind::BottomRightQuarter => "Bottom Right",
        }
    }

    /// The rect this zone resolves to on `area` (a monitor's *usable*,
    /// exclusive-zone-shrunk geometry - matching `snap_zone` below, a
    /// half/quarter snap sits beside a bar/dock like any other tiled or
    /// deliberately-placed window, unlike maximize/fullscreen which
    /// deliberately covers it).
    pub fn rect(self, area: Rect) -> Rect {
        let half_w = area.width / 2;
        let half_h = area.height / 2;
        match self {
            SnapZoneKind::LeftHalf => Rect::new(area.x, area.y, half_w, area.height),
            SnapZoneKind::RightHalf => Rect::new(area.x + half_w as i32, area.y, half_w, area.height),
            SnapZoneKind::TopLeftQuarter => Rect::new(area.x, area.y, half_w, half_h),
            SnapZoneKind::TopRightQuarter => Rect::new(area.x + half_w as i32, area.y, half_w, half_h),
            SnapZoneKind::BottomLeftQuarter => Rect::new(area.x, area.y + half_h as i32, half_w, half_h),
            SnapZoneKind::BottomRightQuarter => Rect::new(area.x + half_w as i32, area.y + half_h as i32, half_w, half_h),
        }
    }
}

/// A `width` x `height` rect centred in `area`, clamped so it never starts
/// outside `area` even when it is larger than it.
///
/// Used for dialogs (see `WindowManager::add_window`). Integer division
/// biases a one-pixel remainder toward the top-left, which is the standard
/// convention and invisible in practice.
pub fn centered_in(area: Rect, width: u32, height: u32) -> Rect {
    let x = area.x + (area.width as i32 - width as i32) / 2;
    let y = area.y + (area.height as i32 - height as i32) / 2;
    Rect::new(x.max(area.x), y.max(area.y), width, height)
}

/// Moves `rect` so it sits inside `area` where it can, shrinking it only if
/// it is genuinely larger than `area`.
///
/// Position is corrected before size on purpose: a window that merely
/// overhangs an edge should slide back on screen at the size it asked for,
/// not be cut down to fit where it happened to land.
pub fn clamp_into(rect: Rect, area: Rect) -> Rect {
    let width = rect.width.min(area.width);
    let height = rect.height.min(area.height);
    let x = rect.x.clamp(area.x, (area.right() - width as i32).max(area.x));
    let y = rect.y.clamp(area.y, (area.bottom() - height as i32).max(area.y));
    Rect::new(x, y, width, height)
}

pub struct SmartPlacement;

impl SmartPlacement {
    /// Place a new window of `size` given the geometries of windows already
    /// occupying `monitor`. Tries a grid cell first, falling back to cascade.
    /// `cascade_step` is a monotonically increasing counter the caller owns
    /// (`WindowManager::next_cascade_step`) - see `cascade`'s own doc
    /// comment for why this can't just be `existing.len()` the way `grid`'s
    /// own cell count legitimately still is.
    ///
    /// Skips grid entirely when `existing` is empty, going straight to
    /// cascade instead: grid's real job is dividing space fairly among
    /// *concurrent* windows, and with nothing else open there is nothing
    /// to divide against - a 1x1 grid is mathematically one single cell
    /// no matter how it's rotated, so it can never vary by session
    /// history the way this reported bug needs. This is the actual
    /// overwhelmingly common case in practice (open one app, use it,
    /// close it, open the next), which is exactly why the bug this fixes
    /// ("every window opens in the same spot") was reported as the normal
    /// experience, not an edge case.
    pub fn place(monitor: &Monitor, existing: &[Rect], size: (u32, u32), cfg: &PlacementConfig, cascade_step: u32) -> Rect {
        if existing.is_empty() {
            return Self::cascade(monitor, size, cfg, cascade_step);
        }
        Self::grid(monitor, existing, size, cfg, cascade_step).unwrap_or_else(|| Self::cascade(monitor, size, cfg, cascade_step))
    }

    fn grid(monitor: &Monitor, existing: &[Rect], size: (u32, u32), cfg: &PlacementConfig, cascade_step: u32) -> Option<Rect> {
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

        // Cells are visited starting from a different one each time rather
        // than always from the top-left, so consecutive windows do not all
        // pile into the same corner. Reported as windows spawning
        // "predominantly left side": the scan returned the first free cell
        // in reading order, which is the leftmost one that happens to be
        // free, over and over.
        let cells = grid_size * grid_size;
        let start = cascade_step % cells.max(1);
        for offset in 0..cells {
            let cell = (start + offset) % cells;
            let (gx, gy) = (cell % grid_size, cell / grid_size);
            let x = area.x + cfg.grid_margin as i32 + (gx * (cell_w + cfg.grid_margin)) as i32;
            let y = area.y + cfg.grid_margin as i32 + (gy * (cell_h + cfg.grid_margin)) as i32;
            let candidate = Rect::new(x, y, cell_w, cell_h);
            if !existing.iter().any(|w| w.overlaps(&candidate)) {
                // The window keeps the size it actually asked for. Shrinking
                // it to the cell is what made every window come out the same
                // boxy shape regardless of what it wanted - reported as
                // windows spawning "as squares". The cell decides *where* a
                // window goes, not how big it is.
                //
                // Clamped into the usable area afterwards so a window bigger
                // than its cell still lands fully on screen rather than
                // hanging off the edge with its border out of view --
                // reported in the same breath as spawning "a little bit out
                // of view, ie i can't see a border".
                return Some(clamp_into(Rect::new(x, y, size.0, size.1), area));
            }
        }
        None
    }

    /// Diagonal cascade, stepping by `cascade_offset` per window opened so
    /// far and wrapping back to the origin once it would run off the
    /// monitor.
    ///
    /// Driven by `cascade_step` - a counter the caller keeps incrementing
    /// across the whole session - rather than `existing.len()` (how many
    /// windows happen to be open on this workspace *right now*), which is
    /// what this used to take. That reads as reasonable ("cascade further
    /// when more windows are open") but has a real, reported bug baked in:
    /// the overwhelmingly common way people actually use a desktop is one
    /// app at a time - open, use, close, open the next - and `existing`
    /// is empty at the start of every single one of those opens, so `step`
    /// was `0` every time regardless of how many windows had already been
    /// opened-and-closed that session. Reported live as "every window
    /// spawns in the exact same place and size, not at all like Windows" --
    /// confirmed by reading this function, not guessed: real Windows
    /// cascades the *next* window further even after you close the
    /// previous one, which needs a counter that survives a window closing,
    /// not one derived from whoever is still open at the moment of the
    /// next placement.
    fn cascade(monitor: &Monitor, size: (u32, u32), cfg: &PlacementConfig, cascade_step: u32) -> Rect {
        let area = monitor.geometry;
        let width = size.0.min(area.width);
        let height = size.1.min(area.height);

        let max_steps_x = ((area.width as i32 - width as i32) / cfg.cascade_offset.max(1)).max(1);
        let max_steps_y = ((area.height as i32 - height as i32) / cfg.cascade_offset.max(1)).max(1);
        let max_steps = max_steps_x.min(max_steps_y).max(1);

        let step = (cascade_step as i32) % max_steps;
        let x = area.x + cfg.cascade_offset + step * cfg.cascade_offset;
        let y = area.y + cfg.cascade_offset + step * cfg.cascade_offset;
        clamp_into(Rect::new(x, y, width, height), area)
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
    fn a_window_opened_alone_cascades_rather_than_using_a_pointless_1x1_grid() {
        // `place` skips `grid` entirely when nothing else is open - see
        // its own doc comment for why: a grid with nothing to divide space
        // against is always exactly one cell, which can never vary by
        // session history, and "one app open at a time" is the ordinary
        // case, not an edge one.
        let cfg = PlacementConfig::default();
        let r = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg, 0);
        assert_eq!(r.x, cfg.cascade_offset);
        assert_eq!(r.y, cfg.cascade_offset);
    }

    #[test]
    fn opening_the_same_app_alone_twice_in_a_row_lands_in_different_spots() {
        // The concrete reported symptom, exercised through the real
        // `place` entry point (not `cascade` directly, unlike the more
        // targeted unit test below) - opening one window, closing it, and
        // opening another must not silently collapse back to the exact
        // same spot just because `existing` is empty again both times.
        let cfg = PlacementConfig::default();
        let first = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg, 0);
        let second = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg, 1);
        assert_ne!(first, second);
    }

    #[test]
    fn grid_avoids_occupied_cells() {
        let cfg = PlacementConfig::default();
        let first = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg, 0);
        let second = SmartPlacement::place(&monitor(), &[first], (400, 300), &cfg, 1);
        assert!(!first.overlaps(&second), "second window must not overlap the first: {first:?} vs {second:?}");
    }

    #[test]
    fn cascade_kicks_in_once_grid_is_full() {
        let cfg = PlacementConfig { max_grid: 1, ..Default::default() };
        // max_grid=1 means the grid is always a single cell, so a second
        // window can never find a free grid cell and must cascade.
        let first = SmartPlacement::place(&monitor(), &[], (400, 300), &cfg, 0);
        let second = SmartPlacement::place(&monitor(), &[first], (400, 300), &cfg, 1);
        assert_ne!(first, second);
        // First window is grid-placed (offset by grid_margin); the second no
        // longer fits any grid cell and falls back to cascade, which steps
        // from the monitor origin by `cascade_offset` per window opened so
        // far this session (the caller's own counter, passed in as `1` here).
        assert_eq!(second.x, cfg.cascade_offset * 2);
        assert_eq!(second.y, cfg.cascade_offset * 2);
    }

    #[test]
    fn cascade_step_keeps_advancing_even_if_the_previous_window_closed() {
        // The actual reported bug this counter exists to fix: opening one
        // window at a time (closing each before the next) used to reset
        // `existing` to empty every time, so `step` - driven by `existing.
        // len()` - was always 0 regardless of how many windows had already
        // been opened-and-closed. A cascade_step the caller keeps
        // incrementing across the session, independent of what is
        // currently open, is what actually fixes it. Calls `cascade`
        // directly (not `place`): `place`'s own grid-first fallback would
        // succeed for an empty `existing` regardless of this test's own
        // point (a grid's cell *count* legitimately does depend on live
        // occupancy - see `place`'s own doc comment on why only `cascade`
        // takes this counter), so a `place`-level test couldn't actually
        // isolate cascade's own behavior here.
        let cfg = PlacementConfig::default();
        let first = SmartPlacement::cascade(&monitor(), (400, 300), &cfg, 0);
        let second = SmartPlacement::cascade(&monitor(), (400, 300), &cfg, 1);
        assert_ne!(first, second, "an unchanged cascade_step of 0 vs 1 must not collapse to the same spot");
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

    #[test]
    fn grid_placement_keeps_the_size_the_window_asked_for() {
        // The reported "windows spawn as squares": every window used to be
        // shrunk to its grid cell, so they all came out the same shape no
        // matter what size they wanted.
        let cfg = PlacementConfig::default();
        let existing = [Rect::new(0, 0, 100, 100)];
        let r = SmartPlacement::place(&monitor(), &existing, (1200, 400), &cfg, 0);
        assert_eq!((r.width, r.height), (1200, 400), "the requested size must survive placement");
    }

    #[test]
    fn a_window_never_lands_partly_off_screen() {
        // "sometimes a little bit out of view, ie i can't see a border".
        let cfg = PlacementConfig::default();
        let area = monitor().geometry;
        for step in 0..40u32 {
            for size in [(400, 300), (1600, 900), (1900, 1000)] {
                let r = SmartPlacement::place(&monitor(), &[Rect::new(0, 0, 50, 50)], size, &cfg, step);
                assert!(
                    r.x >= area.x && r.y >= area.y && r.right() <= area.right() && r.bottom() <= area.bottom(),
                    "step {step} size {size:?} landed at {r:?}, outside {area:?}"
                );
            }
        }
    }

    #[test]
    fn consecutive_windows_do_not_all_pile_into_the_same_corner() {
        // "windows predominately spawn left side": the grid scan always
        // returned the first free cell in reading order.
        let cfg = PlacementConfig::default();
        let existing = [Rect::new(900, 500, 80, 80)];
        let xs: Vec<i32> = (0..4).map(|step| SmartPlacement::place(&monitor(), &existing, (300, 200), &cfg, step).x).collect();
        assert!(xs.iter().any(|&x| x != xs[0]), "every placement started at the same x: {xs:?}");
    }

    #[test]
    fn a_window_larger_than_the_whole_screen_is_cut_down_to_it() {
        let cfg = PlacementConfig::default();
        let r = SmartPlacement::place(&monitor(), &[Rect::new(0, 0, 50, 50)], (4000, 3000), &cfg, 0);
        let area = monitor().geometry;
        assert_eq!((r.width, r.height), (area.width, area.height));
        assert_eq!((r.x, r.y), (area.x, area.y));
    }

    #[test]
    fn snap_zone_kind_halves_split_the_area_down_the_middle() {
        let area = monitor().geometry;
        assert_eq!(SnapZoneKind::LeftHalf.rect(area), Rect::new(0, 0, 960, 1080));
        assert_eq!(SnapZoneKind::RightHalf.rect(area), Rect::new(960, 0, 960, 1080));
    }

    #[test]
    fn snap_zone_kind_quarters_tile_the_area_with_no_gap_or_overlap() {
        let area = monitor().geometry;
        let quarters = [
            SnapZoneKind::TopLeftQuarter.rect(area),
            SnapZoneKind::TopRightQuarter.rect(area),
            SnapZoneKind::BottomLeftQuarter.rect(area),
            SnapZoneKind::BottomRightQuarter.rect(area),
        ];
        for (i, a) in quarters.iter().enumerate() {
            for b in &quarters[i + 1..] {
                assert!(!a.overlaps(b), "{a:?} and {b:?} must not overlap");
            }
        }
        let covered: u32 = quarters.iter().map(|r| r.width * r.height).sum();
        assert_eq!(covered, area.width * area.height, "quarters must cover the whole area with no gap");
    }

    #[test]
    fn snap_zone_kind_all_has_no_duplicates() {
        let area = monitor().geometry;
        let rects: Vec<_> = SnapZoneKind::ALL.iter().map(|z| z.rect(area)).collect();
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert_ne!(a, b, "two different zones must not resolve to the same rect");
            }
        }
    }
}
