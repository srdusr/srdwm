//! Software rasterization of the titlebar band: solid background + drawn
//! title text, as a BGRA8 pixel buffer (the byte order `Fourcc::Argb8888`
//! expects when uploaded via `smithay`'s `GlesRenderer`, see
//! `format::gl_internal_format` - it maps to `GL_BGRA_EXT`/`GL_UNSIGNED_BYTE`).
//!
//! Deliberately has zero `smithay` dependency: it's a pure `(width, height,
//! text) -> Vec<u8>` function, unit-testable without a GL context or
//! display, with a thin adapter in `lib.rs` uploading the result into a
//! `MemoryRenderBuffer`.

use fontdue::{Font, FontSettings};
use std::sync::OnceLock;

pub(crate) const FONT_PIXELS: f32 = 13.0;
pub(crate) const TEXT_LEFT_PADDING: f32 = 8.0;

/// Titlebar buttons are laid out right-aligned in `height`-wide squares --
/// matching `ResizeEdge::hit_test` in `crates/core/src/window.rs`, whose
/// `BUTTON` constant is also `TITLEBAR_HEIGHT`. That function only computes
/// *where* a click on close/maximize/minimize lands; nothing painted the
/// buttons themselves, so the whole band was one undifferentiated bar with
/// no visible way to tell where those three clickable regions were.
const BUTTON_MARGIN: f32 = 0.32;

/// Common monospace font file locations on Linux desktops. Not a full
/// fontconfig query (no new system dependency for something this small) --
/// if none of these resolve, titlebars fall back to solid-color-only, same
/// as before text rendering existed.
pub(crate) fn find_system_font() -> Option<Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(load_any_monospace_font).clone()
}

fn load_any_monospace_font() -> Option<Font> {
    let roots = ["/usr/share/fonts", "/usr/local/share/fonts"];
    let mut home_roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        home_roots.push(format!("{home}/.local/share/fonts"));
        home_roots.push(format!("{home}/.fonts"));
    }
    let all_roots = roots.iter().map(|s| s.to_string()).chain(home_roots);

    let mut best: Option<std::path::PathBuf> = None;
    for root in all_roots {
        find_ttf_preferring_mono(std::path::Path::new(&root), &mut best);
        if best.is_some() {
            break;
        }
    }
    let path = best?;
    let bytes = std::fs::read(&path).ok()?;
    match Font::from_bytes(bytes, FontSettings::default()) {
        Ok(f) => {
            log::info!("wayland titlebar font: {}", path.display());
            Some(f)
        }
        Err(e) => {
            log::warn!("failed to parse font {}: {e}", path.display());
            None
        }
    }
}

/// Walks `dir` looking for a `.ttf`/`.otf` file, preferring one whose name
/// contains "mono". Stops early once a mono-named file is found.
fn find_ttf_preferring_mono(dir: &std::path::Path, best: &mut Option<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_ttf_preferring_mono(&path, best);
            if matches!(best, Some(p) if p.to_string_lossy().to_lowercase().contains("mono")) {
                return;
            }
            continue;
        }
        let is_font = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf")).unwrap_or(false);
        if !is_font {
            continue;
        }
        let is_mono = path.to_string_lossy().to_lowercase().contains("mono");
        if is_mono {
            *best = Some(path);
            return;
        }
        if best.is_none() {
            *best = Some(path);
        }
    }
}

pub(crate) fn rgb_to_bgra(rgb: (u8, u8, u8), alpha: u8) -> [u8; 4] {
    [rgb.2, rgb.1, rgb.0, alpha]
}

/// The titlebar right-click window menu (minimize/maximize/always-on-top/
/// close) - the one interaction virtually every desktop WM has always
/// offered on a titlebar that srdwm never did (right-click there was only
/// ever the SUPER+right-drag resize gesture, and only with the modifier
/// held). `items` is `(label, highlighted)`; `row_height` matches
/// `TITLEBAR_HEIGHT` by convention at the call site, not enforced here.
///
/// Deliberately plain: solid rows, left-padded text, a 1px border for
/// definition against whatever's behind it - no submenus, no icons, no
/// separators. A context menu widget with real visual polish is a project
/// of its own; this is the minimum that makes the actions discoverable and
/// clickable at all, which is the actual gap.
pub fn render_context_menu(width: u32, row_height: u32, items: &[(&str, bool)], bg: (u8, u8, u8), fg: (u8, u8, u8), highlight_bg: (u8, u8, u8), border: (u8, u8, u8)) -> Vec<u8> {
    let (width, row_height) = (width.max(1) as usize, row_height.max(1) as usize);
    let height = (row_height * items.len().max(1)).max(1);
    let mut buf = vec![0u8; width * height * 4];

    let font = find_system_font();
    for (i, (label, highlighted)) in items.iter().enumerate() {
        let row_bg = if *highlighted { highlight_bg } else { bg };
        let row_top = i * row_height;
        for y in row_top..(row_top + row_height).min(height) {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                buf[idx..idx + 4].copy_from_slice(&rgb_to_bgra(row_bg, 255));
            }
        }
        if let Some(font) = &font {
            let baseline = row_top as f32 + row_height as f32 * 0.72;
            let mut pen_x = TEXT_LEFT_PADDING;
            for ch in label.chars() {
                if ch.is_control() {
                    continue;
                }
                let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
                if metrics.width > 0 && metrics.height > 0 {
                    let glyph_x = pen_x + metrics.xmin as f32;
                    let glyph_y = baseline - metrics.height as f32 - metrics.ymin as f32;
                    blit_glyph(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, row_bg, fg);
                }
                pen_x += metrics.advance_width;
                if pen_x as usize >= width {
                    break;
                }
            }
        }
    }

    // A 1px border around the whole menu, drawn last so it isn't overdrawn
    // by any row's background fill.
    let border_px = rgb_to_bgra(border, 255);
    for x in 0..width {
        buf[x * 4..x * 4 + 4].copy_from_slice(&border_px);
        let last_row = (height - 1) * width + x;
        buf[last_row * 4..last_row * 4 + 4].copy_from_slice(&border_px);
    }
    for y in 0..height {
        let left = y * width;
        buf[left * 4..left * 4 + 4].copy_from_slice(&border_px);
        let right = y * width + width - 1;
        buf[right * 4..right * 4 + 4].copy_from_slice(&border_px);
    }
    buf
}

/// The Snap-Layouts flyout (`crates/wayland/src/snap_flyout.rs`) - a
/// `columns`-wide grid of labeled cells, one per `SnapZoneKind`. Same
/// "deliberately plain" bar `render_context_menu` above sets: solid cells,
/// left-padded text, grid lines and an outer border in one colour - no
/// live preview thumbnails, no icons.
pub fn render_snap_flyout(columns: u32, cell_width: u32, cell_height: u32, labels: &[&str], bg: (u8, u8, u8), fg: (u8, u8, u8), border: (u8, u8, u8)) -> Vec<u8> {
    let (cell_width, cell_height, columns) = (cell_width.max(1) as usize, cell_height.max(1) as usize, columns.max(1) as usize);
    let rows = labels.len().div_ceil(columns).max(1);
    let width = cell_width * columns;
    let height = cell_height * rows;
    let mut buf = vec![0u8; width * height * 4];

    let bg_px = rgb_to_bgra(bg, 255);
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg_px);
    }

    let font = find_system_font();
    for (i, label) in labels.iter().enumerate() {
        let (col, row) = (i % columns, i / columns);
        let (cell_x, cell_y) = (col * cell_width, row * cell_height);
        if let Some(font) = &font {
            let baseline = cell_y as f32 + cell_height as f32 * 0.58;
            let mut pen_x = cell_x as f32 + TEXT_LEFT_PADDING;
            for ch in label.chars() {
                if ch.is_control() {
                    continue;
                }
                let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
                if metrics.width > 0 && metrics.height > 0 {
                    let glyph_x = pen_x + metrics.xmin as f32;
                    let glyph_y = baseline - metrics.height as f32 - metrics.ymin as f32;
                    blit_glyph(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, bg, fg);
                }
                pen_x += metrics.advance_width;
                if pen_x as usize >= cell_x + cell_width {
                    break;
                }
            }
        }
    }

    // Grid lines (including the outer border) drawn last, one colour, so
    // nothing overdraws them.
    let border_px = rgb_to_bgra(border, 255);
    for y in 0..height {
        for col in 0..=columns {
            let x = (col * cell_width).min(width - 1);
            let idx = y * width + x;
            buf[idx * 4..idx * 4 + 4].copy_from_slice(&border_px);
        }
    }
    for x in 0..width {
        for row in 0..=rows {
            let y = (row * cell_height).min(height - 1);
            let idx = y * width + x;
            buf[idx * 4..idx * 4 + 4].copy_from_slice(&border_px);
        }
    }
    buf
}

/// The four border strips (top, bottom, left, right) around a window's
/// full rect, `width` thick, drawn *outside* `geometry` - additive to the
/// window's on-screen footprint, the same as a native X11 border, rather
/// than overlapping and clipping into the titlebar or content. This is
/// purely a rendering concern: `geometry` alone stays authoritative for
/// hit-testing and placement, nothing reads the strips back.
///
/// Without any border at all, a compositor-drawn titlebar and independently
/// client-rendered content have nothing visually tying them together as
/// one window - reported live as the titlebar "not seeming part of the
/// window". `Window.border_color`/`border_width` already existed (and are
/// drawn by the X11 backend via a native X11 border) but were dead fields
/// on the Wayland side - `set_border_color`/`set_border_width` were both
/// no-op stubs.
pub fn border_strips(geometry: srdwm_core::Rect, width: u32) -> [srdwm_core::Rect; 4] {
    let w = width as i32;
    [
        srdwm_core::Rect::new(geometry.x - w, geometry.y - w, geometry.width + 2 * width, width),
        srdwm_core::Rect::new(geometry.x - w, geometry.y + geometry.height as i32, geometry.width + 2 * width, width),
        srdwm_core::Rect::new(geometry.x - w, geometry.y, width, geometry.height),
        srdwm_core::Rect::new(geometry.x + geometry.width as i32, geometry.y, width, geometry.height),
    ]
}

/// How far a window's drop shadow extends past its geometry on each side.
pub const SHADOW_SIZE: u32 = 12;

/// The shadow's darkest alpha, right at the window's own edge - out of
/// 255. Deliberately subtle (Nord/GNOME-default territory, not a heavy
/// drop shadow): this compositor has no blur primitive to soften it with
/// (see `shadow_bitmap`'s own doc comment), so a strong value would read as
/// a hard dark ring rather than a shadow.
const SHADOW_MAX_ALPHA: u8 = 90;

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
/// "approximate cutoff over true anti-aliasing" trade-off `round_top_corners`
/// already makes for corners.
///
/// The region directly under the window itself (`dist == 0` below) is left
/// fully transparent rather than filled - harmless either way since the
/// window's own border/titlebar/content always draws over it, but skipping
/// it is one less branch of work for the common case (a window with no
/// occluders in front of it, so most of the bitmap's interior never
/// contributes a visible pixel).
pub fn shadow_bitmap(win_width: u32, win_height: u32) -> Vec<u8> {
    let (win_width, win_height) = (win_width.max(1), win_height.max(1));
    let width = win_width + SHADOW_SIZE * 2;
    let height = win_height + SHADOW_SIZE * 2;
    let mut buf = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        let dy = edge_distance(y, SHADOW_SIZE, win_height);
        if dy > SHADOW_SIZE {
            continue;
        }
        for x in 0..width {
            let dx = edge_distance(x, SHADOW_SIZE, win_width);
            let dist = dx.max(dy);
            if dist == 0 || dist > SHADOW_SIZE {
                continue;
            }
            let alpha = (SHADOW_MAX_ALPHA as u32 * (SHADOW_SIZE - dist) / SHADOW_SIZE) as u8;
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

/// Renders the top border strip (`border_strips`'s first rect) as a BGRA8
/// bitmap instead of a plain solid fill, with its own outer top corners cut
/// the same way `render_titlebar`'s `round_corners` cuts the titlebar's --
/// see that parameter's doc comment for why a titlebar rounds but a square
/// border frame around it used to defeat the point. Rounding *this* strip
/// too, at a radius `width` pixels larger than the titlebar's (so the cut
/// continues outward from the titlebar's own, rather than starting over),
/// is what makes a bordered window's corner read as one continuous curve
/// instead of a rounded titlebar sitting inside a square frame.
///
/// [`render_border_bottom`] gives the bottom strip the matching treatment
/// for its own two corners. The left/right strips don't participate in any
/// visible corner at all (`border_strips`' geometry has them span only the
/// height *between* the top and bottom strips) and stay plain solid fills
/// - see their render call sites.
pub fn render_border_top(width: u32, thickness: u32, color: (u8, u8, u8), radius: u32) -> Vec<u8> {
    let (width, thickness) = (width.max(1) as usize, thickness.max(1) as usize);
    let bg = rgb_to_bgra(color, 255);
    let mut buf = vec![0u8; width * thickness * 4];
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }
    round_top_corners(&mut buf, width, thickness, radius + thickness as u32);
    buf
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
    let bg = rgb_to_bgra(color, 255);
    let mut buf = vec![0u8; width * thickness * 4];
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }
    round_bottom_corners(&mut buf, width, thickness, radius + thickness as u32);
    buf
}

/// Renders a `width x height` BGRA8 buffer: filled with `background`, with
/// `title` drawn left-aligned in `foreground` (best-effort glyph layout --
/// no text shaping/kerning, adequate for the ASCII-heavy titles window
/// managers actually display). Returns `None` (caller keeps the previous
/// solid-color-only look) only if no usable font was found on this system.
///
/// `round_corners` should be `false` only for a window whose border strips
/// are rendered as plain square-cornered fills with no matching rounded
/// treatment of their own. `render_border_top` gives the border's top strip
/// the same rounded-corner cut (see its own doc comment for how the two
/// stay visually continuous), so a normal bordered window should pass
/// `true` here same as a borderless one now - reported live as most
/// windows (anything with the default border) looking inconsistently
/// square next to the few borderless ones that were rounded.
pub fn render_titlebar(width: u32, height: u32, title: &str, background: (u8, u8, u8), foreground: (u8, u8, u8), round_corners: bool, radius: u32) -> Vec<u8> {
    let (width, height) = (width.max(1) as usize, height.max(1) as usize);
    let bg = rgb_to_bgra(background, 255);
    let mut buf = vec![0u8; width * height * 4];
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }

    // Reserve the right-hand button squares before laying out text, so a
    // long title elides under them the same way it would under real window
    // furniture rather than drawing on top of it.
    let button_count = if width >= height * 3 { 3 } else { 0 };
    let text_limit = width.saturating_sub(height * button_count);

    if let Some(font) = find_system_font() {
        let baseline = (height as f32 * 0.72).round();
        let mut pen_x = TEXT_LEFT_PADDING;
        for ch in title.chars() {
            if ch.is_control() {
                continue;
            }
            let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
            if metrics.width > 0 && metrics.height > 0 {
                let glyph_x = pen_x + metrics.xmin as f32;
                let glyph_y = baseline - metrics.height as f32 - metrics.ymin as f32;
                blit_glyph(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, background, foreground);
            }
            pen_x += metrics.advance_width;
            if pen_x as usize >= text_limit {
                break;
            }
        }
    }

    if button_count == 3 {
        draw_minimize_icon(&mut buf, width, height, height * 2, foreground);
        draw_maximize_icon(&mut buf, width, height, height, foreground);
        draw_close_icon(&mut buf, width, height, 0, foreground);
    }
    if round_corners {
        round_top_corners(&mut buf, width, height, radius);
    }
    buf
}

/// Default corner radius, in pixels, applied to a window at creation
/// (`Window::corner_radius`/`ThemeConfig::default_corner_radius`) - kept
/// here only as this file's own test fixture default now that the real
/// radius is a live, per-window value (`theme.decorations.border.radius`
/// in config, `srd set corner_radius <n>` live, `srd.window.
/// set_corner_radius(n)`/a rule's `corner_radius` action per-window).
/// `render_border_top`/`render_border_bottom`/`render_titlebar` all take
/// the real radius as a parameter now rather than reading this directly.
#[cfg(test)]
pub(crate) const CORNER_RADIUS: u32 = 6;

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
fn round_top_corners(buf: &mut [u8], width: usize, height: usize, radius: u32) {
    let r = (radius as usize).min(width / 2).min(height);
    if r == 0 {
        return;
    }
    // Corner centres: `r` in from each edge, `r` down from the top - the
    // standard quarter-circle-in-a-square construction.
    let is_outside_corner = |x: usize, y: usize, cx: usize, cy: usize| -> bool {
        let (dx, dy) = (x as i64 - cx as i64, y as i64 - cy as i64);
        (dx * dx + dy * dy) as u64 > (r * r) as u64
    };
    for y in 0..r {
        for x in 0..r {
            if is_outside_corner(x, y, r, r) {
                buf[(y * width + x) * 4..(y * width + x) * 4 + 4].fill(0);
            }
        }
        for x in (width - r)..width {
            if is_outside_corner(x, y, width - r - 1, r) {
                buf[(y * width + x) * 4..(y * width + x) * 4 + 4].fill(0);
            }
        }
    }
}

/// [`round_top_corners`]'s mirror for the bottom two corners - same
/// construction, corner centres `r` *up* from the bottom instead of down
/// from the top. Same premultiplied-alpha fix, same reason - see that
/// function's own doc comment.
fn round_bottom_corners(buf: &mut [u8], width: usize, height: usize, radius: u32) {
    let r = (radius as usize).min(width / 2).min(height);
    if r == 0 {
        return;
    }
    // `cy` as a signed offset, not a `usize` - `height - r` can be exactly
    // `0` (a strip whose radius clamp landed on its own full height, same
    // as `round_top_corners` allows for `r == height`), which would
    // underflow a plain `usize` subtraction one step further below.
    let is_outside_corner = |x: usize, y: usize, cx: usize, cy: i64| -> bool {
        let (dx, dy) = (x as i64 - cx as i64, y as i64 - cy);
        (dx * dx + dy * dy) as u64 > (r * r) as u64
    };
    let cy = height as i64 - r as i64 - 1;
    for y in (height - r)..height {
        for x in 0..r {
            if is_outside_corner(x, y, r, cy) {
                buf[(y * width + x) * 4..(y * width + x) * 4 + 4].fill(0);
            }
        }
        for x in (width - r)..width {
            if is_outside_corner(x, y, width - r - 1, cy) {
                buf[(y * width + x) * 4..(y * width + x) * 4 + 4].fill(0);
            }
        }
    }
}

/// Sets one pixel to `color` if it falls inside the buffer - every icon
/// drawn below goes through this so none of them need their own bounds
/// checks.
fn set_px(buf: &mut [u8], width: usize, height: usize, x: i32, y: i32, color: (u8, u8, u8)) {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
        return;
    }
    let idx = (y as usize * width + x as usize) * 4;
    buf[idx..idx + 4].copy_from_slice(&rgb_to_bgra(color, 255));
}

/// The square `right_offset` pixels in from the right edge of the titlebar,
/// inset by `BUTTON_MARGIN` on each side - the box a button's glyph is
/// drawn inside.
fn button_box(width: usize, height: usize, right_offset: usize) -> (i32, i32, i32, i32) {
    let square = height as f32;
    let inset = (square * BUTTON_MARGIN).round() as i32;
    let right = width as i32 - right_offset as i32;
    let left = right - height as i32;
    (left + inset, inset, right - inset, height as i32 - inset)
}

/// Bresenham line, since none of these icons need anything fancier.
fn draw_line(buf: &mut [u8], width: usize, height: usize, from: (i32, i32), to: (i32, i32), color: (u8, u8, u8)) {
    let (mut x0, mut y0) = from;
    let (x1, y1) = to;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        set_px(buf, width, height, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_close_icon(buf: &mut [u8], width: usize, height: usize, right_offset: usize, color: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = button_box(width, height, right_offset);
    draw_line(buf, width, height, (x0, y0), (x1, y1), color);
    draw_line(buf, width, height, (x0, y1), (x1, y0), color);
}

fn draw_maximize_icon(buf: &mut [u8], width: usize, height: usize, right_offset: usize, color: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = button_box(width, height, right_offset);
    draw_line(buf, width, height, (x0, y0), (x1, y0), color);
    draw_line(buf, width, height, (x0, y1), (x1, y1), color);
    draw_line(buf, width, height, (x0, y0), (x0, y1), color);
    draw_line(buf, width, height, (x1, y0), (x1, y1), color);
}

fn draw_minimize_icon(buf: &mut [u8], width: usize, height: usize, right_offset: usize, color: (u8, u8, u8)) {
    let (x0, _, x1, y1) = button_box(width, height, right_offset);
    draw_line(buf, width, height, (x0, y1), (x1, y1), color);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn blit_glyph(
    buf: &mut [u8],
    width: usize,
    height: usize,
    glyph_x: i32,
    glyph_y: i32,
    metrics: &fontdue::Metrics,
    coverage: &[u8],
    background: (u8, u8, u8),
    foreground: (u8, u8, u8),
) {
    for row in 0..metrics.height {
        let y = glyph_y + row as i32;
        if y < 0 || y as usize >= height {
            continue;
        }
        for col in 0..metrics.width {
            let x = glyph_x + col as i32;
            if x < 0 || x as usize >= width {
                continue;
            }
            let cov = coverage[row * metrics.width + col] as f32 / 255.0;
            if cov <= 0.0 {
                continue;
            }
            let blend = |bg: u8, fg: u8| -> u8 { (bg as f32 * (1.0 - cov) + fg as f32 * cov).round() as u8 };
            let r = blend(background.0, foreground.0);
            let g = blend(background.1, foreground.1);
            let b = blend(background.2, foreground.2);
            let idx = (y as usize * width + x as usize) * 4;
            buf[idx..idx + 4].copy_from_slice(&rgb_to_bgra((r, g, b), 255));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_strips_surround_geometry_without_overlapping_it() {
        let geom = srdwm_core::Rect::new(100, 100, 200, 150);
        let [top, bottom, left, right] = border_strips(geom, 3);
        // Every strip's own rect must stay entirely outside `geom` - these
        // are meant to frame the window, not clip into its own titlebar or
        // content.
        assert_eq!(top, srdwm_core::Rect::new(97, 97, 206, 3));
        assert_eq!(bottom, srdwm_core::Rect::new(97, 250, 206, 3));
        assert_eq!(left, srdwm_core::Rect::new(97, 100, 3, 150));
        assert_eq!(right, srdwm_core::Rect::new(300, 100, 3, 150));
    }

    #[test]
    fn shadow_rect_expands_geometry_by_shadow_size_on_every_side() {
        let geom = srdwm_core::Rect::new(100, 100, 200, 150);
        let s = shadow_rect(geom);
        assert_eq!(s, srdwm_core::Rect::new(100 - SHADOW_SIZE as i32, 100 - SHADOW_SIZE as i32, 200 + SHADOW_SIZE * 2, 150 + SHADOW_SIZE * 2));
    }

    #[test]
    fn shadow_bitmap_is_the_expected_size_and_transparent_under_the_window() {
        let buf = shadow_bitmap(40, 20);
        let width = 40 + SHADOW_SIZE * 2;
        let height = 20 + SHADOW_SIZE * 2;
        assert_eq!(buf.len(), (width * height * 4) as usize);
        // Dead center is inside the window's own footprint - must stay
        // fully transparent, since the window's own content draws over it.
        let mid = ((height / 2) * width + width / 2) * 4;
        assert_eq!(buf[mid as usize + 3], 0);
    }

    #[test]
    fn shadow_bitmap_is_darkest_right_at_the_window_edge_and_fades_outward() {
        let buf = shadow_bitmap(40, 20);
        let width = (40 + SHADOW_SIZE * 2) as usize;
        // Walking straight up from the window's horizontal center, from one
        // pixel above its top edge (row SHADOW_SIZE - 1) out to the shadow's
        // own outer edge (row 0): alpha must start near SHADOW_MAX_ALPHA and
        // strictly decrease to 0.
        let x = width / 2;
        let mut last_alpha = 255u8;
        for row in (0..SHADOW_SIZE as usize).rev() {
            let i = (row * width + x) * 4;
            let alpha = buf[i + 3];
            assert!(alpha <= last_alpha, "alpha rose from {last_alpha} to {alpha} moving outward at row {row}");
            last_alpha = alpha;
        }
        assert_eq!(last_alpha, 0, "outermost row must be fully transparent");
    }

    #[test]
    fn fills_background_when_no_text() {
        let buf = render_titlebar(40, 20, "", (0x2e, 0x34, 0x40), (0xec, 0xef, 0xf4), true, CORNER_RADIUS);
        assert_eq!(buf.len(), 40 * 20 * 4);
        // Center, not (0,0): the top-left pixel is inside the rounded
        // corner `round_top_corners` clips away, so it's transparent by
        // design - see `corners_are_clipped_but_the_middle_is_not` below.
        let mid = ((20 / 2) * 40 + 40 / 2) * 4;
        assert_eq!(&buf[mid..mid + 4], &rgb_to_bgra((0x2e, 0x34, 0x40), 255));
    }

    #[test]
    fn button_icons_are_drawn_in_the_squares_hit_test_assigns_them() {
        // Regression test for a bug where every drawn icon was one full
        // button-width left of where a click on it actually landed: the
        // visible "X" triggered Maximize, the visible square triggered
        // Minimize, and the true Close hit-zone (the rightmost
        // TITLEBAR_HEIGHT-wide band) was blank. `button_box`'s
        // `right_offset` must put each icon in the same square
        // `ResizeEdge::hit_test` assigns to it - checked here by picking
        // the centre pixel of each drawn icon's square and confirming
        // `hit_test` reports the matching button for that same point.
        let (width, height) = (300u32, srdwm_core::TITLEBAR_HEIGHT);
        let bg = (0x2e, 0x34, 0x40);
        let fg = (0xec, 0xef, 0xf4);
        let buf = render_titlebar(width, height, "", bg, fg, true, CORNER_RADIUS);
        let frame = srdwm_core::Rect::new(0, 0, width, height);
        let (width, height) = (width as usize, height as usize);

        let bg_bytes = rgb_to_bgra(bg, 255);
        for (right_offset, expected) in [(0, srdwm_core::TitlebarHit::Close), (height, srdwm_core::TitlebarHit::Maximize), (height * 2, srdwm_core::TitlebarHit::Minimize)] {
            let (x0, y0, x1, y1) = button_box(width, height, right_offset);
            let drawn = (y0..=y1).any(|y| (x0..=x1).any(|x| buf[(y as usize * width + x as usize) * 4..(y as usize * width + x as usize) * 4 + 4] != bg_bytes));
            assert!(drawn, "expected some drawn icon pixel inside the right_offset={right_offset} square");
            let cx = (x0 + x1) / 2;
            let cy = (y0 + y1) / 2;
            assert_eq!(
                srdwm_core::ResizeEdge::hit_test(frame, cx, cy, true, 0, srdwm_core::RESIZE_MARGIN),
                Some(expected),
                "icon drawn at right_offset={right_offset} does not land in the square hit_test assigns to {expected:?}"
            );
        }
    }

    #[test]
    fn drawing_title_changes_some_pixels_when_font_available() {
        if find_system_font().is_none() {
            eprintln!("skipping: no system font found in this sandbox");
            return;
        }
        let bg = (0x2e, 0x34, 0x40);
        let fg = (0xec, 0xef, 0xf4);
        let buf = render_titlebar(200, 30, "Terminal", bg, fg, true, CORNER_RADIUS);
        let bg_bytes = rgb_to_bgra(bg, 255);
        let changed = buf.chunks_exact(4).any(|px| px != bg_bytes);
        assert!(changed, "expected at least one pixel to differ from the background once text is drawn");
    }

    #[test]
    fn empty_title_leaves_buffer_all_background_outside_the_rounded_corners() {
        let bg = (0x10, 0x20, 0x30);
        let (width, height) = (50, 24);
        let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), true, CORNER_RADIUS);
        let bg_bytes = rgb_to_bgra(bg, 255);
        for (i, px) in buf.chunks_exact(4).enumerate() {
            let (x, y) = (i % width as usize, i / width as usize);
            let in_top_left = x < CORNER_RADIUS as usize && y < CORNER_RADIUS as usize;
            let in_top_right = x >= width as usize - CORNER_RADIUS as usize && y < CORNER_RADIUS as usize;
            if !in_top_left && !in_top_right {
                assert_eq!(px, bg_bytes, "unexpected non-background pixel at ({x}, {y})");
            }
        }
    }

    #[test]
    fn corners_are_clipped_but_the_middle_is_not() {
        let bg = (0x10, 0x20, 0x30);
        let (width, height) = (50, 24);
        let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), true, CORNER_RADIUS);
        let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
        // The very corner pixel is well outside the quarter-circle at any
        // sane radius - fully clipped.
        assert_eq!(alpha_at(0, 0), 0, "top-left corner pixel should be transparent");
        assert_eq!(alpha_at(width as usize - 1, 0), 0, "top-right corner pixel should be transparent");
        // Bottom corners are deliberately left square (see the function's
        // doc comment: the titlebar's bottom edge meets client content,
        // which can't be clipped the same way).
        assert_eq!(alpha_at(0, height as usize - 1), 255, "bottom-left must stay square");
        assert_eq!(alpha_at(width as usize - 1, height as usize - 1), 255, "bottom-right must stay square");
        // Centre is nowhere near either corner circle - untouched.
        assert_eq!(alpha_at(width as usize / 2, height as usize / 2), 255);
    }

    #[test]
    fn clipped_corner_pixels_are_fully_premultiplied_zero_not_just_alpha() {
        // Regression test: `round_top_corners` used to zero only the alpha
        // byte of a clipped pixel, leaving the opaque background RGB behind
        // it untouched. This buffer is `Fourcc::Argb8888`, which both
        // Wayland/`wl_shm` and Pixman treat as premultiplied - Pixman's own
        // `OVER` compositing (`result = src + dst * (1 - src_alpha)`) does
        // not treat `alpha=0, rgb=<something>` as "contributes nothing": it
        // adds that stale, un-premultiplied `rgb` straight through, so the
        // "clipped" corner still rendered fully opaque and every window's
        // top corners read as square regardless of a nonzero radius --
        // confirmed live, pixel-by-pixel, zero transparency anywhere in a
        // real window's corner. A genuinely transparent premultiplied pixel
        // is `(0, 0, 0, 0)` in every channel, not just alpha.
        let bg = (0x10, 0x20, 0x30);
        let (width, height) = (50, 24);
        let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), true, CORNER_RADIUS);
        let px_at = |x: usize, y: usize| &buf[(y * width as usize + x) * 4..(y * width as usize + x) * 4 + 4];
        assert_eq!(px_at(0, 0), [0, 0, 0, 0], "top-left corner pixel must be fully zeroed (premultiplied transparent), not just alpha");
        assert_eq!(px_at(width as usize - 1, 0), [0, 0, 0, 0], "top-right corner pixel must be fully zeroed (premultiplied transparent), not just alpha");
    }

    #[test]
    fn round_corners_false_leaves_the_top_corners_square() {
        let bg = (0x10, 0x20, 0x30);
        let (width, height) = (50, 24);
        let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), false, CORNER_RADIUS);
        let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 255, "top-left corner should stay square when round_corners is false");
        assert_eq!(alpha_at(width as usize - 1, 0), 255, "top-right corner should stay square when round_corners is false");
    }

    #[test]
    fn border_top_rounds_its_own_top_corners_to_match_the_titlebar() {
        // Regression coverage for the "not all window borders are rounded"
        // report: a bordered window's titlebar used to render with
        // `round_corners = false` specifically to avoid clashing with this
        // strip's square corners. Now that this strip rounds too, that
        // workaround is gone (`render_titlebar` is always called with
        // `true`) - this just confirms the strip actually does what that
        // change now depends on.
        let color = (0x40, 0x50, 0x60);
        let (width, thickness) = (60, 2);
        let buf = render_border_top(width, thickness, color, CORNER_RADIUS);
        let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 0, "top-left corner pixel should be clipped");
        assert_eq!(alpha_at(width as usize - 1, 0), 0, "top-right corner pixel should be clipped");
        // A 2px-thick strip is thinner than any sane radius, so the clamp
        // in `round_top_corners` bounds the cut to the strip's own height --
        // the bottom row, at least at the strip's horizontal centre, must
        // stay opaque or there would be no border left to see at all.
        assert_eq!(alpha_at(width as usize / 2, thickness as usize - 1), 255, "centre of the strip must stay opaque");
    }

    #[test]
    fn border_bottom_rounds_its_own_bottom_corners() {
        let color = (0x40, 0x50, 0x60);
        let (width, thickness) = (60, 2);
        let buf = render_border_bottom(width, thickness, color, CORNER_RADIUS);
        let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
        assert_eq!(alpha_at(0, thickness as usize - 1), 0, "bottom-left corner pixel should be clipped");
        assert_eq!(alpha_at(width as usize - 1, thickness as usize - 1), 0, "bottom-right corner pixel should be clipped");
        assert_eq!(alpha_at(width as usize / 2, 0), 255, "centre of the strip must stay opaque");
    }

    #[test]
    fn context_menu_is_one_row_tall_per_item() {
        let items = [("Minimize", false), ("Maximize", false), ("Always on Top", false), ("Close", false)];
        let buf = render_context_menu(160, 28, &items, (0x2e, 0x34, 0x40), (0xff, 0xff, 0xff), (0x4c, 0x56, 0x6a), (0x10, 0x10, 0x10));
        assert_eq!(buf.len(), 160 * (28 * 4) * 4);
    }

    #[test]
    fn context_menu_highlighted_row_has_a_different_background_than_the_rest() {
        let items = [("Minimize", false), ("Close", true)];
        let bg = (0x2e, 0x34, 0x40);
        let highlight = (0x4c, 0x56, 0x6a);
        let buf = render_context_menu(160, 28, &items, bg, (0xff, 0xff, 0xff), highlight, (0x10, 0x10, 0x10));
        let width = 160usize;
        // Sample a background pixel from each row, away from the text/border.
        let px_at = |x: usize, y: usize| -> [u8; 3] {
            let i = (y * width + x) * 4;
            [buf[i + 2], buf[i + 1], buf[i]] // BGRA -> RGB
        };
        assert_eq!(px_at(100, 5), [bg.0, bg.1, bg.2], "row 0 (not highlighted) should use bg");
        assert_eq!(px_at(100, 33), [highlight.0, highlight.1, highlight.2], "row 1 (highlighted) should use highlight_bg");
    }

    #[test]
    fn context_menu_border_is_opaque_at_every_edge() {
        let items = [("Close", false)];
        let buf = render_context_menu(100, 28, &items, (0, 0, 0), (0xff, 0xff, 0xff), (0, 0, 0), (0x99, 0x99, 0x99));
        let alpha_at = |x: usize, y: usize| buf[(y * 100 + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 255);
        assert_eq!(alpha_at(99, 0), 255);
        assert_eq!(alpha_at(0, 27), 255);
        assert_eq!(alpha_at(99, 27), 255);
    }

    #[test]
    fn snap_flyout_is_sized_for_a_full_grid_of_labels() {
        let labels = ["Left Half", "Right Half", "Top Left", "Top Right", "Bottom Left", "Bottom Right"];
        let buf = render_snap_flyout(3, 90, 60, &labels, (0x2e, 0x34, 0x40), (0xff, 0xff, 0xff), (0x10, 0x10, 0x10));
        // 3 columns x 2 rows (6 labels / 3 columns, rounded up).
        assert_eq!(buf.len(), (90 * 3) * (60 * 2) * 4);
    }

    #[test]
    fn snap_flyout_border_is_opaque_at_every_outer_edge() {
        let labels = ["A", "B", "C", "D", "E", "F"];
        let (cell_w, cell_h) = (90, 60);
        let buf = render_snap_flyout(3, cell_w, cell_h, &labels, (0, 0, 0), (0xff, 0xff, 0xff), (0x99, 0x99, 0x99));
        let (width, height) = (cell_w * 3, cell_h * 2);
        let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 255);
        assert_eq!(alpha_at(width as usize - 1, 0), 255);
        assert_eq!(alpha_at(0, height as usize - 1), 255);
        assert_eq!(alpha_at(width as usize - 1, height as usize - 1), 255);
    }

    #[test]
    fn snap_flyout_has_an_internal_grid_line_between_columns() {
        let labels = ["A", "B", "C", "D", "E", "F"];
        let (cell_w, cell_h) = (90, 60);
        let buf = render_snap_flyout(3, cell_w, cell_h, &labels, (0, 0, 0), (0xff, 0xff, 0xff), (0x99, 0x99, 0x99));
        let width = cell_w * 3;
        // The boundary between column 0 and column 1, away from the outer border.
        let idx = (30 * width as usize + cell_w as usize) * 4;
        assert_eq!(buf[idx + 3], 255, "column boundary must be drawn, not just the outer border");
    }
}
