//! The four solid-colour strips around a decorated window's own `geometry`
//! - top/bottom rendered as small rounded bitmaps (their own two outer
//! corners cut to match `titlebar::render_titlebar`'s), left/right left to
//! the caller as plain flat fills (`elements::border_side_render_element`).

use super::color::rgb_to_bgra;
use super::corners::{round_bottom_corners, round_top_corners};

pub fn border_strips(geometry: srdwm_core::Rect, width: u32) -> [srdwm_core::Rect; 4] {
    let w = width as i32;
    [
        srdwm_core::Rect::new(geometry.x - w, geometry.y - w, geometry.width + 2 * width, width),
        srdwm_core::Rect::new(geometry.x - w, geometry.y + geometry.height as i32, geometry.width + 2 * width, width),
        srdwm_core::Rect::new(geometry.x - w, geometry.y, width, geometry.height),
        srdwm_core::Rect::new(geometry.x + geometry.width as i32, geometry.y, width, geometry.height),
    ]
}

/// Renders the top border strip (`border_strips`'s first rect) as a BGRA8
/// bitmap instead of a plain solid fill, with its own outer top corners cut
/// the same way `titlebar::render_titlebar`'s `round_corners` cuts the
/// titlebar's - see that parameter's doc comment for why a titlebar rounds
/// but a square border frame around it used to defeat the point. This
/// strip's own row 0 sits at the *true* top of the combined titlebar-plus-
/// border shape (the border is the outermost layer), so it shares the
/// titlebar's exact same circle - same `radius`, unshifted `center_row` --
/// rather than a same-centre-different-radius circle of its own; see
/// `corners::round_top_corners`'s own doc comment for why an earlier
/// version of this (`radius + thickness` as the radius passed there) drew a
/// visibly different curve that didn't actually meet the titlebar's at the
/// seam between them, and `titlebar::render_titlebar`'s call site for the
/// other half of this pair.
///
/// [`render_border_bottom`] gives the bottom strip the matching treatment
/// for its own two corners.
///
/// The buffer is `thickness.max(radius)` rows tall, not always exactly
/// `thickness` - when `radius > thickness` (true even at this codebase's
/// own theme defaults, radius 6 over a 4px border), the corner's curve
/// doesn't finish resolving to flat within just `thickness` rows, and the
/// left/right strips (`border_side_render_element`, plain flat rectangles
/// with no curve awareness of their own, starting immediately at this
/// strip's own bottom edge) have no way to cover the rest of it - the
/// result was a real, visible wedge of bare background between the
/// straight border segments and the window's own rounded silhouette,
/// worse the larger the radius/thickness gap, confirmed live via pixel-
/// level inspection of a real screenshot (not just reasoned about): the
/// horizontal top segment only became visible some ~20 columns in from the
/// corner while the vertical side segment started almost immediately,
/// exactly the asymmetry a too-short top strip and a curve-blind side
/// strip produce together. Extending this buffer to the *full* radius
/// gives the curve enough room to finish, and - since this element draws
/// on top of the side strips (`render_udev_frame` pushes it first, and
/// earlier-pushed custom elements composite over later ones) - the extra
/// rows correctly overpaint the side strip's naive square corner with the
/// real curve, no changes needed to the side strips or their occlusion-
/// fragment splitting at all.
///
/// Rows past the original `thickness` are real *extra* canvas purely so
/// the corner columns have room to curve - the middle (non-corner)
/// columns of those rows sit visually inside where the titlebar/content
/// begins, not the border ring, so they're forced transparent after
/// rounding rather than left as solid `color` (which would otherwise paint
/// a solid border-coloured bar over the titlebar's own left/right edges
/// for however many rows this extended by).
pub fn render_border_top(width: u32, thickness: u32, color: (u8, u8, u8), radius: u32) -> Vec<u8> {
    let (width, thickness) = (width.max(1) as usize, thickness.max(1) as usize);
    let height = thickness.max(radius as usize).max(1);
    let bg = rgb_to_bgra(color, 255);
    let mut buf = vec![0u8; width * height * 4];
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }
    round_top_corners(&mut buf, width, height, radius, radius as i32);
    clip_middle_beyond_thickness(&mut buf, width, radius as usize, thickness..height);
    buf
}

/// Zeroes the non-corner (middle) columns of every row in `rows` - the
/// "extra" rows `render_border_top`/`render_border_bottom` add past their
/// own true `thickness` purely to give a corner's curve room to resolve.
/// Left untouched, those rows would stay solid `color` outside the two
/// corner column-ranges (nothing else clips them), painting a border-
/// coloured bar across whatever the titlebar/content actually owns there.
/// `radius` here is the corner column width on each side (already clamped
/// to `width / 2` inside `corners::round_top_corners`/`round_bottom_
/// corners`, so this re-derives the same clamp rather than trusting the
/// caller's raw value).
fn clip_middle_beyond_thickness(buf: &mut [u8], width: usize, radius: usize, rows: std::ops::Range<usize>) {
    let r = radius.min(width / 2);
    if r * 2 >= width {
        return;
    }
    for y in rows {
        let row = y * width * 4;
        buf[row + r * 4..row + (width - r) * 4].fill(0);
    }
}

/// [`render_border_top`]'s mirror for the bottom strip - same construction,
/// its own two corners (bottom-left/bottom-right) cut instead. Reported
/// live, alongside the top-corner work: a bordered window's bottom two
/// corners still read as square next to the now-rounded top ones, the same
/// "inconsistently square" complaint that motivated rounding the top strip
/// in the first place.
///
/// Handled as one all-or-nothing bitmap rather than folded into the
/// left/right strips' per-fragment occlusion splitting (`visible_border_
/// fragments`) - the same trade-off `render_border_top`'s own call site
/// already makes and for the same reason: cropping a rounded bitmap's
/// source rect per fragment is real extra work for a strip this thin.
pub fn render_border_bottom(width: u32, thickness: u32, color: (u8, u8, u8), radius: u32) -> Vec<u8> {
    let (width, thickness) = (width.max(1) as usize, thickness.max(1) as usize);
    let height = thickness.max(radius as usize).max(1);
    let bg = rgb_to_bgra(color, 255);
    let mut buf = vec![0u8; width * height * 4];
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }
    // Plain `radius`, not `radius + thickness` (what this line passed
    // before) - that drew the corner against a *larger*, self-invented
    // circle than the window's own `corner_radius`, the exact same wrong
    // shape the top strip's own seam fix already rejected for an
    // equivalent reason (see `corners::round_top_corners`'s doc comment):
    // sampling only the near-flat tip of an oversized circle produces a
    // curve that barely bends at all, not one that matches the window's
    // real corner. `render_border_top` gets `radius` used unshifted here
    // for the same reason it does: this buffer's own outermost row is
    // genuinely the true tip of the shape.
    round_bottom_corners(&mut buf, width, height, radius);
    // Extra rows sit above the original `thickness`, not below - the
    // bottom strip's curve resolves going *up* into content, the mirror of
    // the top strip's resolving *down* into it. See
    // `clip_middle_beyond_thickness`'s own doc comment for why this needs
    // to happen at all.
    clip_middle_beyond_thickness(&mut buf, width, radius as usize, 0..height - thickness);
    buf
}

/// Which rows of `render_border_top`'s own buffer a real call site should
/// actually draw, and how far below the strip's nominal position to start
/// (`(start_row, row_count, position_shift_down)`) - pulled out as a
/// plain, backend-independent function so both `udev/render.rs` and
/// `winit/render.rs` share one tested answer instead of two hand-written
/// copies that could quietly drift apart.
///
/// `decorated` matters because the buffer's own "extra" rows (past
/// `border_width`, present whenever `corner_radius > border_width`) are
/// colour-filled at the two corner columns by design - safe to draw only
/// because a *decorated* window has a titlebar band directly beneath this
/// strip to receive them (see `render_border_top`'s own doc comment). An
/// undecorated (CSD) window has no such band: `frame.y` is the top of the
/// client's own real content, immediately below the nominal `border_width`
/// rows, so those same extra rows would paint a border-coloured wedge onto
/// it instead - confirmed live on a real Firefox window, not assumed.
/// Cropping to just the nominal rows in that case is the fix; the buffer's
/// own row 0 already sits at the correct position either way (this strip
/// grows *downward*, unlike its bottom sibling), so the shift is always 0.
pub(crate) fn border_top_visible_rows(decorated: bool, border_width: u32, corner_radius: u32) -> (u32, u32, u32) {
    let border_width = border_width.max(1);
    if decorated {
        (0, border_width.max(corner_radius), 0)
    } else {
        (0, border_width, 0)
    }
}

/// [`border_top_visible_rows`]'s mirror for `render_border_bottom`'s own
/// buffer, whose extra rows sit *above* the nominal ones (growing upward
/// into content) rather than below - so the nominal, safe-to-draw-
/// unconditionally rows are the buffer's *last* `border_width` of them,
/// starting at `start_row = extra`, and skipping them entirely (undecorated
/// case) also means skipping the compensating downward-shift-into-content
/// a decorated window's fuller buffer needs, landing back at this strip's
/// own unshifted nominal position instead.
pub(crate) fn border_bottom_visible_rows(decorated: bool, border_width: u32, corner_radius: u32) -> (u32, u32, u32) {
    let border_width = border_width.max(1);
    let extra = border_width.max(corner_radius) - border_width;
    if decorated {
        (0, border_width.max(corner_radius), extra)
    } else {
        (extra, border_width, 0)
    }
}
