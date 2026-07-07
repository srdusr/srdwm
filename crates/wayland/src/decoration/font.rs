//! Locating a system monospace font and blitting its rasterized glyphs into
//! a titlebar's own pixel buffer - everything `render_titlebar`'s title-
//! text pass needs, kept separate from the titlebar layout/button logic
//! that actually calls it.

use super::color::rgb_to_bgra;
use fontdue::{Font, FontSettings};
use std::sync::OnceLock;

pub(crate) const FONT_PIXELS: f32 = 13.0;
pub(crate) const TEXT_LEFT_PADDING: f32 = 8.0;

/// Common monospace font file locations on Linux desktops. Not a full
/// fontconfig query (no new system dependency for something this small) --
/// if none of these resolve, titlebars fall back to solid-color-only, same
/// as before text rendering existed.
pub(crate) fn find_system_font() -> Option<Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(load_any_monospace_font).clone()
}

/// The real rendered pixel width of `text` at `size`, in this font - the
/// same two-pass "sum every glyph's own advance width" measurement `render_
/// header_box`'s `draw_centered` already does inline, pulled out here so a
/// caller that needs to *size a box* around text (not just draw it) has one
/// place to ask, rather than repeating the sum. Returns `0.0` with no
/// system font found, matching every other text-rendering path's own
/// "solid colour only, no text at all" fallback in that case.
pub(crate) fn measure_text_width(font: &Option<Font>, text: &str, size: f32) -> f32 {
    let Some(font) = font else { return 0.0 };
    text.chars().filter(|c| !c.is_control()).map(|ch| font.rasterize(ch, size).0.advance_width).sum()
}

/// A proportional UI font for prose, falling back to the monospace one.
///
/// [`find_system_font`] deliberately prefers a *monospace* face, which is
/// right for a titlebar title and a menu row - they sit in columns and
/// benefit from even advances. It is wrong for a sentence. The lock
/// screen's "Enter Password" prompt rendered in DejaVu Sans Mono, which is
/// what this machine's font scan picks, and reads as a mistake next to
/// every other prompt on the system. Reported as the prompt having "a weird
/// font".
///
/// Ranked by preference rather than taking whatever a directory walk turns
/// up first, so the result does not depend on filesystem order the way the
/// mono scan historically did (it once picked an italic face and rendered
/// every titlebar in italic).
pub(crate) fn find_ui_font() -> Option<Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(|| load_ui_font().or_else(load_any_monospace_font)).clone()
}

fn load_ui_font() -> Option<Font> {
    // Common, widely-installed proportional faces, best first. Each is
    // matched against the whole path lowercased, so a distro's own
    // subdirectory layout does not matter.
    const PREFERRED: [&str; 8] =
        ["inter-regular", "cantarell-regular", "notosans-regular", "dejavusans.ttf", "liberationsans-regular", "opensans-regular", "roboto-regular", "freesans"];
    let mut roots: Vec<String> = ["/usr/share/fonts", "/usr/local/share/fonts"].iter().map(|s| s.to_string()).collect();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(format!("{home}/.local/share/fonts"));
        roots.push(format!("{home}/.fonts"));
    }
    let mut best: Option<(std::path::PathBuf, usize)> = None;
    for root in &roots {
        collect_ui_font(std::path::Path::new(root), &PREFERRED, &mut best);
    }
    let (path, _) = best?;
    let bytes = std::fs::read(&path).ok()?;
    match Font::from_bytes(bytes, FontSettings::default()) {
        Ok(f) => {
            log::info!("ui font: {}", path.display());
            Some(f)
        }
        Err(e) => {
            log::warn!("failed to parse ui font {}: {e}", path.display());
            None
        }
    }
}

fn collect_ui_font(dir: &std::path::Path, preferred: &[&str], best: &mut Option<(std::path::PathBuf, usize)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ui_font(&path, preferred, best);
            continue;
        }
        let name = path.to_string_lossy().to_lowercase();
        if !(name.ends_with(".ttf") || name.ends_with(".otf")) || name.contains("mono") || name.contains("italic") || name.contains("oblique") {
            continue;
        }
        let Some(rank) = preferred.iter().position(|p| name.contains(p)) else { continue };
        if best.as_ref().is_none_or(|(_, r)| rank < *r) {
            *best = Some((path, rank));
        }
    }
}

fn load_any_monospace_font() -> Option<Font> {
    let roots = ["/usr/share/fonts", "/usr/local/share/fonts"];
    let mut home_roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        home_roots.push(format!("{home}/.local/share/fonts"));
        home_roots.push(format!("{home}/.fonts"));
    }
    let all_roots = roots.iter().map(|s| s.to_string()).chain(home_roots);

    let mut best: Option<(std::path::PathBuf, u8)> = None;
    for root in all_roots {
        find_best_font(std::path::Path::new(&root), &mut best);
        if matches!(best, Some((_, 0))) {
            break;
        }
    }
    let (path, _) = best?;
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

/// Ranks a font file by how suitable it is for titlebar text: `0` (best) is
/// a mono-named file with no weight/style marker at all (a plain
/// "Regular"), `1` is mono but italic/oblique/some other non-regular
/// weight, `2` is not mono-named at all. Lower is better.
///
/// The style-marker list matters as much as the mono check: without it, a
/// directory listing that happens to turn up `...Mono...-Italic.ttf`
/// before any regular-weight mono file sorts no worse than one, and gets
/// picked and stuck with for the process's whole lifetime (`find_system_
/// font`'s own `OnceLock`). Reported live: this exact case, `/usr/share/
/// fonts/TTF/JetBrainsMonoNerdFontPropo-Italic.ttf` picked over every
/// regular-weight JetBrains Mono variant also installed on the same
/// system, purely because "mono" matched and nothing excluded italic --
/// every titlebar rendered in italic instead of upright text.
fn font_rank(path: &std::path::Path) -> u8 {
    let name = path.to_string_lossy().to_lowercase();
    let is_mono = name.contains("mono");
    let is_styled =
        ["italic", "oblique", "bold", "light", "thin", "black", "medium", "semibold", "extrabold", "condensed"].iter().any(|s| name.contains(s));
    match (is_mono, is_styled) {
        (true, false) => 0,
        (true, true) => 1,
        (false, _) => 2,
    }
}

/// Walks `dir` for the lowest-`font_rank` `.ttf`/`.otf` file, checking
/// every file rather than stopping at the first mono match - unlike rank
/// alone, "first found" says nothing about *style*, and the filesystem's
/// own directory-listing order is not something to trust for that. Only
/// stops early once a genuine rank-`0` (mono, unstyled) match is found,
/// since nothing could ever beat that.
fn find_best_font(dir: &std::path::Path, best: &mut Option<(std::path::PathBuf, u8)>) {
    if matches!(best, Some((_, 0))) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_best_font(&path, best);
            if matches!(best, Some((_, 0))) {
                return;
            }
            continue;
        }
        let is_font = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf")).unwrap_or(false);
        if !is_font {
            continue;
        }
        let rank = font_rank(&path);
        if best.as_ref().is_none_or(|(_, r)| rank < *r) {
            *best = Some((path, rank));
            if rank == 0 {
                return;
            }
        }
    }
}

/// Composites a glyph over whatever is already in `buf`, honouring the
/// destination's own alpha - straight (non-premultiplied) BGRA, the same
/// convention `rgb_to_bgra` writes everywhere else in this module.
///
/// [`blit_glyph`] below blends the glyph against one flat opaque colour and
/// writes alpha 255, which is right for a surface that has an opaque
/// background of its own (a titlebar, a menu row). It is wrong for text
/// drawn straight onto a transparent surface: every glyph pixel would
/// become an opaque block of the assumed background colour, so the text
/// would sit in a solid rectangle of exactly the box this was meant to
/// remove. Used by the lock screen's password field, which draws over the
/// blurred wallpaper with no panel behind it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn blit_glyph_over(
    buf: &mut [u8],
    width: usize,
    height: usize,
    glyph_x: i32,
    glyph_y: i32,
    metrics: &fontdue::Metrics,
    coverage: &[u8],
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
            let sa = coverage[row * metrics.width + col] as f32 / 255.0;
            if sa <= 0.0 {
                continue;
            }
            let idx = (y as usize * width + x as usize) * 4;
            let (db, dg, dr, da) = (buf[idx] as f32, buf[idx + 1] as f32, buf[idx + 2] as f32, buf[idx + 3] as f32 / 255.0);
            let out_a = sa + da * (1.0 - sa);
            if out_a <= 0.0 {
                continue;
            }
            // Un-premultiplied source-over: each channel weighted by its
            // own coverage, then divided back out by the result's alpha.
            let mix = |fg: f32, dst: f32| -> u8 { (((fg * sa) + (dst * da * (1.0 - sa))) / out_a).round().clamp(0.0, 255.0) as u8 };
            buf[idx] = mix(foreground.2 as f32, db);
            buf[idx + 1] = mix(foreground.1 as f32, dg);
            buf[idx + 2] = mix(foreground.0 as f32, dr);
            buf[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
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
