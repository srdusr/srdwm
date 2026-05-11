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
pub(crate) use color::{mix_rgb, rgb_to_bgra};
pub(crate) use corners::{round_bottom_corners, round_top_corners};
pub(crate) use font::{blit_glyph, blit_glyph_over, find_system_font, measure_text_width, FONT_PIXELS, TEXT_LEFT_PADDING};
pub use shadow::{shadow_bitmap, shadow_rect, shadow_rect_clipped};
pub(crate) use shadow::{SHADOW_MAX_ALPHA, SHADOW_SIZE};
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
/// Rounded floating panel with a per-row rounded hover highlight, matching
/// the reference this project's own AGS panel already settled on for its
/// global-menu dropdown (`widget/Bar/components/GlobalMenu/style.scss`'s
/// `popover box.menu-list`): flat rows with no border/outline at rest, a
/// soft tinted fill (not a frame) on the highlighted one, inset padding so
/// rows don't touch the panel's own edge, real gaps between rows. Reported
/// live as looking "squished, no spacing/padding/margining, not at all
/// polished" - the previous version drew edge-to-edge square rows with a
/// single hard 1px border around the whole menu, exactly what that
/// complaint (raised about the AGS dropdown, fixed there first) describes.
/// Still no submenus/icons - real gaps beyond this pass' own scope, not
/// attempted blind.
///
/// A row's shape, alongside its own label/height - `render_context_menu`
/// used to detect a divider by checking whether a label was made entirely
/// of the box-drawing character `─`, and had no representation for a
/// section-header row at all (`"─── Move to Workspace ───"` rendered, and
/// behaved, as an ordinary clickable-looking item that merely did
/// nothing). Reported live as looking wrong on both counts - a real,
/// distinct row kind is what `srdwm_core::context_menu::MenuAction`'s own
/// `Separator`/`Header` variants exist for; this mirrors that split here
/// without this crate needing to depend on that enum itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuRowKind {
    Item,
    /// A thin divider line, no label.
    Separator,
    /// A non-interactive section caption - dimmer, smaller text, never
    /// highlighted.
    Header,
}

/// Rounded floating panel with a per-row rounded hover highlight, matching
/// the reference this project's own AGS panel already settled on for its
/// global-menu dropdown (`widget/Bar/components/GlobalMenu/style.scss`'s
/// `popover box.menu-list`): flat rows with no border/outline at rest, a
/// soft tinted fill (not a frame) on the highlighted one, inset padding so
/// rows don't touch the panel's own edge, real gaps between rows. Reported
/// live as looking "squished, no spacing/padding/margining, not at all
/// polished" - the previous version drew edge-to-edge square rows with a
/// single hard 1px border around the whole menu, exactly what that
/// complaint (raised about the AGS dropdown, fixed there first) describes.
/// Still no submenus/icons - real gaps beyond this pass' own scope, not
/// attempted blind.
///
/// `rows` carries each row's own height alongside its label/kind - a
/// [`MenuRowKind::Separator`]/[`MenuRowKind::Header`] row is much shorter
/// than a real item (see `srdwm_core::context_menu`'s own `SEPARATOR_
/// HEIGHT`/`HEADER_HEIGHT`), reported live as looking wrong when every
/// row, divider included, took the same full item height: a hairline
/// sitting in the middle of a mostly-empty 32px slot. The caller is what
/// actually knows each row's real height (`ContextMenu::row_height_for`);
/// this function just draws whatever it's told, at the offsets implied by
/// summing those heights in order - the same summing `ContextMenu::row_
/// at`/`row_y` do, so a click and a pixel can never disagree about where a
/// row actually is.
///
/// A [`MenuRowKind::Separator`] is drawn as a real 1px anti-aliased line,
/// inset from both edges and blended low-opacity against the panel
/// (matching the AGS reference dropdown's own `separator.menu-sep`:
/// `color-mix(in srgb, var(--fg) 12%, transparent)`) - pixels, not
/// Unicode box-drawing characters, which render inconsistently thin/
/// dotted across fonts at small sizes. A [`MenuRowKind::Header`] draws its
/// label dimmed (the same mix ratio a separator's own line uses) and
/// never highlighted, regardless of what the caller passes for that row's
/// `highlighted` field - section captions aren't clickable, so nothing
/// should ever visually suggest they are.
pub fn render_context_menu(width: u32, rows: &[(&str, bool, u32, MenuRowKind)], bg: (u8, u8, u8), fg: (u8, u8, u8), highlight_bg: (u8, u8, u8), border: (u8, u8, u8)) -> Vec<u8> {
    let _ = border; // No outline anywhere now - see this function's own doc comment. Kept as a parameter so callers/themes don't need updating for a look this function no longer draws.
    const PANEL_RADIUS: f32 = 10.0;
    const ROW_INSET: i32 = 4;
    const ROW_RADIUS: f32 = 6.0;
    // AGS reference's own measured/settled ratio (`popover box.menu-list
    // button:hover`'s `color-mix(in srgb, var(--primary-bg) 22%, var(
    // --widget-bg))`) - a subtle tinted wash, not a flat, fully-
    // saturated fill of whatever colour the caller passes as `highlight_
    // bg`. Applied here rather than changing what callers pass in, so
    // every existing call site's own colour choice still means "the
    // accent to tint toward", not "the literal pixel colour".
    const HIGHLIGHT_MIX: f32 = 0.22;
    // Matches the reference's own `separator.menu-sep` background --
    // barely-there, a hairline rather than a visible bar. Reused as the
    // header caption's own dim ratio - both exist to read as "present,
    // but deliberately not the main content" against the same panel.
    const DIM_MIX: f32 = 0.12;

    let width = width.max(1) as usize;
    let height = rows.iter().map(|(_, _, h, _)| *h).sum::<u32>().max(1) as usize;
    let mut buf = vec![0u8; width * height * 4];

    // The panel itself: one flat rounded-rect fill on an otherwise fully
    // transparent canvas, so the corners genuinely show whatever's behind
    // the menu (desktop/window content) rather than a hard-edged square.
    fill_rounded_rect(&mut buf, width, height, 0, 0, width as i32, height as i32, PANEL_RADIUS, bg, bg);

    let highlight_fill = mix_rgb(bg, highlight_bg, HIGHLIGHT_MIX);
    let dim_color = mix_rgb(bg, fg, DIM_MIX);
    let font = find_system_font();
    let mut row_top: i32 = 0;
    for (label, highlighted, row_height, kind) in rows.iter().copied() {
        let row_height = row_height as i32;
        if kind == MenuRowKind::Separator {
            let y = row_top + row_height / 2;
            fill_rounded_rect_over(&mut buf, width, height, ROW_INSET * 2, y, width as i32 - ROW_INSET * 2, y + 1, 0.0, dim_color);
            row_top += row_height;
            continue;
        }
        let highlighted = highlighted && kind != MenuRowKind::Header;
        let label_color = if kind == MenuRowKind::Header { dim_color } else { fg };
        // The background text actually sits on, for `blit_glyph`'s own
        // blend-toward-a-known-solid-colour contract - the row's own
        // highlight fill (already baked into `buf` by this point, above)
        // when highlighted, otherwise the panel's shared flat fill.
        // `blit_glyph_on_transparent` would be wrong here even though most
        // of `buf` started transparent: every row itself sits on the
        // panel's own opaque fill, not bare transparency, and that
        // blitter's whole design assumes the latter (see its own doc
        // comment) - used correctly, it would leave a visible dark
        // fringe around every character's anti-aliased edge instead of a
        // clean blend into the row's real colour.
        let row_bg = if highlighted { highlight_fill } else { bg };
        if highlighted {
            fill_rounded_rect_over(&mut buf, width, height, ROW_INSET, row_top, width as i32 - ROW_INSET, row_top + row_height, ROW_RADIUS, highlight_fill);
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
                    blit_glyph(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, row_bg, label_color);
                }
                pen_x += metrics.advance_width;
                if pen_x as usize >= width {
                    break;
                }
            }
        }
        row_top += row_height;
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

/// One desktop icon's cell: a hand-drawn glyph (no icon-theme artwork
/// exists anywhere in this workspace - see `desktop_icons.rs`'s own doc
/// comment) plus a centred label underneath, on an otherwise fully
/// transparent `width`x`height` canvas so the wallpaper shows through
/// everywhere the glyph/label don't draw. Same "deliberately plain, no
/// icon-theme fidelity" first-pass philosophy as `render_context_menu`.
///
/// `selected` draws an opaque highlight box behind the label only (not the
/// glyph) - matching the classic file-manager convention that the label,
/// not the whole cell, is what visibly marks a selection - rather than a
/// translucent overlay across the glyph, which would need premultiplied-
/// alpha blending this function has no other reason to do (every other
/// pixel here is drawn fully opaque or left fully transparent).
#[allow(clippy::too_many_arguments)]
/// The glyph area within a desktop-icon cell, shared between `render_
/// desktop_icon` (where it draws into) and its caller (which needs the
/// exact same box to know what size to rasterize a real theme icon at --
/// a mismatch would either leave a gap or need cropping, neither of which
/// `render_desktop_icon`'s own straight-copy blend handles).
pub(crate) fn desktop_icon_glyph_box(width: u32, height: u32) -> (i32, i32, i32, i32) {
    let _ = height;
    ((width as i32 - 40) / 2, 8, (width as i32 + 40) / 2, 44)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_desktop_icon(
    width: u32,
    height: u32,
    kind: crate::desktop_icons::IconKind,
    label: &str,
    selected: bool,
    icon_color: (u8, u8, u8),
    label_color: (u8, u8, u8),
    selected_bg: (u8, u8, u8),
    real_icon: Option<&[u8]>,
) -> Vec<u8> {
    use crate::desktop_icons::IconKind;
    let (width, height) = (width.max(1) as usize, height.max(1) as usize);
    let mut buf = vec![0u8; width * height * 4];

    let glyph_box = desktop_icon_glyph_box(width as u32, height as u32);
    if let Some(real_icon) = real_icon {
        // A real theme icon, already rasterized by `icon_theme::
        // rasterize_svg` at exactly this box's own size - premultiplied
        // BGRA8 straight from `resvg`, the same convention `blit_glyph_on_
        // transparent` below already uses, and `buf` starts fully
        // transparent (freshly zeroed) everywhere this box covers, so
        // "blend over" and "copy" are the same operation here: no alpha
        // math needed, unlike compositing onto a real background would.
        let box_w = (glyph_box.2 - glyph_box.0).max(0) as usize;
        let box_h = (glyph_box.3 - glyph_box.1).max(0) as usize;
        for row in 0..box_h {
            let y = glyph_box.1 + row as i32;
            if y < 0 || y as usize >= height {
                continue;
            }
            let src_row = &real_icon[row * box_w * 4..(row + 1) * box_w * 4];
            let dst_start = (y as usize * width + glyph_box.0.max(0) as usize) * 4;
            let copy_w = box_w.min(width.saturating_sub(glyph_box.0.max(0) as usize));
            buf[dst_start..dst_start + copy_w * 4].copy_from_slice(&src_row[..copy_w * 4]);
        }
    } else {
        // A top-lighter/bottom-`icon_color` vertical gradient, not one flat
        // fill - the same subtle top-to-bottom light-source cue `buttons.rs`'s
        // own `glossy_shade` uses for the titlebar dots, applied here as a
        // plain linear gradient (`fill_rounded_rect`'s own job) rather than a
        // radial highlight, which reads just as "polished" at this icon size
        // for a lot less code. `border` (outline/detail colour) stays a flat
        // darken of the base, unchanged. Only reached when no real icon-theme
        // artwork resolved at all (`icon_theme::find_icon` found nothing, or
        // the file it found failed to parse/render) - the fallback, not the
        // normal path on a machine with any real icon theme installed.
        let top = color::brighten(icon_color);
        let border = color::darken(icon_color);
        match kind {
            IconKind::Home => draw_home_glyph(&mut buf, width, height, glyph_box, top, icon_color, border),
            IconKind::Computer => draw_computer_glyph(&mut buf, width, height, glyph_box, top, icon_color, border),
            IconKind::Trash => draw_trash_glyph(&mut buf, width, height, glyph_box, top, icon_color, border),
            IconKind::Folder => draw_folder_glyph(&mut buf, width, height, glyph_box, top, icon_color, border),
            IconKind::File => draw_file_glyph(&mut buf, width, height, glyph_box, top, icon_color, border),
        }
    }

    let label_top = 50i32;
    if let Some(font) = find_system_font() {
        let baseline = label_top as f32 + 14.0;
        let mut widths = Vec::new();
        let mut total = 0.0f32;
        for ch in label.chars() {
            let (m, _) = font.rasterize(ch, FONT_PIXELS);
            widths.push(m.advance_width);
            total += m.advance_width;
        }
        // Reported live: the old selection highlight was a flat, edge-to-edge
        // rectangle spanning the label's whole vertical band (`label_top` to
        // `height - 2`, full cell width minus 2px either side) - "big
        // highlighting... for some reason", next to a saturated theme accent
        // colour (`selected_bg` is `theme.default_border_color`, e.g.
        // Catppuccin's mauve) it read as an oversized, disproportionate
        // block rather than a label being picked out. Real file managers
        // (Nautilus, Explorer) size the highlight to the text itself plus a
        // small margin, not to the cell - snug and rounded, same "fill, not
        // a frame" principle already applied to the context-menu rewrite.
        const LABEL_PAD_X: f32 = 6.0;
        const LABEL_PAD_Y: f32 = 3.0;
        const LABEL_RADIUS: f32 = 5.0;
        let text_height = FONT_PIXELS; // close enough for a snug box; exact ascent/descent isn't worth tracking here.
        if selected {
            let box_x0 = ((width as f32 - total) / 2.0 - LABEL_PAD_X).max(0.0);
            let box_x1 = ((width as f32 + total) / 2.0 + LABEL_PAD_X).min(width as f32);
            let box_y0 = (baseline - text_height - LABEL_PAD_Y).max(0.0);
            let box_y1 = (baseline + LABEL_PAD_Y).min(height as f32);
            fill_rounded_rect(&mut buf, width, height, box_x0.round() as i32, box_y0.round() as i32, box_x1.round() as i32, box_y1.round() as i32, LABEL_RADIUS, selected_bg, selected_bg);
        }
        let row_bg = selected_bg; // only meaningful when `selected`; `blit_glyph_on_transparent` below is used otherwise.
        let mut pen_x = ((width as f32 - total) / 2.0).max(2.0);
        for (ch, adv) in label.chars().zip(widths) {
            if ch.is_control() {
                pen_x += adv;
                continue;
            }
            let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
            if metrics.width > 0 && metrics.height > 0 {
                let glyph_x = pen_x + metrics.xmin as f32;
                let glyph_y = baseline - metrics.height as f32 - metrics.ymin as f32;
                if selected {
                    blit_glyph(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, row_bg, label_color);
                } else {
                    blit_glyph_on_transparent(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, label_color);
                }
            }
            pen_x += adv;
            if pen_x as usize >= width {
                break;
            }
        }
    }
    buf
}

/// Fills a straight-alpha `color` at `alpha` into every pixel of the given
/// rect, clamped to the canvas - the one primitive every glyph below is
/// built from. `alpha` is only ever `255` from any call site in this file
/// (every icon glyph is drawn fully opaque against an otherwise-transparent
/// canvas), so there's no premultiplication to get right here - straight
/// and premultiplied colour are identical at full opacity.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_rect(buf: &mut [u8], width: usize, height: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: (u8, u8, u8), alpha: u8) {
    let px = rgb_to_bgra(color, alpha);
    for y in y0.max(0)..y1.min(height as i32) {
        for x in x0.max(0)..x1.min(width as i32) {
            let idx = (y as usize * width + x as usize) * 4;
            buf[idx..idx + 4].copy_from_slice(&px);
        }
    }
}

/// Same job as `font::blit_glyph`, but for a canvas that starts fully
/// transparent rather than a known solid `background` colour to blend
/// toward - `blit_glyph` always writes full alpha, blended toward that
/// assumed background, which is wrong here: a partially-covered edge pixel
/// needs to stay partially *transparent*, not opaque-and-blended. Written
/// as real premultiplied-alpha BGRA (`rgb * alpha / 255`, matching alpha)
/// rather than straight colour at a partial alpha - see this session's own
/// `rounded_corners_pixman.rs` doc comments for why an un-premultiplied
/// partial-alpha pixel is a real, previously-hit correctness bug here, not
/// a style choice.
#[allow(clippy::too_many_arguments)]
pub(crate) fn blit_glyph_on_transparent(buf: &mut [u8], width: usize, height: usize, glyph_x: i32, glyph_y: i32, metrics: &fontdue::Metrics, coverage: &[u8], color: (u8, u8, u8)) {
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
            let alpha = coverage[row * metrics.width + col];
            if alpha == 0 {
                continue;
            }
            let premul = |c: u8| ((c as u32 * alpha as u32 + 127) / 255) as u8;
            let idx = (y as usize * width + x as usize) * 4;
            buf[idx..idx + 4].copy_from_slice(&rgb_to_bgra((premul(color.0), premul(color.1), premul(color.2)), alpha));
        }
    }
}

/// Same job as `fill_rect`, but with softly rounded corners (the same
/// clamp-then-distance smoothstep construction `rounded_corners_pixman::
/// apply_corner_mask` already established for real content masking, not a
/// new technique) and a vertical `top`-to-`bottom` gradient instead of one
/// flat colour - together, what actually takes a shape from "programmer
/// art" to reading as a small polished icon glyph rather than a coloured
/// rectangle.
#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect(buf: &mut [u8], width: usize, height: usize, x0: i32, y0: i32, x1: i32, y1: i32, radius: f32, top: (u8, u8, u8), bottom: (u8, u8, u8)) {
    let (w, h) = (width as i32, height as i32);
    let radius = radius.min((x1 - x0) as f32 / 2.0).min((y1 - y0) as f32 / 2.0).max(0.0);
    let rect_h = (y1 - y0).max(1) as f32;
    for y in y0.max(0)..y1.min(h) {
        let t = ((y - y0) as f32 / rect_h).clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        let color = (lerp(top.0, bottom.0), lerp(top.1, bottom.1), lerp(top.2, bottom.2));
        for x in x0.max(0)..x1.min(w) {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let cx = px.clamp(x0 as f32 + radius, x1 as f32 - radius);
            let cy = py.clamp(y0 as f32 + radius, y1 as f32 - radius);
            let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius;
            if dist >= 1.0 {
                continue;
            }
            let idx = (y as usize * width + x as usize) * 4;
            if dist <= -1.0 {
                buf[idx..idx + 4].copy_from_slice(&rgb_to_bgra(color, 255));
            } else {
                let alpha = (255.0 * (1.0 - smoothstep(-1.0, 1.0, dist))).round() as u8;
                let premul = |c: u8| ((c as u32 * alpha as u32 + 127) / 255) as u8;
                buf[idx..idx + 4].copy_from_slice(&rgb_to_bgra((premul(color.0), premul(color.1), premul(color.2)), alpha));
            }
        }
    }
}

/// Same rounded-rect antialiasing as `fill_rounded_rect`, but blends its
/// edge pixels against whatever is *already* in `buf` instead of assuming
/// a transparent canvas - needed for a menu row's hover highlight, which
/// paints on top of the panel's own already-opaque background fill.
/// `fill_rounded_rect` itself can't be reused there: its edge pixels
/// premultiply toward black (correct on a blank canvas, where "not fully
/// covered" means "let the transparent backdrop show through"), which
/// would show up as a visible dark seam around every rounded hover chip
/// sitting on top of an opaque panel instead of a clean blend into it.
#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect_over(buf: &mut [u8], width: usize, height: usize, x0: i32, y0: i32, x1: i32, y1: i32, radius: f32, color: (u8, u8, u8)) {
    let (w, h) = (width as i32, height as i32);
    let radius = radius.min((x1 - x0) as f32 / 2.0).min((y1 - y0) as f32 / 2.0).max(0.0);
    let new_px = rgb_to_bgra(color, 255);
    for y in y0.max(0)..y1.min(h) {
        for x in x0.max(0)..x1.min(w) {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let cx = px.clamp(x0 as f32 + radius, x1 as f32 - radius);
            let cy = py.clamp(y0 as f32 + radius, y1 as f32 - radius);
            let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius;
            if dist >= 1.0 {
                continue;
            }
            let idx = (y as usize * width + x as usize) * 4;
            if dist <= -1.0 {
                buf[idx..idx + 4].copy_from_slice(&new_px);
            } else {
                let t = 1.0 - smoothstep(-1.0, 1.0, dist);
                let existing = &buf[idx..idx + 4];
                let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
                let blended = (lerp(existing[0], new_px[0]), lerp(existing[1], new_px[1]), lerp(existing[2], new_px[2]), lerp(existing[3], new_px[3]));
                buf[idx..idx + 4].copy_from_slice(&[blended.0, blended.1, blended.2, blended.3]);
            }
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn draw_folder_glyph(buf: &mut [u8], width: usize, height: usize, b: (i32, i32, i32, i32), top: (u8, u8, u8), bottom: (u8, u8, u8), border: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = b;
    let tab_w = (x1 - x0) * 2 / 5;
    let tab_h = 6;
    fill_rounded_rect(buf, width, height, x0, y0, x0 + tab_w, y0 + tab_h + 2, 2.0, top, bottom);
    fill_rounded_rect(buf, width, height, x0, y0 + tab_h, x1, y1, 4.0, top, bottom);
    fill_rect(buf, width, height, x0 + 1, y0 + tab_h, x1 - 1, y0 + tab_h + 1, border, 255);
}

fn draw_computer_glyph(buf: &mut [u8], width: usize, height: usize, b: (i32, i32, i32, i32), top: (u8, u8, u8), bottom: (u8, u8, u8), border: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = b;
    let screen_bottom = y0 + (y1 - y0) * 3 / 4;
    fill_rounded_rect(buf, width, height, x0, y0, x1, screen_bottom, 3.0, border, border);
    fill_rounded_rect(buf, width, height, x0 + 2, y0 + 2, x1 - 2, screen_bottom - 2, 1.5, top, bottom);
    let stand_w = (x1 - x0) / 4;
    let stand_x0 = x0 + (x1 - x0 - stand_w) / 2;
    fill_rect(buf, width, height, stand_x0, screen_bottom, stand_x0 + stand_w, y1 - 2, border, 255);
    fill_rounded_rect(buf, width, height, x0 + 2, y1 - 3, x1 - 2, y1, 1.5, border, border);
}

fn draw_trash_glyph(buf: &mut [u8], width: usize, height: usize, b: (i32, i32, i32, i32), top: (u8, u8, u8), bottom: (u8, u8, u8), border: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = b;
    let lid_h = 4;
    fill_rounded_rect(buf, width, height, x0, y0, x1, y0 + lid_h, 1.5, border, border);
    let handle_w = (x1 - x0) / 3;
    let handle_x0 = x0 + (x1 - x0 - handle_w) / 2;
    fill_rect(buf, width, height, handle_x0, y0 - 3, handle_x0 + handle_w, y0, border, 255);
    let body_x0 = x0 + 2;
    let body_x1 = x1 - 2;
    fill_rounded_rect(buf, width, height, body_x0, y0 + lid_h, body_x1, y1, 3.0, top, bottom);
    // Three vertical ridge lines, the classic trash-can silhouette detail.
    let ridge_w = 2;
    for i in 0..3 {
        let rx = body_x0 + (body_x1 - body_x0) * (i * 2 + 1) / 6;
        fill_rect(buf, width, height, rx, y0 + lid_h + 3, rx + ridge_w, y1 - 3, border, 255);
    }
}

fn draw_file_glyph(buf: &mut [u8], width: usize, height: usize, b: (i32, i32, i32, i32), top: (u8, u8, u8), bottom: (u8, u8, u8), border: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = b;
    fill_rounded_rect(buf, width, height, x0, y0, x1, y1, 3.0, border, border);
    fill_rounded_rect(buf, width, height, x0 + 2, y0 + 2, x1 - 2, y1 - 2, 2.0, top, bottom);
    // A header strip near the top, the same "document" cue `draw_folder_
    // glyph`'s tab gives a folder - deliberately no folded-corner detail,
    // which would need a diagonal (not axis-aligned) fill this file's other
    // glyphs never need.
    fill_rect(buf, width, height, x0 + 4, y0 + 5, x1 - 4, y0 + 8, border, 255);
}

fn draw_home_glyph(buf: &mut [u8], width: usize, height: usize, b: (i32, i32, i32, i32), top: (u8, u8, u8), bottom: (u8, u8, u8), border: (u8, u8, u8)) {
    let (x0, y0, x1, y1) = b;
    let mid_x = (x0 + x1) / 2;
    let roof_y = y0 + (y1 - y0) / 3;
    // A simple triangular roof built from shrinking horizontal strips
    // (this file's only axis-aligned primitives are filled rects) rather
    // than a real diagonal line - coarse at this size, but reads clearly
    // as a roof over the body rect below it.
    let steps = (roof_y - y0).max(1);
    for i in 0..steps {
        let y = y0 + i;
        let inset = (i * (mid_x - x0)) / steps;
        fill_rect(buf, width, height, mid_x - inset - 1, y, mid_x + inset + 1, y + 1, border, 255);
    }
    fill_rounded_rect(buf, width, height, x0 + 2, roof_y, x1 - 2, y1, 2.0, top, bottom);
    fill_rect(buf, width, height, x0 + 2, roof_y, x1 - 2, roof_y + 2, border, 255);
    let door_w = (x1 - x0) / 4;
    let door_x0 = mid_x - door_w / 2;
    fill_rounded_rect(buf, width, height, door_x0, y1 - 10, door_x0 + door_w, y1, 1.5, border, border);
}

#[cfg(test)]
mod tests;
