use crate::geometry::Rect;

pub type MonitorId = u32;

#[derive(Debug, Clone)]
pub struct Monitor {
    pub id: MonitorId,
    /// Usable area: the output rect shrunk by any layer-shell exclusive
    /// zone (a bar/dock). What placement, tiling and maximize target --
    /// see `full_geometry`'s doc comment for the one thing that
    /// deliberately does *not* use this field.
    pub geometry: Rect,
    /// The output's true full rect, ignoring any exclusive zone.
    ///
    /// Kept separate from `geometry` because "respects the dock" and
    /// "doesn't" are two genuinely different behaviors a window needs,
    /// not one setting: fullscreen (and a window being interactively
    /// dragged) should be able to cover or cross the strip a bar/dock
    /// reserves - the bar just renders on top, as an overlay, the same
    /// way it does everywhere else - while a *new* window's placement,
    /// tiling and maximize should keep avoiding that strip, same as
    /// before. Defaults to `geometry` (no reservation) for any backend
    /// that hasn't been taught the distinction yet.
    pub full_geometry: Rect,
    /// The rect `toggle_maximize` targets: `full_geometry` with only a
    /// top-anchored bar's exclusive zone (a menu bar, always expected to
    /// stay visible/reachable) subtracted back out again - a dock anchored
    /// to any other edge is deliberately left alone, same as
    /// `full_geometry`. Two behaviors maximize needs that neither
    /// `geometry` (shrunk by *every* zone) nor `full_geometry` (shrunk by
    /// none) can express on its own: "go past the dock" and "still stop at
    /// the top bar" are both true at once, on the user's own explicit
    /// call when the two pulled in opposite directions (`full_geometry` had
    /// briefly covered both, which un-did "stop at the top bar" as a side
    /// effect of fixing "go past the dock").
    ///
    /// Defaults to `geometry` (no reservation ignored at all) for any
    /// backend or test that hasn't been taught the per-edge distinction --
    /// same conservative-default reasoning as `full_geometry`'s own doc
    /// comment.
    pub maximize_geometry: Rect,
    pub name: String,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
    /// `true` when this entry is one part of a real output divided by
    /// `srd.monitor.split` - not a second `wl_output`, not a second
    /// physical connector. A display-arrangement UI reads this to tell a
    /// split part apart from a genuinely separate monitor, so it does not
    /// offer to move or extend a physical arrangement onto something that
    /// is not a real, independent output. `false` for an ordinary,
    /// undivided output.
    pub split: bool,
    /// This output's real scale factor (automatic, from `srdwm_core::
    /// monitor::auto_scale_for`, or an explicit `srd.monitor.scale`
    /// override) - `1.0` for an unscaled output. Every other field on
    /// this struct (`geometry`, `full_geometry`, `maximize_geometry`) is
    /// in *physical* pixels, not the logical points a Wayland client
    /// itself sees; a caller that needs to convert between the two (a
    /// display-arrangement UI chaining outputs by their reported size,
    /// for instance) multiplies logical by this to get physical, or
    /// divides physical by this to get logical. Requested directly by the
    /// AGS peer session after a real bug (`srd dispatch set output
    /// position` and this compositor's own physical-pixel bookkeeping
    /// silently disagreeing with a client's logical one at any scale
    /// other than `1.0`) traced back to exactly this missing piece of
    /// information.
    pub scale: f64,
    /// `true` for a fully virtual/headless output created by `srd dispatch
    /// create fake-monitor` (`crates/wayland/src/udev/virtual_heads.rs`) --
    /// a real, independent `wl_output` global with no DRM connector behind
    /// it. `false` for every ordinary connected output, split part
    /// included (`split` and `is_virtual` are independent: a split part is
    /// still a real output's own rectangle, not a second `wl_output`).
    ///
    /// Requested directly by the AGS peer session after a fake monitor's
    /// `wl_output` caused a real live incident: a fake output looks like an
    /// ordinary new monitor to any client watching the core Wayland
    /// registry (not just `wlr-output-management-v1`, which already
    /// deliberately excludes it - see `virtual_heads.rs`'s own module doc
    /// comment), so AGS's own remembered-layout restore treated it as a
    /// real hotplug and repositioned the *real* monitor to make room for
    /// it, twice, once per fake monitor created. AGS's own fix was a
    /// name-pattern match (`/^FAKE-/i`) since nothing else in `srd
    /// monitors`' output let it tell a fake output apart from a real one --
    /// this field is the real discriminator that match was standing in for.
    pub is_virtual: bool,
}

impl Monitor {
    pub fn new(id: MonitorId, name: impl Into<String>, geometry: Rect) -> Self {
        Self {
            id,
            name: name.into(),
            geometry,
            full_geometry: geometry,
            maximize_geometry: geometry,
            refresh_rate_mhz: 60_000,
            primary: false,
            split: false,
            scale: 1.0,
            is_virtual: false,
        }
    }
}

/// A connector a backend has administratively disabled (`srd dispatch set
/// output enabled <name> false`) but that's still physically connected --
/// purely informational, reported by the backend via `WindowManager::
/// set_disabled_monitor` for `srd monitors`/the `monitors` subscribe event
/// to list (so a display-settings UI can offer to turn it back on by
/// name), and deliberately never fed into `WindowManager::monitors()` or
/// any real placement/tiling logic, which continues to see only genuinely
/// live outputs exactly as before this existed. Geometry is a last-known
/// snapshot from the moment it was disabled - stale by construction, and
/// meant to be: a caller wanting to reposition it correctly re-queries
/// once it's actually re-enabled, not from this.
#[derive(Debug, Clone)]
pub struct DisabledMonitor {
    pub geometry: Rect,
    pub full_geometry: Rect,
    pub primary: bool,
}

/// A `srd.monitor.split(name, parts, direction)` config-time request:
/// divide one real output into `parts` equal (within a pixel) logical
/// [`Monitor`] entries, so placement/tiling can treat them as separate
/// screens without any DRM/`wl_output` involvement - see `split_rect`'s
/// own doc comment for the actual division, and the udev platform's
/// `monitors()` for where this turns into real `Monitor` entries.
///
/// Deliberately just a division of one real output's rectangle for
/// placement purposes, not a second `wl_output` global - a client
/// fullscreening or querying `wl_output.enter`/scale for a specific
/// sub-region still sees it as part of the one real output. See the
/// "different monitors mode in one" plan for why that's an accepted,
/// explicitly-flagged limitation of this first version.
#[derive(Debug, Clone, Copy)]
pub struct MonitorSplit {
    pub parts: u32,
    /// `false` (the default): side-by-side columns, splitting width.
    /// `true`: stacked rows, splitting height.
    pub rows: bool,
}

/// Divides `rect` into `parts` equal (within one pixel) pieces along one
/// axis, returning piece number `index` (`0..parts`). `rows` chooses which
/// axis: stacked rows (splitting height) when `true`, side-by-side columns
/// (splitting width) when `false`.
///
/// Any remainder from an uneven division is spread one pixel at a time
/// across the first `remainder` pieces, rather than dumped entirely onto
/// the last one - so a 1919px-wide monitor split into 2 columns yields
/// 960/959, not a lopsided 959/960 vs. a naive 959/960-plus-slack-on-one-
/// side that would leave one part visibly wider for no reason tied to the
/// actual pixel count.
///
/// `index >= parts` or `parts == 0` returns `rect` unchanged - callers
/// are expected to only iterate `0..parts.max(1)`, this is just a safe
/// fallback rather than a panic for a config-driven value.
pub fn split_rect(rect: Rect, index: u32, parts: u32, rows: bool) -> Rect {
    if parts <= 1 || index >= parts {
        return rect;
    }
    let total = if rows { rect.height } else { rect.width };
    let other = if rows { rect.width } else { rect.height };
    let base = total / parts;
    let remainder = total % parts;
    let size_for = |i: u32| base + if i < remainder { 1 } else { 0 };
    let offset: u32 = (0..index).map(size_for).sum();
    let size = size_for(index);
    if rows {
        Rect::new(rect.x, rect.y + offset as i32, other, size)
    } else {
        Rect::new(rect.x + offset as i32, rect.y, size, other)
    }
}

/// The pixel density (in real, physical-size terms) srdwm treats as
/// needing no scale correction at all. `92`, close to the classic desktop
/// "96 DPI" constant - lowered from an initial `109` (roughly a 24"
/// 1920x1080 or 27" 2560x1440 monitor) after live testing on a real 1080p
/// monitor at ~78 PPI: `109` produced a `0.71` scale there, reported as
/// too aggressive a shrink; `92` produces `~0.85`, still a real reduction
/// but closer to what actually reads as "more space", not "suddenly tiny
/// text".
const REFERENCE_PPI: f64 = 92.0;

/// Automatically derives an output scale from real EDID physical size and
/// native resolution, with no monitor name or fixed size bucket involved
/// anywhere - a large panel with low pixel density (a big monitor at the
/// same resolution as a much smaller one, the concrete case this exists
/// for) gets scaled down smoothly in proportion to how far its real PPI
/// falls below [`REFERENCE_PPI`], clamped to `0.5` so a pathologically
/// large/low-res panel doesn't shrink text into illegibility. Deliberately
/// never scales *above* `1.0` on its own - a high-density panel already
/// benefits from more detail, not less, and plenty of people want native
/// crispness there; `srd.monitor.scale` remains the explicit, manual way
/// to opt into upscaling a specific connector.
///
/// `physical_mm` of `(0, 0)` (no EDID physical-size descriptor at all --
/// some VMs/adapters report this) returns `1.0` rather than guessing from
/// nothing.
pub fn auto_scale_for(physical_mm: (i32, i32), resolution_px: (i32, i32)) -> f64 {
    let (pw, ph) = physical_mm;
    if pw <= 0 || ph <= 0 {
        return 1.0;
    }
    let diagonal_mm = ((pw as f64).powi(2) + (ph as f64).powi(2)).sqrt();
    let diagonal_in = diagonal_mm / 25.4;
    let (rw, rh) = resolution_px;
    let diagonal_px = ((rw as f64).powi(2) + (rh as f64).powi(2)).sqrt();
    let ppi = diagonal_px / diagonal_in;
    if ppi >= REFERENCE_PPI {
        1.0
    } else {
        (ppi / REFERENCE_PPI).clamp(0.5, 1.0)
    }
}

#[cfg(test)]
mod auto_scale_tests {
    use super::*;

    #[test]
    fn a_15_inch_1080p_laptop_panel_needs_no_correction() {
        // 340mm x 190mm, ~143 PPI - comfortably above the reference, and
        // the concrete real-hardware case this must not regress: this
        // laptop's own panel was already correct at 1.0.
        assert_eq!(auto_scale_for((340, 190), (1920, 1080)), 1.0);
    }

    #[test]
    fn a_physically_large_1080p_monitor_scales_down() {
        // 600mm x 400mm at the same 1920x1080 as the laptop above --
        // ~78 PPI, well under the reference. The concrete case this whole
        // function exists for: reported live as "too big, should utilize
        // greater real estate" on exactly this monitor.
        let s = auto_scale_for((600, 400), (1920, 1080));
        assert!(s < 1.0 && s > 0.5, "expected a real scale-down, got {s}");
    }

    #[test]
    fn a_high_density_panel_is_never_auto_upscaled() {
        // A small, very high-resolution panel (e.g. a 13" 4K) - far above
        // the reference PPI. Must clamp at 1.0, not scale past it.
        assert_eq!(auto_scale_for((290, 170), (3840, 2160)), 1.0);
    }

    #[test]
    fn an_extreme_low_density_panel_clamps_at_half_scale() {
        let s = auto_scale_for((2000, 1200), (1024, 768));
        assert_eq!(s, 0.5);
    }

    #[test]
    fn missing_physical_size_does_not_guess() {
        assert_eq!(auto_scale_for((0, 0), (1920, 1080)), 1.0);
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    #[test]
    fn single_part_returns_the_whole_rect_unchanged() {
        let r = Rect::new(0, 0, 1920, 1080);
        assert_eq!(split_rect(r, 0, 1, false), r);
    }

    #[test]
    fn even_columns_split_width_with_no_gap_or_overlap() {
        let r = Rect::new(100, 0, 1920, 1080);
        let a = split_rect(r, 0, 2, false);
        let b = split_rect(r, 1, 2, false);
        assert_eq!(a, Rect::new(100, 0, 960, 1080));
        assert_eq!(b, Rect::new(1060, 0, 960, 1080));
        assert_eq!(a.right(), b.x, "no gap or overlap between adjacent parts");
    }

    #[test]
    fn uneven_columns_spread_the_remainder_one_pixel_at_a_time() {
        let r = Rect::new(0, 0, 1919, 1080);
        let a = split_rect(r, 0, 2, false);
        let b = split_rect(r, 1, 2, false);
        assert_eq!(a.width, 960);
        assert_eq!(b.width, 959);
        assert_eq!(a.width + b.width, r.width);
        assert_eq!(a.right(), b.x);
    }

    #[test]
    fn rows_split_height_and_leave_width_untouched() {
        let r = Rect::new(0, 50, 1920, 1080);
        let a = split_rect(r, 0, 2, true);
        let b = split_rect(r, 1, 2, true);
        assert_eq!(a, Rect::new(0, 50, 1920, 540));
        assert_eq!(b, Rect::new(0, 590, 1920, 540));
        assert_eq!(a.bottom(), b.y);
    }

    #[test]
    fn three_parts_covers_the_whole_rect_exactly() {
        let r = Rect::new(0, 0, 1000, 500);
        let parts: Vec<Rect> = (0..3).map(|i| split_rect(r, i, 3, false)).collect();
        let total_width: u32 = parts.iter().map(|p| p.width).sum();
        assert_eq!(total_width, r.width);
        for w in parts.windows(2) {
            assert_eq!(w[0].right(), w[1].x);
        }
    }

    #[test]
    fn out_of_range_index_returns_the_whole_rect_unchanged() {
        let r = Rect::new(0, 0, 1920, 1080);
        assert_eq!(split_rect(r, 5, 2, false), r);
    }
}
