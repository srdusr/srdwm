//! A window's drop shadow, rasterized as its own BGRA8 bitmap - extent,
//! sizing, and the corner-aware falloff distance function it needs to read
//! as rounded next to a window with a real rounded corner, rather than a
//! plain square-cornered glow sitting incongruously beside one.

/// How far a window's drop shadow extends past its geometry on each side.
///
/// `24`, not the original `12`: reported live as srdwm's own shadow
/// reading as a thin, tight dark line rather than the soft, generously-
/// sized glow real macOS windows have - doubled, and paired with
/// `shadow_bitmap`'s own falloff moving from a plain linear ramp to an
/// eased (`smoothstep`) one, which reads as noticeably softer at the same
/// pixel budget even without a real blur primitive to work with.
pub const SHADOW_SIZE: u32 = 24;

/// The shadow's darkest alpha, right at the window's own edge - out of
/// 255. Deliberately subtle (Nord/GNOME-default territory, not a heavy
/// drop shadow): this compositor has no blur primitive to soften it with
/// (see `shadow_bitmap`'s own doc comment), so a strong value would read as
/// a hard dark ring rather than a shadow. This is the *focused*-window
/// value - see `shadow_bitmap`'s own `max_alpha` parameter for why an
/// unfocused window doesn't just reuse it unconditionally.
pub(crate) const SHADOW_MAX_ALPHA: u8 = 90;

/// `geometry` expanded by [`SHADOW_SIZE`] on every side - the full bounding
/// box [`shadow_bitmap`] rasterises into, and where the caller positions it
/// (top-left corner at `(geometry.x - SHADOW_SIZE, geometry.y - SHADOW_SIZE)`).
pub fn shadow_rect(geometry: srdwm_core::Rect) -> srdwm_core::Rect {
    let s = SHADOW_SIZE as i32;
    srdwm_core::Rect::new(geometry.x - s, geometry.y - s, geometry.width + SHADOW_SIZE * 2, geometry.height + SHADOW_SIZE * 2)
}

/// Renders a window's drop shadow as a BGRA8 bitmap: black at an alpha that
/// falls off linearly from [`SHADOW_MAX_ALPHA`] right at the window's own
/// edge to fully transparent [`SHADOW_SIZE`] pixels out. `win_width`/
/// `win_height` are the window's own footprint (`geometry`, border strips
/// included if any - whatever the caller already draws as opaque); the
/// returned bitmap is `shadow_rect`'s size, `SHADOW_SIZE` larger on every
/// side.
///
/// Not a true Gaussian blur - no blur primitive is available without a GPU
/// shader (the udev backend's `PixmanRenderer` is software-only) or a new
/// image-processing dependency - so this is a stepless *linear* falloff
/// using Chebyshev (square-ring) distance from the window's edge rather
/// than a rounded/radial one, cheap enough to rebuild on every resize (see
/// the caller for when that is) without a per-pixel sqrt. Reads as "soft
/// enough" at the sizes a titlebar-height window actually uses, the same
/// "approximate cutoff over true anti-aliasing" trade-off `corners::round_
/// top_corners` already makes for corners.
///
/// The region directly under the window itself (`dist == 0` below) is left
/// fully transparent rather than filled - harmless either way since the
/// window's own border/titlebar/content always draws over it, but skipping
/// it is one less branch of work for the common case (a window with no
/// occluders in front of it, so most of the bitmap's interior never
/// contributes a visible pixel).
///
/// `max_alpha` - the shadow's own darkest value, right at the window's
/// edge - is a parameter rather than always `SHADOW_MAX_ALPHA`, so a
/// caller can dim an *unfocused* window's shadow the same way `theme.
/// border.inactive_dim` already dims an unfocused window's border colour
/// (see `effective_border_color`). Real desktop convention, not invented
/// here: Hyprland's own `decoration:shadow` config exposes `color` and
/// `color_inactive` as two separate values specifically for this, common
/// user configs going as far as a fully transparent `color_inactive` (no
/// shadow at all once a window loses focus) - confirmed via Hyprland's
/// own wiki, not assumed. `redraw_decoration_buffer` reuses `theme.
/// border_inactive_dim` for this rather than adding a second, separately
/// configurable factor: both are "how much does losing focus fade this
/// window's own chrome", and this codebase already has a user-tunable
/// answer to that question.
pub fn shadow_bitmap(win_width: u32, win_height: u32, radius: u32, max_alpha: u8) -> Vec<u8> {
    let (win_width, win_height) = (win_width.max(1), win_height.max(1));
    let width = win_width + SHADOW_SIZE * 2;
    let height = win_height + SHADOW_SIZE * 2;
    // Clamped the same way `corners::round_top_corners`/`round_bottom_
    // corners` clamp their own radius against the buffer they're cutting --
    // a radius that would eat more than half of either the window's own
    // width or height isn't geometrically meaningful.
    let radius = radius.min(win_width / 2).min(win_height / 2);
    let mut buf = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        let dy = edge_distance(y, SHADOW_SIZE, win_height);
        // The widest a row can still possibly contribute a visible pixel:
        // a corner-quadrant pixel's distance is `sqrt(qx^2 + qy^2) -
        // radius` where `qy = dy - radius`, and that can still be `<=
        // SHADOW_SIZE` (this function's own cutoff below) even with
        // `qx == 0`, i.e. up to `dy == SHADOW_SIZE + 2 * radius` - not
        // just `SHADOW_SIZE + radius`, which would cut off real corner
        // pixels a few rows early.
        if dy > SHADOW_SIZE + 2 * radius {
            continue;
        }
        for x in 0..width {
            let dx = edge_distance(x, SHADOW_SIZE, win_width);
            let dist = rounded_edge_distance(dx, dy, radius);
            if dist == 0 || dist > SHADOW_SIZE {
                continue;
            }
            // Eased (`smoothstep`), not a plain linear ramp - the same
            // curve `apply_corner_mask`/`fill_button_dot` already use for
            // their own anti-aliased edges, applied here across the whole
            // shadow's width instead of a 2px antialiasing band. A linear
            // falloff reads as a visible ring with a hard-ish inner edge
            // even when fully transparent at both ends; easing both ends
            // of the same 0..=1 range softens the transition into and out
            // of the shadow without needing a real blur primitive.
            let t = dist as f32 / SHADOW_SIZE as f32;
            let eased = 1.0 - (t * t * (3.0 - 2.0 * t));
            let alpha = (max_alpha as f32 * eased).round() as u8;
            if alpha == 0 {
                continue;
            }
            let i = ((y * width + x) * 4) as usize;
            // Premultiplied BGRA, but the colour is black (0, 0, 0) - a
            // premultiplied black pixel is just (0, 0, 0, alpha) at any
            // alpha, so there's no separate multiply step needed here.
            buf[i + 3] = alpha;
        }
    }
    buf
}

/// `edge_distance`'s corner-aware version: `dx`/`dy` are already `edge_
/// distance`'s own plain per-axis distances past the window's true edge
/// (`0` on either axis means "not in a corner quadrant at all" - directly
/// above/below/left/right of the window, or inside it) - this only
/// changes what happens where *both* are positive, i.e. genuinely outside
/// the window on both axes at once. Along a flat edge, a rounded rect's
/// boundary is identical to a square one's (rounding only touches the
/// corners), so the plain `max(dx, dy)` this replaces was already correct
/// there and stays correct here too.
///
/// Without this, the shadow was a plain square-cornered falloff (`shadow_
/// bitmap`'s own historical doc comment called this out as a deliberate
/// Chebyshev-not-radial simplification - reasonable for a soft blur where
/// nothing else in the frame gives the eye a hard edge to compare against,
/// but wrong once the window it belongs to has a *visibly* rounded corner
/// right next to it) - confirmed live via a real screenshot at actual
/// render resolution: the shadow's own corner cut a hard diagonal well
/// outside the window's own curve, plainly a different, unrelated shape
/// sitting right beside it.
///
/// The corner-quadrant formula: place the window's true (sharp) corner at
/// the origin, with the window occupying the quadrant behind it - `dx`/
/// `dy` are how far past that origin the shadow pixel sits on each axis. A
/// *rounded* corner's circle sits centred `radius` pixels in from that
/// origin on both axes, i.e. at `(-radius, -radius)`. Straight-line
/// distance from the pixel to that centre is `sqrt((dx + radius)^2 + (dy +
/// radius)^2)`; subtracting `radius` converts "distance to the circle's
/// centre" into "distance to the circle's own boundary", which is what a
/// real rounded corner's curve actually traces. (At `dx = dy = 0` - the
/// old sharp corner's own tip - this correctly comes out positive, not
/// `0`: the rounded window's boundary has curved away from that point
/// entirely, so it's already outside the window, not sitting right on its
/// edge the way a real square corner's tip would be.)
pub(super) fn rounded_edge_distance(dx: u32, dy: u32, radius: u32) -> u32 {
    // `radius == 0` (nothing to round - kept byte-identical to the
    // pre-existing Chebyshev-everywhere behaviour, not just "close
    // enough": true Euclidean distance to a sharp corner *point* differs
    // from Chebyshev distance to it even without any rounding, and this
    // function must not change a square window's own shadow shape) or a
    // genuine flat-edge point (`dx == 0` xor `dy == 0` - rounding never
    // touches these, only the four corners) both use plain Chebyshev
    // distance, unchanged from before this function existed.
    if radius == 0 || (dx == 0) != (dy == 0) {
        return dx.max(dy);
    }
    let (ex, ey) = (dx as f32 + radius as f32, dy as f32 + radius as f32);
    let corner_dist = (ex * ex + ey * ey).sqrt() - radius as f32;
    corner_dist.max(0.0).round() as u32
}

/// How far outside `[margin, margin + extent)` - the window's own span
/// along one axis, inside the shadow's `margin`-pixel border on each side
/// - position `pos` sits, in pixels. `0` anywhere inside that span
/// (including exactly on its edge).
fn edge_distance(pos: u32, margin: u32, extent: u32) -> u32 {
    if pos < margin {
        margin - pos
    } else if pos >= margin + extent {
        pos - (margin + extent) + 1
    } else {
        0
    }
}
