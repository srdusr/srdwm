//! Laying out and rasterizing the whole titlebar band: background, title
//! text, and the button cluster (which one goes where, which side, how
//! many). The buttons' own dots/glyphs are `buttons.rs`'s job; the corner
//! cut at the end is `corners.rs`'s.

use super::buttons::{
    draw_close_glyph, draw_maximize_glyph, draw_minimize_glyph, draw_zoom_glyph, fill_button_dot, BUTTON_MARGIN, BUTTON_MARGIN_LEFT, TRAFFIC_LIGHT_CLOSE,
    TRAFFIC_LIGHT_INACTIVE, TRAFFIC_LIGHT_MAXIMIZE, TRAFFIC_LIGHT_MINIMIZE,
};
use super::color::{brighten, darken, rgb_to_bgra};
use super::corners::round_top_corners;
use super::font::{blit_glyph, find_system_font, FONT_PIXELS, TEXT_LEFT_PADDING};

/// Renders a `width x height` BGRA8 buffer: filled with `background`, with
/// `title` drawn left-aligned in `foreground` (best-effort glyph layout --
/// no text shaping/kerning, adequate for the ASCII-heavy titles window
/// managers actually display). Returns `None` (caller keeps the previous
/// solid-color-only look) only if no usable font was found on this system.
///
/// `round_corners` should be `false` only for a window whose border strips
/// are rendered as plain square-cornered fills with no matching rounded
/// treatment of their own. `border::render_border_top` gives the border's
/// top strip the same rounded-corner cut (see its own doc comment for how
/// the two stay visually continuous), so a normal bordered window should
/// pass `true` here same as a borderless one now - reported live as most
/// windows (anything with the default border) looking inconsistently
/// square next to the few borderless ones that were rounded.
///
/// `border_width` shifts the corner circle's centre by that many rows (see
/// `corners::round_top_corners`'s own doc comment): a titlebar with a
/// border strip sitting above it starts `border_width` rows *into* the
/// shared circle, not at its top, so it needs the same shift subtracted to
/// draw its own correct slice of that one circle rather than a second,
/// uncoordinated one. Pass `0` for an undecorated/borderless window's
/// titlebar (there is none in practice - an undecorated window has no
/// titlebar at all - but `0` is also the correct, harmless value if
/// `round_corners` handling ever changes to allow it).
/// The most characters of a title that are ever measured.
///
/// Nothing legible survives past this in any titlebar a person would use,
/// but a client is free to set a title of any length at all - a browser
/// tab carrying a whole data URL, say - and every character costs a
/// rasterization before the layout below can decide it does not fit. This
/// bounds that work without bounding what is drawn: a title this long is
/// elided many times over regardless.
pub(crate) const MAX_TITLE_CHARS: usize = 256;

/// Lays a title out into `available` pixels, ending it with an ellipsis if
/// it does not fit.
///
/// Truncating at the end is what Windows, GNOME and KDE all do, and it
/// keeps the informative half of a window title - an application or
/// document name almost always begins distinctively and ends in boilerplate
/// ("... - Mozilla Firefox"). The ellipsis matters on its own: hard-cutting
/// a title mid-glyph, which is what this did before, leaves no sign that
/// anything was removed, so a truncated name reads as the whole name.
///
/// Prefers a real "…" and falls back to three periods when the system font
/// has no such glyph, since a missing glyph rasterizes to nothing at all
/// and would silently reintroduce the invisible-truncation problem.
pub(crate) fn lay_out_title(font: &fontdue::Font, title: &str, available: f32) -> Vec<(fontdue::Metrics, Vec<u8>)> {
    let ellipsis: &[char] = if font.lookup_glyph_index('\u{2026}') != 0 { &['\u{2026}'] } else { &['.', '.', '.'] };
    let mut glyphs: Vec<(fontdue::Metrics, Vec<u8>)> = Vec::new();
    let mut width = 0.0f32;
    let mut truncated = false;
    for ch in title.chars().filter(|c| !c.is_control()).take(MAX_TITLE_CHARS) {
        let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
        if width + metrics.advance_width > available {
            truncated = true;
            break;
        }
        width += metrics.advance_width;
        glyphs.push((metrics, coverage));
    }
    if title.chars().filter(|c| !c.is_control()).count() > MAX_TITLE_CHARS {
        truncated = true;
    }
    if !truncated {
        return glyphs;
    }
    let marks: Vec<(fontdue::Metrics, Vec<u8>)> = ellipsis.iter().map(|&c| font.rasterize(c, FONT_PIXELS)).collect();
    let marks_width: f32 = marks.iter().map(|(m, _)| m.advance_width).sum();
    // Drop whole characters off the end until the ellipsis fits beside what
    // is left. A title with no room even for the ellipsis draws nothing
    // rather than a lone "…", which says less than an empty titlebar does.
    while !glyphs.is_empty() && width + marks_width > available {
        if let Some((metrics, _)) = glyphs.pop() {
            width -= metrics.advance_width;
        }
    }
    if glyphs.is_empty() && marks_width > available {
        return Vec::new();
    }
    glyphs.extend(marks);
    glyphs
}

#[allow(clippy::too_many_arguments)]
pub fn render_titlebar(
    width: u32,
    height: u32,
    title: &str,
    background: (u8, u8, u8),
    foreground: (u8, u8, u8),
    round_corners: bool,
    radius: u32,
    border_width: u32,
    focused: bool,
    // `(button, glyph alpha 0..=255)` - the alpha is the eased hover-
    // reveal animation's own current progress (see `tick_hover_glyph_
    // animation`), already discretized by the caller so this stays a
    // plain data-in function with no `Instant`/timing concept of its own.
    hovered: Option<(srdwm_core::TitlebarHit, u8)>,
    centered: bool,
    buttons_left: bool,
    // Modern GNOME/Adwaita mode (see `ThemeConfig::button_glyph_always`'s
    // own doc comment): every glyph drawn at full opacity always, `hovered`
    // only still used for the background-circle brighten below, not glyph
    // visibility.
    glyph_always: bool,
    // `ThemeConfig::button_order`'s resolved value - see `ButtonOrder`'s
    // own doc comment. Must stay in agreement with whatever `ResizeEdge::
    // hit_test` was called with for the same window, the same "renders on
    // one side, hit-tests on the other" trap `buttons_left` itself already
    // has to avoid.
    button_order: Option<srdwm_core::ButtonOrder>,
    // `ThemeConfig::traffic_light_buttons`'s resolved value - see its own
    // doc comment for what each mode actually draws differently.
    traffic_lights: bool,
    // `Window::is_dialog`'s resolved value - see its own doc comment. Only
    // Close is ever drawn for a dialog, and never as a coloured traffic
    // light regardless of `traffic_lights` above (forced off below):
    // requested directly ("dialog windows shouldn't have maximize/minimize
    // buttons... don't use traffic lights there ever"). Must stay in exact
    // agreement with `ResizeEdge::hit_test`'s own `is_dialog` parameter,
    // the same "renders on one side, hit-tests on the other" trap every
    // other button-geometry value here already has to avoid.
    is_dialog: bool,
    // Whether a Maximize button is drawn at all - `WindowManager::
    // show_maximize`'s answer, which resolves `theme.dynamic_buttons`
    // against the window's own `resizable`. Must stay in exact agreement
    // with `ResizeEdge::hit_test`'s own `show_maximize` parameter, the
    // same "renders on one side, hit-tests on the other" contract
    // `is_dialog` directly above already carries.
    show_maximize: bool,
) -> Vec<u8> {
    let (width, height) = (width.max(1) as usize, height.max(1) as usize);
    // Forced off, not just defaulted - a dialog never gets coloured
    // traffic lights even when the active theme otherwise uses them
    // everywhere else.
    let traffic_lights = traffic_lights && !is_dialog;
    let bg = rgb_to_bgra(background, 255);
    let mut buf = vec![0u8; width * height * 4];
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }

    // Reserve the button squares (whichever side they're actually on)
    // before laying out text, so a long title elides under them the same
    // way it would under real window furniture rather than drawing on top
    // of it. `text_start`/`text_limit` bound the span text is allowed to
    // occupy - both edges when `buttons_left` (buttons eat into the left,
    // not the right), only the far edge otherwise.
    let pitch = srdwm_core::BUTTON_PITCH as usize;
    let cluster_margin = srdwm_core::BUTTON_CLUSTER_MARGIN as usize;
    // A dialog only ever gets one button (Close) - see this function's own
    // `is_dialog` doc comment.
    let wanted_buttons = if is_dialog {
        1
    } else if show_maximize {
        3
    } else {
        2
    };
    let button_count = if width >= cluster_margin + pitch * wanted_buttons { wanted_buttons } else { 0 };
    // `BUTTON_CLUSTER_MARGIN` included, not just the buttons' own `pitch *
    // button_count` span - the cluster's own leading gap needs reserving
    // too, or a long title's text could draw underneath it (or, on the
    // `buttons_left` side, right through the gap between the titlebar's
    // real edge and the first button).
    let reserved = if button_count > 0 { cluster_margin + pitch * button_count } else { 0 };
    let (text_start, text_limit) = if buttons_left { (reserved as f32, width as f32) } else { (TEXT_LEFT_PADDING, width.saturating_sub(reserved) as f32) };

    if let Some(font) = find_system_font() {
        let baseline = (height as f32 * 0.72).round();
        // Rasterized up front, not drawn incrementally in one pass - see
        // `title_centered`'s own doc comment: centering needs the title's
        // total advance width known *before* the first pixel is placed,
        // and reusing these glyphs for the real draw below avoids
        // rasterizing every character twice just to get there. Same
        // truncation rule as before this existed: a glyph whose own
        // advance crosses `text_limit` still gets drawn (matches a real
        // window's furniture starting exactly at `text_limit`, not one
        // glyph-width short of it), only the *next* one is dropped.
        let glyphs = lay_out_title(&font, title, text_limit - text_start);
        let total_width: f32 = glyphs.iter().map(|(m, _)| m.advance_width).sum();
        // Centered on the *whole* titlebar width, not on the narrower
        // `text_start..text_limit` span left over after reserving the
        // button squares - matches real macOS, which ignores its own
        // traffic-light cluster for centering purposes rather than
        // centering in the remaining space. Centering in the reserved
        // span instead (the previous behaviour) put the text visibly off
        // the window's true center - for a 3-button, 30px-tall titlebar
        // that's a 90px reservation, shifting the centered point 45px
        // off true center, exactly the "not real center" a user would
        // notice at a glance. Still clamped into `text_start..text_limit`
        // afterward so a long title never draws under the buttons.
        let start_x = if centered { ((width as f32 - total_width) / 2.0).max(text_start).min((text_limit - total_width).max(text_start)) } else { text_start };
        let mut pen_x = start_x;
        for (metrics, coverage) in &glyphs {
            if metrics.width > 0 && metrics.height > 0 {
                let glyph_x = pen_x + metrics.xmin as f32;
                let glyph_y = baseline - metrics.height as f32 - metrics.ymin as f32;
                blit_glyph(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, metrics, coverage, background, foreground);
            }
            pen_x += metrics.advance_width;
        }
    }

    if button_count > 0 {
        let (mut close_c, mut minimize_c, mut maximize_c) = if !traffic_lights {
            // Unused in this mode (no dot is ever filled at rest - see the
            // `traffic_lights` branch below), except as the base colour
            // `brighten` starts from for the neutral hover backdrop.
            (background, background, background)
        } else if focused {
            (TRAFFIC_LIGHT_CLOSE, TRAFFIC_LIGHT_MINIMIZE, TRAFFIC_LIGHT_MAXIMIZE)
        } else {
            (TRAFFIC_LIGHT_INACTIVE, TRAFFIC_LIGHT_INACTIVE, TRAFFIC_LIGHT_INACTIVE)
        };
        // Explicitly requested background-highlight-on-hover for the
        // titlebar buttons (see docs/TODO.md) - brightens whichever one
        // is actually hovered, close included, rather than giving close a
        // separate hardcoded hover colour: close is already red at rest
        // (focused) or grey (unfocused), same as the other two, so
        // "red-on-hover for close" falls out of this same brightening,
        // not a special case. Brightened as soon as a hover is in
        // progress at all (any glyph alpha > 0), not gated on it having
        // finished animating in - the circle brightening and the glyph
        // reveal read as one combined "waking up" motion when they start
        // together, not two separately-timed effects.
        //
        // One button at a time, not the whole cluster - a group-hover
        // version (matching real macOS's own behaviour) was tried in this
        // same session and explicitly reverted: the user confirmed this
        // project's own convention is per-button, not per-cluster, despite
        // what real macOS itself does.
        let (mut close_glyph, mut minimize_glyph, mut maximize_glyph) = (0u8, 0u8, 0u8);
        match hovered {
            Some((srdwm_core::TitlebarHit::Close, a)) => {
                close_c = brighten(close_c);
                close_glyph = a;
            }
            Some((srdwm_core::TitlebarHit::Minimize, a)) => {
                minimize_c = brighten(minimize_c);
                minimize_glyph = a;
            }
            Some((srdwm_core::TitlebarHit::Maximize, a)) => {
                maximize_c = brighten(maximize_c);
                maximize_glyph = a;
            }
            _ => {}
        }
        // Traditional (non-traffic-light) glyphs are always visible, same
        // as a real Windows/GNOME titlebar's own icons - there's no
        // filled dot drawing attention to the button at rest the way a
        // traffic light does, so hiding the glyph too, pending an explicit
        // `button_glyph = "always"`, would leave the button showing
        // nothing at all until hovered.
        let glyph_always = glyph_always || !traffic_lights;
        if glyph_always {
            close_glyph = 255;
            minimize_glyph = 255;
            maximize_glyph = 255;
        }
        // A traffic-light glyph is `darken`ed from that *same* button's own
        // (possibly already-`brighten`ed-by-hover) colour - real macOS
        // draws a dark red mark on the red button, dark amber on the
        // yellow one, not one shared tint reused across all three (see
        // `darken`'s own doc comment). A traditional glyph instead uses
        // the titlebar's actual text colour, drawn straight on the
        // titlebar's own dark background - a dark-on-dark glyph the
        // traffic-light shade uses would be unreadable there.
        let (close_shade, minimize_shade, maximize_shade) =
            if traffic_lights { (darken(close_c), darken(minimize_c), darken(maximize_c)) } else { (foreground, foreground, foreground) };
        let margin = if buttons_left { BUTTON_MARGIN_LEFT } else { BUTTON_MARGIN };
        // Closest-to-the-aligned-edge first - must stay in exact
        // agreement with `ResizeEdge::hit_test`'s own resolution of the
        // same two fields, the same "renders on one side, hit-tests on
        // the other" trap `buttons_left` alone already has to avoid. See
        // `ButtonOrder`'s own doc comment for why the two built-in
        // defaults are genuinely different relative orderings, not
        // mirrors of each other.
        // A dialog always draws Close, full stop - not just whichever
        // button a `button_order` override would otherwise put first, or
        // Minimize/Maximize could still end up the one (and only) button
        // drawn. `button_count` (1 for a dialog) caps the loop below to
        // just this first slot either way.
        let order: srdwm_core::ButtonOrder = if is_dialog {
            [srdwm_core::TitlebarButton::Close; 3]
        } else {
            button_order.unwrap_or(if buttons_left {
                [srdwm_core::TitlebarButton::Close, srdwm_core::TitlebarButton::Minimize, srdwm_core::TitlebarButton::Maximize]
            } else {
                [srdwm_core::TitlebarButton::Close, srdwm_core::TitlebarButton::Maximize, srdwm_core::TitlebarButton::Minimize]
            })
        };
        // Maximize removed from the list rather than skipped in the loop:
        // skipping would leave an empty slot where it used to be, while
        // `hit_test` closes the gap - so every later button would be drawn
        // one pitch away from where its clicks actually land.
        let order: Vec<srdwm_core::TitlebarButton> = if show_maximize {
            order.to_vec()
        } else {
            order.iter().copied().filter(|b| *b != srdwm_core::TitlebarButton::Maximize).collect()
        };
        // `BUTTON_CLUSTER_MARGIN` first, then each button's own `pitch * i`
        // spacing after it - must stay in agreement with `ResizeEdge::
        // hit_test`'s matching `left`/`right` base, the same "renders on
        // one side, hit-tests on the other" trap every other button-
        // geometry value here already has to avoid.
        for (i, button) in order.iter().take(button_count).enumerate() {
            let offset = srdwm_core::BUTTON_CLUSTER_MARGIN as usize + pitch * i;
            match button {
                srdwm_core::TitlebarButton::Close => {
                    // Traditional mode has no dot at rest - only once this
                    // button is actually the hovered one (`close_glyph > 0`,
                    // the same signal the glyph reveal itself already uses)
                    // does the neutral, brightened backdrop appear at all.
                    // A traffic light always fills, rest state included.
                    if traffic_lights || close_glyph > 0 {
                        fill_button_dot(&mut buf, width, height, offset, buttons_left, margin, close_c);
                    }
                    draw_close_glyph(&mut buf, width, height, offset, buttons_left, margin, close_glyph, close_shade);
                }
                srdwm_core::TitlebarButton::Minimize => {
                    if traffic_lights || minimize_glyph > 0 {
                        fill_button_dot(&mut buf, width, height, offset, buttons_left, margin, minimize_c);
                    }
                    draw_minimize_glyph(&mut buf, width, height, offset, buttons_left, margin, minimize_glyph, minimize_shade);
                }
                srdwm_core::TitlebarButton::Maximize => {
                    if traffic_lights || maximize_glyph > 0 {
                        fill_button_dot(&mut buf, width, height, offset, buttons_left, margin, maximize_c);
                    }
                    // The macOS "zoom" double-arrow only reads correctly
                    // paired with that same convention's traffic lights --
                    // traditional mode keeps the plain square every other
                    // desktop's own maximize icon already uses.
                    if traffic_lights {
                        draw_zoom_glyph(&mut buf, width, height, offset, buttons_left, margin, maximize_glyph, maximize_shade);
                    } else {
                        draw_maximize_glyph(&mut buf, width, height, offset, buttons_left, margin, maximize_glyph, maximize_shade);
                    }
                }
            }
        }
    }
    if round_corners {
        // Same shift both ways - see `round_top_corners`'s own doc comment
        // on `center_col`: this titlebar's buffer starts `border_width`
        // columns inside the true left/right edges the same way it starts
        // `border_width` rows inside the true top, so both centres need
        // the identical correction, not just the row one.
        let shift = radius as i32 - border_width as i32;
        round_top_corners(&mut buf, width, height, radius, shift, shift, None);
    }
    buf
}

