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

/// Linear channel-wise blend of `a` toward `b` by `t` (`0.0` is pure `a`,
/// `1.0` is pure `b`) - unlike `brighten`/`darken`, which blend toward a
/// fixed white/black, this blends toward an arbitrary second colour, at
/// an arbitrary caller-chosen ratio. `render_context_menu`'s own row
/// highlight uses this to reproduce the AGS reference dropdown's own
/// subtle wash (`color-mix(in srgb, var(--primary-bg) 22%, var(--widget-
/// bg))`) instead of a flat, fully-saturated fill - the same reference
/// this project's own menu rebuild already targets elsewhere.
pub(crate) fn mix_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |ac: u8, bc: u8| (ac as f32 + (bc as f32 - ac as f32) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}
