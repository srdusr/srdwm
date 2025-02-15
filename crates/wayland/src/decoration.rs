//! Software rasterization of every hand-drawn piece of window chrome this
//! compositor's own CPU/Pixman render path draws itself: the titlebar band
//! (background, title text, button cluster), the border strips, the drop
//! shadow, and two small standalone popups (the titlebar right-click menu,
//! the Snap-Layouts flyout) - all as BGRA8 pixel buffers (the byte order
//! `Fourcc::Argb8888` expects when uploaded via `smithay`'s `GlesRenderer`,
//! see `format::gl_internal_format` - it maps to `GL_BGRA_EXT`/
//! `GL_UNSIGNED_BYTE`).
//!
//! Deliberately has zero `smithay` dependency throughout: every function
//! here is a pure `(dimensions, ...) -> Vec<u8>` call, unit-testable
//! without a GL context or display, with a thin adapter in `lib.rs`
//! uploading each result into a `MemoryRenderBuffer`.
//!
//! Split by concern, matching niri's own `render_helpers/` module
//! boundaries (see `docs/TODO.md`'s "module splits" entry for the research
//! behind that choice) - this file itself only re-exports each
//! submodule's own public API plus the two standalone popup renderers that
//! don't belong to any single one of them:
//! - [`color`]: byte-order conversion and the two directional colour
//!   blends (`brighten`/`darken`) shared by several submodules.
//! - [`font`]: locating a system monospace font and blitting its
//!   rasterized glyphs.
//! - [`corners`]: rounding a bitmap's own top/bottom corners to a
//!   quarter-circle - shared by `titlebar`/`border`.
//! - [`shadow`]: a window's drop shadow.
//! - [`border`]: the four strips around a window's `geometry`.
//! - [`buttons`]: the three titlebar buttons' own dots and glyphs.
//! - [`titlebar`]: laying out and rasterizing the whole titlebar band,
//!   using `buttons`/`corners`/`font`/`color`.

mod border;
mod buttons;
mod color;
mod corners;
mod font;
mod shadow;
mod titlebar;

pub use border::{border_strips, render_border_bottom, render_border_top};
pub(crate) use border::{border_bottom_visible_rows, border_top_visible_rows};
pub(crate) use buttons::HOVER_GLYPH_DURATION;
pub(crate) use color::rgb_to_bgra;
pub(crate) use corners::{round_bottom_corners, round_top_corners};
pub(crate) use font::{blit_glyph, find_system_font, FONT_PIXELS, TEXT_LEFT_PADDING};
pub use shadow::{shadow_bitmap, shadow_rect};
pub(crate) use shadow::SHADOW_MAX_ALPHA;
pub use titlebar::render_titlebar;

/// Default corner radius, in pixels, applied to a window at creation
/// (`Window::corner_radius`/`ThemeConfig::default_corner_radius`) - kept
/// here only as this file's own test fixture default now that the real
/// radius is a live, per-window value (`theme.decorations.border.radius`
/// in config, `srd set corner_radius <n>` live, `srd.window.
/// set_corner_radius(n)`/a rule's `corner_radius` action per-window).
/// `render_border_top`/`render_border_bottom`/`render_titlebar` all take
/// the real radius as a parameter now rather than reading this directly.
/// `12`, not the original `6`: a `radius / TITLEBAR_HEIGHT` ratio of
/// `0.36`, matching real macOS's own ~10pt/28pt proportions rather than
/// this project's original, visibly-tighter `0.2` (docs/TODO.md's
/// macOS-comparison research) - kept in sync with `ThemeConfig::
/// default_corner_radius` and the shipped `theme.decorations.border.radius`
/// default in `crates/srdwm/src/main.rs` so this fixture actually
/// represents what a fresh install renders. Moves in step with
/// `srdwm_core::TITLEBAR_HEIGHT` (currently `32`) - same ratio, not a
/// separate size decision.
#[cfg(test)]
pub(crate) const CORNER_RADIUS: u32 = 12;

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

#[cfg(test)]
mod tests;
