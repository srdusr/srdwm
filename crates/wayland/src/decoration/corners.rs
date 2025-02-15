//! Rounding a rasterized titlebar/border bitmap's own top or bottom
//! corners to a quarter-circle, by fading the pixels outside it to fully
//! transparent - the CPU-bitmap equivalent of `rounded_corners.rs`'s GLES
//! fragment shader, for the software-only udev/Pixman render path. Shared
//! by `titlebar.rs` and `border.rs`, which both cut corners out of their
//! own otherwise-independent buffers and need the two curves to agree
//! exactly where they meet.

/// Clips the top-left and top-right corners of a titlebar buffer to a
/// quarter-circle by making the pixels outside it fully transparent, so
/// whatever's behind (the desktop, on every top-level window) shows through
/// instead of a hard square corner.
///
/// Only the *top* corners: the titlebar's bottom edge meets the window's
/// content, which this compositor has no way to clip (content is rendered
/// entirely by the client) - rounding that seam too would need a
/// compositor-wide clip mask over arbitrary client buffers, a much larger
/// change than this cosmetic pass. Real desktops mostly round this the same
/// way: only the outermost corners of a window, not every internal seam.
///
/// Hard cutoff rather than an anti-aliased edge, matching this codebase's
/// existing pixel-art aesthetic elsewhere (the cursor bitmaps) rather than
/// mixing rendering styles for one corner treatment.
///
/// Zeroes all four BGRA bytes for a cut pixel, not just alpha: this buffer
/// is `Fourcc::Argb8888`, which both Wayland/`wl_shm` and Pixman treat as
/// premultiplied - a genuinely transparent premultiplied pixel is `(0, 0,
/// 0, 0)` in every channel, not just alpha, since the stored colour already
/// carries the alpha multiplied in. Leaving the opaque titlebar-background
/// RGB behind while zeroing only alpha produced a byte pattern Pixman's own
/// `OVER` compositing (`result = src + dst * (1 - src_alpha)`) does not
/// actually treat as "nothing here": with `src_alpha = 0` the formula still
/// adds the stale, un-premultiplied `src` RGB straight through, so the
/// "cut" pixel came out opaque and the corner still read as square --
/// confirmed live, pixel-by-pixel, no visible transparency anywhere in a
/// window's real top corner despite this function running and a nonzero
/// radius. `rounded_corners_pixman.rs`'s `apply_corner_mask` - the
/// equivalent mask for client *content* - already gets this right (scales
/// all four bytes together); this was the one corner-rounding path in the
/// codebase that didn't match it.
/// `center_row` is which row of *this* buffer's own local coordinates the
/// corner circle's centre sits on - not always `radius` itself. A plain
/// titlebar with nothing above it passes `radius as i32` (the ordinary
/// case: the circle's top tip is this buffer's own row 0, same as this
/// function always assumed before `center_row` existed). A border-top
/// strip sitting `thickness` rows *above* the titlebar it visually
/// continues into needs the *same* radius and the *same* circle - not a
/// same-centre-different-radius circle of its own, which is what passing
/// `radius + thickness` here used to do (see the doc comment on
/// `render_border_top`'s call site for why that was tried first). Two
/// concentric circles of different radii do not meet smoothly at any
/// boundary between them: at the exact seam, one buffer's mask is
/// computed against one radius and the other buffer's mask is computed
/// one pixel later against a different radius, producing a visible jump
/// rather than a continuous curve - confirmed live, screenshotted at
/// actual render resolution, not just reasoned about: the titlebar-to-
/// border seam showed a hard stepped notch, not a curve. Since a border
/// strip's own row 0 already sits at the *true* top of the combined
/// shape, it passes `radius as i32` too (unshifted) - it's the titlebar,
/// starting `thickness` rows *into* the circle instead of at its top,
/// that needs to shift, by passing `radius as i32 - border_width as
/// i32` (see `render_titlebar`'s call site).
pub(crate) fn round_top_corners(buf: &mut [u8], width: usize, height: usize, radius: u32, center_row: i32) {
    let r = (radius as usize).min(width / 2);
    if r == 0 {
        return;
    }
    let rf = r as f32;
    let cy = center_row as f32;
    // Only rows that could plausibly need blending at all: below
    // `center_row` (this buffer's slice of the circle, whatever portion
    // of it falls within `[0, height)`) is where the actual curve lives;
    // rows above `center_row - r` or at/below `center_row` are either
    // already past the transparent tip or already fully inside the
    // shape, and calling `blend_corner_pixel` there would either be a
    // wasted no-op (large `dist`, `mask >= 1`) or - critically, for a
    // *tall* buffer whose straight edge extends far past the corner --
    // wrongly compute a huge `dist` from being far below the centre and
    // clip an ordinary straight-edge pixel to transparent. The original
    // unshifted version of this function avoided that the same way, by
    // simply never iterating past row `r`; this is that same bound,
    // generalised to an arbitrary `center_row`.
    let y_lo = (center_row - r as i32).max(0) as usize;
    let y_hi = (center_row.max(0) as usize).min(height);
    for y in y_lo..y_hi {
        for x in 0..r {
            blend_corner_pixel(buf, width, x, y, rf, cy, rf);
        }
        for x in (width - r)..width {
            blend_corner_pixel(buf, width, x, y, (width - r - 1) as f32, cy, rf);
        }
    }
}

/// Multiplies the pixel at `(x, y)` by a smoothed 0..1 mask based on its
/// distance from `(cx, cy)` versus `radius` - `1` (unchanged) well inside
/// the circle, `0` (fully transparent) well outside it, blended over a ~2px
/// band at the boundary. Same anti-aliasing technique `rounded_corners.rs`'s
/// GLES fragment shader already uses for content rounding
/// (`smoothstep(radius - 1.0, radius + 1.0, dist)`), applied here to a CPU
/// bitmap pixel by pixel instead of a per-fragment shader.
///
/// The previous version of both callers did a hard binary cut instead --
/// fully opaque or fully transparent, nothing between - which read as a
/// jagged single-pixel "break" in the border line rather than a curve,
/// especially in a border strip only a couple of rows tall (the common
/// case: `border_width` is usually 2-3px) where there's no room for the
/// eye to average a staircase into something that looks round. Reported
/// live as "line breaks" right where a window's border met its curved
/// corner.
///
/// `buf` is premultiplied BGRA (`color::rgb_to_bgra`'s own convention), so
/// scaling all four bytes by the same factor is the correct way to reduce a
/// pixel's effective alpha - same reasoning
/// `clipped_corner_pixels_are_fully_premultiplied_zero_not_just_alpha`
/// already established for the hard-cut case this replaces.
fn blend_corner_pixel(buf: &mut [u8], width: usize, x: usize, y: usize, cx: f32, cy: f32, radius: f32) {
    let (dx, dy) = (x as f32 - cx, y as f32 - cy);
    let dist = (dx * dx + dy * dy).sqrt();
    let t = ((dist - (radius - 1.0)) / 2.0).clamp(0.0, 1.0);
    let mask = 1.0 - (t * t * (3.0 - 2.0 * t));
    if mask >= 1.0 {
        return;
    }
    let idx = (y * width + x) * 4;
    if mask <= 0.0 {
        buf[idx..idx + 4].fill(0);
        return;
    }
    for c in &mut buf[idx..idx + 4] {
        *c = (*c as f32 * mask).round() as u8;
    }
}

/// [`round_top_corners`]'s mirror for the bottom two corners - same
/// construction, corner centres `r` *up* from the bottom instead of down
/// from the top. Same anti-aliasing, same reason - see
/// [`blend_corner_pixel`]'s own doc comment.
pub(crate) fn round_bottom_corners(buf: &mut [u8], width: usize, height: usize, radius: u32) {
    let r = (radius as usize).min(width / 2);
    if r == 0 {
        return;
    }
    // See `round_top_corners`' matching comment: `r` (the real corner
    // radius) must stay unclamped by `height`, or a strip thinner than the
    // radius cuts its own separate, too-tight arc instead of continuing the
    // titlebar's. `rows` is just how many of that circle's rows this
    // buffer actually has room for.
    let rows = r.min(height);
    let rf = r as f32;
    // As a float, not the signed-`i64`-offset trick the hard-cut version
    // needed to avoid a `usize` underflow - `blend_corner_pixel` already
    // takes float centres, so `height - r - 1` going negative when `r >
    // height` (the strip-thinner-than-radius case above) is just a
    // negative `f32`, no special-casing required.
    let cy = height as f32 - rf - 1.0;
    for y in (height - rows)..height {
        for x in 0..r {
            blend_corner_pixel(buf, width, x, y, rf, cy, rf);
        }
        for x in (width - r)..width {
            blend_corner_pixel(buf, width, x, y, (width - r - 1) as f32, cy, rf);
        }
    }
}
