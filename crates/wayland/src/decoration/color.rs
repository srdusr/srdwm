//! Small, dependency-free colour helpers shared across every other
//! `decoration` submodule - converting to the renderer's own byte order,
//! and the two directional blends (`brighten`/`darken`) button/glyph
//! shading needs. Nothing here reads a pixel buffer or knows what a
//! titlebar/border/shadow is; see `buttons.rs` for the module that actually
//! applies these.

pub(crate) fn rgb_to_bgra(rgb: (u8, u8, u8), alpha: u8) -> [u8; 4] {
    [rgb.2, rgb.1, rgb.0, alpha]
}

/// Lightens a button colour for the hover state - see `render_titlebar`'s
/// `hovered` parameter. Blends toward white rather than just scaling each
/// channel up, so a fully-saturated channel (e.g. green's `0x00` blue) still
/// visibly brightens instead of clamping at its own max with nothing left
/// to move.
pub(crate) fn brighten(color: (u8, u8, u8)) -> (u8, u8, u8) {
    const AMOUNT: f32 = 0.35;
    let mix = |c: u8| (c as f32 + (255.0 - c as f32) * AMOUNT).round() as u8;
    (mix(color.0), mix(color.1), mix(color.2))
}

/// Darkens a colour toward black by a fixed fraction - used for a traffic-
/// light glyph's own shade (see `render_titlebar`'s call site): real macOS
/// draws each button's glyph as a *darker shade of that same button's own
/// hue* (a dark red mark on the red button, dark amber on the yellow one),
/// not one universal near-black tint reused across all three - reported
/// live as looking too dark/heavy and not colour-matched once compared
/// directly against a real hover-glyph screenshot.
pub(crate) fn darken(color: (u8, u8, u8)) -> (u8, u8, u8) {
    const AMOUNT: f32 = 0.45;
    let mix = |c: u8| (c as f32 * (1.0 - AMOUNT)).round() as u8;
    (mix(color.0), mix(color.1), mix(color.2))
}
