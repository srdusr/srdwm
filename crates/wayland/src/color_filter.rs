//! Whole-screen colour treatments (`srd set night_light`/`srd set
//! reading_mode`), ported from a Hyprland setup that pointed its
//! `decoration:screen_shader` at a small GLSL fragment shader - one that
//! multiplied every pixel by a warm tint, the other that flattened every
//! pixel to its own luminance.
//!
//! Neither backend has an equivalent hook: the udev backend's
//! `PixmanRenderer` is software-only (see `blur.rs`'s own doc comment),
//! and reproducing the effect faithfully even on the GLES/winit backend
//! would mean a full-frame capture + per-pixel transform + re-import on
//! every damaged frame - the same per-frame CPU cost this codebase has
//! already measured and rejected once for something far smaller (see
//! `rounded_corners_pixman`'s module doc comment, and `udev/render.rs`'s
//! own note on why that backend defaults corner-rounding off: "a full
//! row-by-row buffer copy on every commit of a constantly-repainting
//! client", named video specifically as the cost case that mattered).
//! A whole-output tint is that same cost applied to *every* pixel of
//! *every* frame, not just a window's corners.
//!
//! Instead, both effects are approximated with a single translucent
//! `SolidColorRenderElement` covering the output, alpha-blended over the
//! real scene by the renderer's native (and therefore free) `Frame::
//! draw_solid` - no readback, no per-pixel work, no texture import.
//! Blending any colour with a fixed colour is mathematically a pull
//! toward that colour on every channel, which is close enough to both
//! source shaders' actual intent (night light: pull blue/green down more
//! than red; reading mode: pull every channel toward flat gray) to read
//! as the same effect, at a cost indistinguishable from one extra
//! ordinary border strip.

use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::Color32F;
use smithay::utils::Point;

use srdwm_core::ColorFilter;

/// `(r, g, b, a)`, each `0.0..=1.0`, for the given filter - `None` for
/// [`ColorFilter::None`], meaning "draw nothing".
///
/// Night light's warm colour (255, 166, 87) and reading mode's neutral
/// gray (128, 128, 128) are standard picks for this exact overlay trick
/// (the same warm RGB triple Redshift/f.lux converge on for a low colour
/// temperature); the alphas were picked by eye against the ported
/// shaders' own strength - night light stays subtle (it runs for hours),
/// reading mode is deliberately stronger (it is opted into for a single
/// focused task).
fn overlay_rgba(filter: ColorFilter) -> Option<(f32, f32, f32, f32)> {
    match filter {
        ColorFilter::None => None,
        ColorFilter::NightLight => Some((1.0, 0.65, 0.34, 0.35)),
        ColorFilter::ReadingMode => Some((0.5, 0.5, 0.5, 0.55)),
    }
}

/// Builds the full-output overlay element for `filter`, or `None` for
/// [`ColorFilter::None`] (nothing to draw). `buf` must be a *persistent*
/// buffer kept one-per-output across frames, exactly like `elements::
/// border_side_render_element`'s own `buf` parameter - a fresh
/// `SolidColorBuffer` every frame gets a fresh `Id`, which defeats the
/// damage tracker's element cache and marks the whole output damaged
/// forever (see that function's doc comment for the full mechanism, and
/// why border strips themselves used to have this exact bug).
pub(crate) fn render_element(buf: &mut SolidColorBuffer, filter: ColorFilter, size: (i32, i32)) -> Option<SolidColorRenderElement> {
    let (r, g, b, a) = overlay_rgba(filter)?;
    buf.update(size, Color32F::new(r, g, b, a));
    Some(SolidColorRenderElement::from_buffer(buf, Point::from((0, 0)), 1.0, 1.0, Kind::Unspecified))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_draws_nothing() {
        assert_eq!(overlay_rgba(ColorFilter::None), None);
    }

    #[test]
    fn night_light_pulls_blue_down_the_most_and_red_the_least() {
        let (r, g, b, _) = overlay_rgba(ColorFilter::NightLight).unwrap();
        assert!(r > g && g > b, "warm tint should redden more than it greens, and green more than it blues");
    }

    #[test]
    fn reading_mode_is_neutral_gray() {
        let (r, g, b, _) = overlay_rgba(ColorFilter::ReadingMode).unwrap();
        assert_eq!((r, g, b), (0.5, 0.5, 0.5));
    }

    #[test]
    fn every_variant_alpha_is_a_real_blend_not_opaque_or_invisible() {
        for filter in [ColorFilter::NightLight, ColorFilter::ReadingMode] {
            let (.., a) = overlay_rgba(filter).unwrap();
            assert!(a > 0.0 && a < 1.0, "{filter:?}'s overlay must still let the real scene show through");
        }
    }
}
