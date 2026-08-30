//! The three titlebar buttons' own dots and glyphs - traffic-light fill,
//! glossy shading, and the four hand-drawn glyph shapes (X / dash / square
//! / zoom-arrows). `titlebar.rs` owns *laying out* the cluster (which
//! button goes where, which side, how many); everything here just draws
//! one button once given its own box.

use super::color::rgb_to_bgra;

/// Titlebar buttons are laid out right-aligned in `srdwm_core::BUTTON_PITCH`-
/// wide squares, vertically centred inside the taller `height` band --
/// matching `ResizeEdge::hit_test` in `crates/core/src/window.rs`, whose
/// `BUTTON` constant is also `BUTTON_PITCH`, not `TITLEBAR_HEIGHT` (see that
/// constant's own doc comment for why the two were split apart). That
/// function only computes *where* a click on close/maximize/minimize lands;
/// nothing painted the buttons themselves, so the whole band was one
/// undifferentiated bar with no visible way to tell where those three
/// clickable regions were.
pub(super) const BUTTON_MARGIN: f32 = 0.32;
/// Smaller margin used only when `buttons_left` is set - explicitly
/// requested ("bigger" buttons on the left, matching macOS convention).
/// The button's own *box* stays the same size as the right-aligned case
/// (see `ResizeEdge::hit_test`'s matching comment on why growing the box
/// itself would clip against its own pitch); a smaller margin just lets
/// the dot fill more of that same box. `0.1667` specifically: a button
/// diameter is `BUTTON_PITCH * (1 - 2 * margin)`, which at `BUTTON_PITCH
/// = 24` gives a 16px dot - measured directly against a real, live
/// Firefox window (column-scanned screenshot, edges at 40% luminance
/// difference from the titlebar background): dot diameter 16px, centre-
/// to-centre pitch 24px, at this same system's own scale. A previous
/// `0.25` (12px dot) undershot this - it came from an estimate against a
/// downloaded reference screenshot rather than a live, same-scale
/// measurement.
pub(super) const BUTTON_MARGIN_LEFT: f32 = 0.1667;
/// How long the titlebar-button glyph-reveal-on-hover animation takes to
/// reach full opacity - matches real, extracted libadwaita CSS on this
/// machine almost exactly (`transition: ... 200ms cubic-bezier(...)` on
/// `windowcontrols > button > image`, found via `gresource extract` on the
/// installed `.so`, not guessed), even though this project's own default
/// mode (`ThemeConfig::button_glyph_always`) animates the glyph itself in
/// rather than Adwaita's own choice of animating the background circle
/// with the glyph always shown - the *timing* still carries over as the
/// one piece of real DE precedent either mode can share.
pub(crate) const HOVER_GLYPH_DURATION: std::time::Duration = std::time::Duration::from_millis(200);

/// Traffic-light button colours (close/minimize/maximize), matching macOS's
/// own - and, on this machine, matching what Firefox's own CSD already
/// renders via the WhiteSur GTK theme (confirmed live via `grim`: an
/// unfocused Firefox window shows the same flat grey dots
/// `TRAFFIC_LIGHT_INACTIVE` below produces). srdwm's own SSD titlebar used
/// to draw a plain outline glyph (X/square/dash) in the ordinary text
/// colour instead - reported live as looking nothing like the traffic-
/// light buttons every CSD client on this theme already has, and as
/// visibly different window furniture between, e.g., Firefox (CSD, real
/// traffic lights) and a terminal (SSD, srdwm's own outline glyphs) side by
/// side. These are deliberately plain colour constants, not new theme
/// fields - the accepted, still-open ask is hover-state glyph/highlight
/// work on top of this base look (see `docs/TODO.md`), not a configurable
/// palette for it.
pub(super) const TRAFFIC_LIGHT_CLOSE: (u8, u8, u8) = (0xff, 0x5f, 0x57);
pub(super) const TRAFFIC_LIGHT_MINIMIZE: (u8, u8, u8) = (0xff, 0xbd, 0x2e);
pub(super) const TRAFFIC_LIGHT_MAXIMIZE: (u8, u8, u8) = (0x28, 0xc8, 0x40);
/// Unfocused state for all three buttons - real macOS (and this WhiteSur
/// theme) dims every traffic light to the same flat grey when its window
/// isn't active, rather than keeping the colours at reduced opacity.
pub(super) const TRAFFIC_LIGHT_INACTIVE: (u8, u8, u8) = (0x6e, 0x6e, 0x6e);

/// The `srdwm_core::BUTTON_PITCH`-square box a button's dot is drawn inside,
/// `offset` pixels in from whichever edge `from_left` selects and centred
/// vertically inside the taller `height` titlebar band - the box's own
/// size is the same regardless of side (see `ResizeEdge::hit_test`'s
/// matching comment on why a *bigger box* on the left would risk the dot
/// clipping against its own pitch; only the margin, and so the dot within
/// the same box, actually grows there - `titlebar::render_titlebar`'s own
/// call site picks `BUTTON_MARGIN`/`BUTTON_MARGIN_LEFT` accordingly).
pub(super) fn button_box(width: usize, height: usize, offset: usize, from_left: bool, margin: f32) -> (i32, i32, i32, i32) {
    let square = srdwm_core::BUTTON_PITCH as i32;
    let inset = (square as f32 * margin).round() as i32;
    let top = ((height as i32 - square) / 2).max(0);
    let (left, right) = if from_left { (offset as i32, offset as i32 + square) } else { (width as i32 - offset as i32 - square, width as i32 - offset as i32) };
    (left + inset, top + inset, right - inset, top + square - inset)
}

/// Fills a traffic-light dot centred in its button square, anti-aliased the
/// same `smoothstep` way `corners::blend_corner_pixel` rounds a window's
/// own corners - a hard-edged circle at this size (typically well under
/// `TITLEBAR_HEIGHT`, i.e. a ~20px-diameter dot) read as visibly jagged,
/// the same class of problem the corner-seam fix already solved for a
/// bigger radius. Unlike that function (which reduces an existing pixel's
/// alpha to clip it away), this blends *toward* `color` over whatever's
/// already in `buf` - the titlebar background, always already opaque here
/// - so the result stays fully opaque at every edge pixel rather than
///   letting the background show through a soft ring.
pub(super) fn fill_button_dot(buf: &mut [u8], width: usize, height: usize, offset: usize, from_left: bool, margin: f32, color: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = button_box(width, height, offset, from_left, margin);
    let cx = (x0 + x1) as f32 / 2.0;
    let cy = (y0 + y1) as f32 / 2.0;
    let radius = ((x1 - x0).min(y1 - y0) as f32 / 2.0).max(0.0);
    let span = radius.ceil() as i32 + 2;
    for y in (cy.round() as i32 - span)..=(cy.round() as i32 + span) {
        if y < 0 || y as usize >= height {
            continue;
        }
        for x in (cx.round() as i32 - span)..=(cx.round() as i32 + span) {
            if x < 0 || x as usize >= width {
                continue;
            }
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let dist = (dx * dx + dy * dy).sqrt();
            let t = ((dist - (radius - 1.0)) / 2.0).clamp(0.0, 1.0);
            let coverage = 1.0 - (t * t * (3.0 - 2.0 * t));
            if coverage <= 0.0 {
                continue;
            }
            // Shaded per-pixel, not once for the whole dot - see
            // `glossy_shade`'s own doc comment for why a flat fill read as
            // noticeably flatter than real macOS's own traffic lights.
            let target = rgb_to_bgra(glossy_shade(color, dx, dy, radius.max(1.0)), 255);
            let idx = (y as usize * width + x as usize) * 4;
            if coverage >= 1.0 {
                buf[idx..idx + 4].copy_from_slice(&target);
                continue;
            }
            for c in 0..3 {
                let existing = buf[idx + c] as f32;
                buf[idx + c] = (existing + (target[c] as f32 - existing) * coverage).round() as u8;
            }
            buf[idx + 3] = 255;
        }
    }
}

/// Shades a flat traffic-light colour into the soft glossy-sphere look real
/// macOS buttons have, referenced directly against a real screenshot
/// (Finder's own traffic lights, `~/Downloads`) rather than guessed at: a
/// gentle highlight toward the upper-left, where its own light source
/// sits, fading through the flat colour and into a touch of shadow toward
/// the lower-right rim. Deliberately restrained on both ends - the lit
/// side never blows out to white and the shadowed side never drops to a
/// hard black ring - so every dot still reads as its own colour at a
/// glance, just with real dimensionality instead of a flat fill. `(dx,
/// dy)` is the pixel's own offset from the dot's centre, in the same units
/// as `radius`, so this has no dependency on the caller's coordinate
/// system beyond that.
fn glossy_shade(color: (u8, u8, u8), dx: f32, dy: f32, radius: f32) -> (u8, u8, u8) {
    let (nx, ny) = (dx / radius, dy / radius);
    // Light source up and to the left - the same convention every real
    // desktop's own icon/button shading already uses.
    const LIGHT: (f32, f32) = (-0.55, -0.7);
    const LIGHT_LEN: f32 = 0.888_819_44; // sqrt(0.55^2 + 0.7^2), precomputed
    let facing = (nx * LIGHT.0 + ny * LIGHT.1) / LIGHT_LEN;
    // A glossy sphere isn't uniformly lit even on its bright side - it
    // dims gradually toward every edge, not just the shadowed one.
    let rim = (nx * nx + ny * ny).min(1.0);
    let highlight = facing.max(0.0) * (1.0 - rim * 0.4);
    let shadow = (-facing).max(0.0) * 0.5 + rim * 0.15;
    let mix_toward = |c: u8, target: f32, amount: f32| (c as f32 + (target - c as f32) * amount).clamp(0.0, 255.0) as u8;
    let lit = (mix_toward(color.0, 255.0, highlight * 0.45), mix_toward(color.1, 255.0, highlight * 0.45), mix_toward(color.2, 255.0, highlight * 0.45));
    (mix_toward(lit.0, 0.0, shadow * 0.35), mix_toward(lit.1, 0.0, shadow * 0.35), mix_toward(lit.2, 0.0, shadow * 0.35))
}

/// A semi-opaque colour blended over whatever's already at `(x, y)`, scaled
/// by both `alpha` (the glyph-reveal animation's own current progress --
/// see `tick_hover_glyph_animation`, or a flat 255 in `glyph_always` mode)
/// and `coverage` (this pixel's own distance-based antialiasing weight from
/// `blend_glyph_line`, 0..=1). `alpha == 0` is a plain no-op, so a not-yet-
/// hovered button pays nothing for a glyph nobody can see yet, not even a
/// fully-transparent draw call. `shade` is the colour blended toward --
/// near-black for a glyph drawn on a traffic light's own bright fill, or
/// the titlebar's real foreground colour for one drawn straight on the
/// titlebar background instead (see `titlebar::render_titlebar`'s own
/// `glyph_shade` local for which, and why).
#[allow(clippy::too_many_arguments)]
fn blend_glyph_px(buf: &mut [u8], width: usize, height: usize, x: i32, y: i32, alpha: u8, coverage: f32, shade: (u8, u8, u8)) {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= height || alpha == 0 || coverage <= 0.0 {
        return;
    }
    let idx = (y as usize * width + x as usize) * 4;
    let a = (alpha as f32 / 255.0) * coverage.min(1.0);
    for (c, target) in [shade.0, shade.1, shade.2].into_iter().enumerate() {
        let existing = buf[idx + c] as f32;
        buf[idx + c] = (existing + (target as f32 - existing) * a).round().clamp(0.0, 255.0) as u8;
    }
}

/// Half the glyph stroke's own width, in pixels, before the antialiased
/// feather outside it - `0.55` reads as a crisp, thin stroke at this dot's
/// own ~16px scale, matching how thin a real toolkit-rendered traffic-light
/// glyph actually is. Reported live (a real, live screenshot, not the
/// downloaded macOS reference this session started from) as visibly too
/// bold at the previous `1.0` - a real glyph is a hairline, not a stroke
/// that reads as almost as thick as the dot's own edge AA.
const GLYPH_HALF_WIDTH: f32 = 0.55;

/// A line segment with a soft, antialiased stroke - a raw Bresenham 1px
/// line (the original implementation) has hard-stepped, jagged edges on
/// any diagonal, which stood out badly against every other shape in this
/// file (`fill_button_dot`, `corners::blend_corner_pixel`) already being
/// smoothstep-antialiased. Distance-to-segment per candidate pixel, not a
/// stepped walk, so the diagonal close-glyph "X" gets the same smooth edge
/// its own button dot does. `shade` - see `blend_glyph_px`'s own doc
/// comment - passes straight through unchanged.
fn blend_glyph_line(buf: &mut [u8], width: usize, height: usize, from: (i32, i32), to: (i32, i32), alpha: u8, shade: (u8, u8, u8)) {
    if alpha == 0 {
        return;
    }
    let (x0, y0) = (from.0 as f32, from.1 as f32);
    let (x1, y1) = (to.0 as f32, to.1 as f32);
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = (dx * dx + dy * dy).max(0.0001);
    const FEATHER: f32 = 0.7;
    let reach = (GLYPH_HALF_WIDTH + FEATHER).ceil() as i32;
    let (xmin, xmax) = (from.0.min(to.0) - reach, from.0.max(to.0) + reach);
    let (ymin, ymax) = (from.1.min(to.1) - reach, from.1.max(to.1) + reach);
    for y in ymin..=ymax {
        if y < 0 || y as usize >= height {
            continue;
        }
        for x in xmin..=xmax {
            if x < 0 || x as usize >= width {
                continue;
            }
            let t = (((x as f32 - x0) * dx + (y as f32 - y0) * dy) / len2).clamp(0.0, 1.0);
            let (px, py) = (x0 + t * dx, y0 + t * dy);
            let dist = ((x as f32 - px).powi(2) + (y as f32 - py).powi(2)).sqrt();
            let ft = ((dist - GLYPH_HALF_WIDTH) / FEATHER).clamp(0.0, 1.0);
            let coverage = 1.0 - (ft * ft * (3.0 - 2.0 * ft));
            blend_glyph_px(buf, width, height, x, y, alpha, coverage, shade);
        }
    }
}

/// The `[0.46]` shrink is deliberate, not arbitrary - the glyph has to sit
/// visibly *inside* the dot's own circular edge (see `fill_button_dot`),
/// not touch or cross it, matching real macOS traffic-light glyphs, which
/// are always noticeably smaller than the button itself. `0.46`, not an
/// earlier `0.62`: reported live (a rendered dump compared directly
/// against a real reference screenshot) as too big - a real macOS hover
/// glyph reads as a small, delicate mark centred in the dot, not a shape
/// that nearly fills it.
fn glyph_box(width: usize, height: usize, offset: usize, from_left: bool, margin: f32) -> (i32, i32, i32, i32) {
    let (x0, y0, x1, y1) = button_box(width, height, offset, from_left, margin);
    let (cx, cy) = ((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0);
    let half = (x1 - x0).min(y1 - y0) as f32 / 2.0 * 0.46;
    ((cx - half).round() as i32, (cy - half).round() as i32, (cx + half).round() as i32, (cy + half).round() as i32)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_close_glyph(buf: &mut [u8], width: usize, height: usize, offset: usize, from_left: bool, margin: f32, alpha: u8, shade: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = glyph_box(width, height, offset, from_left, margin);
    blend_glyph_line(buf, width, height, (x0, y0), (x1, y1), alpha, shade);
    blend_glyph_line(buf, width, height, (x0, y1), (x1, y0), alpha, shade);
}

/// The plain square maximize icon - this project's own original look
/// (`traffic_lights = false`, a real Windows/GNOME titlebar's own
/// convention), and still what a traffic-light-style maximize falls back
/// to if it doesn't get `draw_zoom_glyph` instead. See `titlebar::
/// render_titlebar`'s own call site for which mode picks which.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_maximize_glyph(buf: &mut [u8], width: usize, height: usize, offset: usize, from_left: bool, margin: f32, alpha: u8, shade: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = glyph_box(width, height, offset, from_left, margin);
    blend_glyph_line(buf, width, height, (x0, y0), (x1, y0), alpha, shade);
    blend_glyph_line(buf, width, height, (x0, y1), (x1, y1), alpha, shade);
    blend_glyph_line(buf, width, height, (x0, y0), (x0, y1), alpha, shade);
    blend_glyph_line(buf, width, height, (x1, y0), (x1, y1), alpha, shade);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_minimize_glyph(buf: &mut [u8], width: usize, height: usize, offset: usize, from_left: bool, margin: f32, alpha: u8, shade: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = glyph_box(width, height, offset, from_left, margin);
    let mid = (y0 + y1) / 2;
    blend_glyph_line(buf, width, height, (x0, mid), (x1, mid), alpha, shade);
}

/// Real macOS's "zoom" maximize glyph - a double-headed diagonal arrow,
/// not a square - for `traffic_lights = true` only (see `titlebar::
/// render_titlebar`'s own call site). One diagonal shaft from the glyph
/// box's bottom-left to its top-right corner, plus a small two-stroke
/// arrowhead at each end pointing further outward (away from the glyph's
/// own centre) - the same primitive (`blend_glyph_line`) every other
/// glyph here already uses, so this reads as the same family of icon
/// rather than a different rendering technique bolted on just for this one
/// shape.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_zoom_glyph(buf: &mut [u8], width: usize, height: usize, offset: usize, from_left: bool, margin: f32, alpha: u8, shade: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = glyph_box(width, height, offset, from_left, margin);
    blend_glyph_line(buf, width, height, (x0, y1), (x1, y0), alpha, shade);
    let head = (((x1 - x0).max(1) as f32) * 0.4).round() as i32;
    // Top-right arrowhead, pointing further up-right (away from centre).
    blend_glyph_line(buf, width, height, (x1, y0), (x1 - head, y0), alpha, shade);
    blend_glyph_line(buf, width, height, (x1, y0), (x1, y0 + head), alpha, shade);
    // Bottom-left arrowhead, pointing further down-left (away from centre).
    blend_glyph_line(buf, width, height, (x0, y1), (x0 + head, y1), alpha, shade);
    blend_glyph_line(buf, width, height, (x0, y1), (x0, y1 - head), alpha, shade);
}

