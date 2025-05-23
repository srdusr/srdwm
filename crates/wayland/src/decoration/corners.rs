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
///
/// `center_col` is the exact same idea, horizontally: which *column* of
/// this buffer's own local coordinates the left corner's circle centre
/// sits on (the right corner mirrors it, `width - r` outward from the
/// right edge by the same amount `center_col` is inward from the left).
/// A border strip's own column 0 is the *true* left edge, so it passes
/// `radius as i32`, same as its unshifted `center_row`. A titlebar's own
/// column 0 sits `border_width` columns *inside* that same true edge --
/// its buffer is only as wide as the content it sits above, not the
/// wider border strip around it - so it needs the identical `radius as
/// i32 - border_width as i32` shift horizontally too, or its own circle
/// centre ends up `border_width` columns to the right of the border
/// strip's, two different circles again despite `center_row` already
/// lining up the vertical one. Confirmed live at a real corner, zoomed:
/// the border's own curve covered most of the shared corner correctly,
/// but a `border_width`-wide sliver of the titlebar's own (wrongly
/// centred) curve poked through right where the two should have met
/// exactly, reading as a small square notch bitten out of an otherwise
/// smooth arc - reported as "squares on the inside corners of each
/// vertex/border corner." Every existing caller before this parameter
/// existed passed the unshifted, no-op case (`radius as i32`, same as
/// `center_row`'s own default), so this is additive, not a behaviour
/// change for border's own corner.
///
/// `inner`, when `Some`, also carves this corner into a proper ring - see
/// [`carve_inner_corner_pixel`]'s own doc comment for why that's needed at
/// all, and [`InnerRing`]'s own doc comment for why it needs a centre
/// that's independent of `center_row`/`center_col`, not just a smaller
/// radius at the same one. Every caller that isn't `render_border_top`/
/// `render_border_bottom` (a titlebar's own corner, the lock-screen box)
/// passes `None` and keeps today's solid-disk-past-the-nominal-edge
/// behaviour, which is correct for a single flat-coloured panel with
/// nothing of a *different* colour underneath it needing to show through.
pub(crate) fn round_top_corners(buf: &mut [u8], width: usize, height: usize, radius: u32, center_row: i32, center_col: i32, inner: Option<InnerRing>) {
    let r = (radius as usize).min(width / 2);
    if r == 0 {
        return;
    }
    let rf = r as f32;
    let cy = center_row as f32;
    // How far a given centre column sits from the unshifted default
    // (`radius`) - the right corner's own centre needs shifting by the
    // same amount, in the opposite direction (further *into* the buffer
    // from the right edge, mirroring how the left corner shifts further
    // *into* it from the left), since the buffer's own right edge is the
    // mirror image of its left one, not an independent second true edge.
    // A closure, not a one-off `let`, since the inner ring's own centre
    // (potentially different from `center_col`) needs the identical
    // mirroring, not just the outer cut's.
    let mirror_col = |col: i32| (width - r) as f32 + (radius as i32 - col) as f32;
    // `> 0`, not just `.is_some()`: a ring whose own radius would be zero
    // or negative (an unusually thick border relative to its corner
    // radius, for the titlebar-aligned case) has no ring to carve at all
    // - the whole disk out to `radius` already *is* the intended
    // thickness, and a zero/negative inner radius would carve away the
    // entire corner instead of nothing.
    let inner = inner.filter(|ring| ring.radius > 0);
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
            blend_corner_pixel(buf, width, x, y, center_col as f32, cy, rf);
            if let Some(ring) = &inner {
                carve_inner_corner_pixel(buf, width, x, y, ring.center_col as f32, ring.center_row as f32, ring.radius as f32);
            }
        }
        for x in (width - r)..width {
            // `width - r`, not `width - r - 1` - see `blend_corner_pixel`'s
            // own doc comment: that `- 1` compensated for this function not
            // sampling at the pixel centre, which it now does, so the
            // right corner's centre column lines up with `rounded_corners_
            // pixman.rs`'s `apply_corner_mask` (`px.clamp(radius, wf -
            // radius)`, which clamps to exactly `w - r` here) without it.
            let cx = mirror_col(center_col);
            blend_corner_pixel(buf, width, x, y, cx, cy, rf);
            if let Some(ring) = &inner {
                carve_inner_corner_pixel(buf, width, x, y, mirror_col(ring.center_col), ring.center_row as f32, ring.radius as f32);
            }
        }
    }
}

/// The inner ring [`round_top_corners`]/[`round_bottom_corners`] carve into
/// their own outer disk - see [`carve_inner_corner_pixel`]'s own doc
/// comment for why a ring, not a filled disk, is what a border strip's
/// corner actually needs to look like.
///
/// A *separate* centre from the outer cut's `center_row`/`center_col`, not
/// just a smaller radius at the same one - because what the inner cut
/// needs to reveal differs by what's actually drawn underneath this
/// specific strip's own "extra" rows:
///
/// - A **titlebar** band underneath (`render_border_top`, `decorated`):
///   the titlebar draws its *own* independently rounded corner, sharing
///   the exact same circle as the border's own outer cut (`center_row`/
///   `center_col`'s own doc comment on the `radius - border_width` shift
///   that lines the two buffers' circles up). The inner ring here should
///   match *that* circle exactly - same centre as the outer cut, radius
///   `radius - border_width`.
/// - **Client content** underneath (`render_border_top` when undecorated,
///   `render_border_bottom` always - there is no "bottom titlebar" in
///   this compositor's design): content's own rounded-corner mask
///   (`rounded_corners_pixman.rs`'s `apply_corner_mask`) is centred
///   `radius` from *its own* buffer's edges, and that buffer's own origin
///   sits `border_width` rows/columns inside this strip's - a genuinely
///   *different* circle from the border's own outer one, offset by
///   `(border_width, border_width)` diagonally, not just a smaller
///   concentric one. Reusing the titlebar-style "same centre, smaller
///   radius" ring here left a real, visible gap along part of the seam
///   and a thin sliver of double coverage along the rest - both curves
///   are radius-`radius` circles, but centred `border_width` apart, so no
///   single concentric ring traces both correctly. Confirmed live at
///   extreme zoom against a solid-colour wallpaper (easier to spot a
///   sub-pixel-scale gap against than the usual desktop image): a very
///   thin wedge of wallpaper visible right at the point the two circles'
///   radii diverge most. Matching content's own circle exactly --
///   `radius` unchanged, centre shifted `border_width` further into the
///   buffer on both axes - traces the *same* curve content's own mask
///   already cuts to, so the two meet with no gap and no overlap.
pub(crate) struct InnerRing {
    pub(crate) center_row: i32,
    pub(crate) center_col: i32,
    pub(crate) radius: u32,
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
    let keep = corner_keep_mask(x, y, cx, cy, radius);
    scale_pixel(buf, width, x, y, keep);
}

/// `1.0` (fully kept) within `radius - 1` of `(cx, cy)`, `0.0` (fully cut)
/// beyond `radius + 1`, smoothstepped between - the shared falloff both
/// [`blend_corner_pixel`] (an *outer* cut: keep near the centre, cut far
/// from it) and [`carve_inner_corner_pixel`] (an *inner* cut: the same
/// falloff, inverted, so it cuts *near* the centre instead) are built from.
/// Sampled at the pixel's own *center* (`+ 0.5`), not its raw integer
/// coordinate - matching `rounded_corners_pixman.rs`'s `apply_corner_mask`
/// (`px = x as f32 + 0.5`) and `rounded_corners.rs`'s GLES shader, both of
/// which already use this standard rasterization convention. This function
/// didn't, once - a half-pixel systematic difference between this
/// border-strip curve and the client-content curve it's supposed to trace
/// exactly the same circle as, confirmed live at extreme zoom: a small but
/// real right-angle step partway along an otherwise smooth arc, right
/// where the two curves are supposed to meet.
fn corner_keep_mask(x: usize, y: usize, cx: f32, cy: f32, radius: f32) -> f32 {
    let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
    let dist = (dx * dx + dy * dy).sqrt();
    let t = ((dist - (radius - 1.0)) / 2.0).clamp(0.0, 1.0);
    1.0 - (t * t * (3.0 - 2.0 * t))
}

/// Multiplies every BGRA byte of the pixel at `(x, y)` by `mask` - `buf` is
/// premultiplied BGRA (`color::rgb_to_bgra`'s own convention), so scaling
/// all four bytes by the same factor is the correct way to reduce a
/// pixel's effective alpha (zeroing all four, not just alpha, for a fully
/// cut pixel - a genuinely transparent premultiplied pixel is `(0, 0, 0,
/// 0)` in every channel, not just alpha, since the stored colour already
/// carries the alpha multiplied in; leaving stale opaque RGB behind while
/// zeroing only alpha produced a byte pattern Pixman's own `OVER`
/// compositing does not actually treat as "nothing here" - confirmed
/// live, pixel-by-pixel, no visible transparency despite alpha already
/// being zero).
fn scale_pixel(buf: &mut [u8], width: usize, x: usize, y: usize, mask: f32) {
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

/// The border strip's own missing half of a proper rounded-corner *ring*:
/// [`blend_corner_pixel`] already cuts everything *outside* `radius` of the
/// shared corner centre (the true rounded silhouette), but nothing used to
/// cut anything *inside* it - so the strip's own "extra" rows (past its
/// nominal `border_width`, present whenever `corner_radius > border_width`
/// - see `render_border_top`'s own doc comment) stayed a solid *filled*
/// quarter-disk out to the centre column/row, then hit `clip_middle_
/// beyond_thickness`'s hard, unblended rectangular cut at exactly column/
/// row `radius` - which is essentially the disk's own *most opaque*
/// point (dead centre, mask ~1.0), not somewhere the curve had already
/// faded out. The result: a solid wedge of border colour with two straight
/// inner edges meeting the titlebar/content at a right angle, not a
/// uniform-width curved ring - confirmed live, zoomed: a clean rectangular
/// step, not a blend, reported as "squares on the inside corners."
///
/// The fix: cut *this* pixel wherever it falls within `border_width` of
/// the *same* shared centre `blend_corner_pixel` already cut around --
/// i.e. within `radius - border_width` of it, same smoothstep falloff,
/// inverted. Combined with the existing outer cut, the strip's corner
/// becomes a genuine ring of ~`border_width` visible thickness tapering
/// smoothly to nothing by the point `clip_middle_beyond_thickness`'s own
/// (already-transparent-by-then) hard cut takes over, instead of jumping
/// from opaque to transparent in one pixel. Only ever called with `radius
/// > border_width` (`round_top_corners`/`round_bottom_corners` skip it
/// otherwise, since there is no ring to speak of - see their own call
/// sites) - `inner_radius` would otherwise be zero or negative, cutting
/// the entire disk including the visible outer sliver that's supposed to
/// remain `border_width` px thick.
fn carve_inner_corner_pixel(buf: &mut [u8], width: usize, x: usize, y: usize, cx: f32, cy: f32, inner_radius: f32) {
    let keep = corner_keep_mask(x, y, cx, cy, inner_radius);
    scale_pixel(buf, width, x, y, 1.0 - keep);
}

/// [`round_top_corners`]'s mirror for the bottom two corners - same
/// construction, corner centres `r` *up* from the bottom instead of down
/// from the top. Same anti-aliasing, same reason - see
/// [`blend_corner_pixel`]'s own doc comment. `inner` is the same idea as
/// [`round_top_corners`]' own parameter of the same name - see
/// [`InnerRing`]'s own doc comment. Unlike `round_top_corners`, this
/// function's own outer cut has no `center_row`/`center_col` of its own to
/// default the ring's centre from (nothing has ever needed to shift the
/// *outer* cut here - only `render_border_top`'s own top strip sits above
/// a titlebar that needs one), so `InnerRing`'s fields are the ring's own
/// absolute centre, not an offset from anything.
pub(crate) fn round_bottom_corners(buf: &mut [u8], width: usize, height: usize, radius: u32, inner: Option<InnerRing>) {
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
    // takes float centres, so `height - r` going negative when `r >
    // height` (the strip-thinner-than-radius case above) is just a
    // negative `f32`, no special-casing required. Plain `height - r`, not
    // `height - r - 1` - see `blend_corner_pixel`'s own doc comment: that
    // `- 1` compensated for this function not sampling at the pixel
    // centre, which it now does, so this lines up with `rounded_corners_
    // pixman.rs`'s own bottom-box centre (`py.clamp(radius, hf - radius)`,
    // which clamps to exactly `h - r`) without it.
    let cy = height as f32 - rf;
    // See `round_top_corners`'s own `mirror_col` for why this needs to be a
    // closure, not a one-off `let`.
    let mirror_col = |col: i32| (width - r) as f32 + (radius as i32 - col) as f32;
    // See `round_top_corners`'s matching line for why this is `.filter(|ring|
    // ring.radius > 0)`, not just `.is_some()`.
    let inner = inner.filter(|ring| ring.radius > 0);
    for y in (height - rows)..height {
        for x in 0..r {
            blend_corner_pixel(buf, width, x, y, rf, cy, rf);
            if let Some(ring) = &inner {
                carve_inner_corner_pixel(buf, width, x, y, ring.center_col as f32, ring.center_row as f32, ring.radius as f32);
            }
        }
        for x in (width - r)..width {
            // See `round_top_corners`'s matching comment for why this is
            // `width - r`, not `width - r - 1`.
            let cx = (width - r) as f32;
            blend_corner_pixel(buf, width, x, y, cx, cy, rf);
            if let Some(ring) = &inner {
                carve_inner_corner_pixel(buf, width, x, y, mirror_col(ring.center_col), ring.center_row as f32, ring.radius as f32);
            }
        }
    }
}
