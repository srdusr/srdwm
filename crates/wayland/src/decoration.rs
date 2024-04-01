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

const FONT_PIXELS: f32 = 13.0;
const TEXT_LEFT_PADDING: f32 = 8.0;

/// Common monospace font file locations on Linux desktops. Not a full
/// fontconfig query (no new system dependency for something this small) --
/// if none of these resolve, titlebars fall back to solid-color-only, same
/// as before text rendering existed.
fn find_system_font() -> Option<Font> {
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

fn rgb_to_bgra(rgb: (u8, u8, u8), alpha: u8) -> [u8; 4] {
    [rgb.2, rgb.1, rgb.0, alpha]
}

/// Renders a `width x height` BGRA8 buffer: filled with `background`, with
/// `title` drawn left-aligned in `foreground` (best-effort glyph layout --
/// no text shaping/kerning, adequate for the ASCII-heavy titles window
/// managers actually display). Returns `None` (caller keeps the previous
/// solid-color-only look) only if no usable font was found on this system.
pub fn render_titlebar(width: u32, height: u32, title: &str, background: (u8, u8, u8), foreground: (u8, u8, u8)) -> Vec<u8> {
    let (width, height) = (width.max(1) as usize, height.max(1) as usize);
    let bg = rgb_to_bgra(background, 255);
    let mut buf = vec![0u8; width * height * 4];
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }

    let Some(font) = find_system_font() else { return buf };

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
        if pen_x as usize >= width {
            break;
        }
    }
    buf
}

#[allow(clippy::too_many_arguments)]
fn blit_glyph(
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
    fn fills_background_when_no_text() {
        let buf = render_titlebar(40, 20, "", (0x2e, 0x34, 0x40), (0xec, 0xef, 0xf4));
        assert_eq!(buf.len(), 40 * 20 * 4);
        assert_eq!(&buf[0..4], &rgb_to_bgra((0x2e, 0x34, 0x40), 255));
    }

    #[test]
    fn drawing_title_changes_some_pixels_when_font_available() {
        if find_system_font().is_none() {
            eprintln!("skipping: no system font found in this sandbox");
            return;
        }
        let bg = (0x2e, 0x34, 0x40);
        let fg = (0xec, 0xef, 0xf4);
        let buf = render_titlebar(200, 30, "Terminal", bg, fg);
        let bg_bytes = rgb_to_bgra(bg, 255);
        let changed = buf.chunks_exact(4).any(|px| px != bg_bytes);
        assert!(changed, "expected at least one pixel to differ from the background once text is drawn");
    }

    #[test]
    fn empty_title_leaves_buffer_all_background() {
        let bg = (0x10, 0x20, 0x30);
        let buf = render_titlebar(50, 24, "", bg, (0xff, 0xff, 0xff));
        let bg_bytes = rgb_to_bgra(bg, 255);
        assert!(buf.chunks_exact(4).all(|px| px == bg_bytes));
    }
}
