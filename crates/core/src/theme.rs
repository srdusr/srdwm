/// Default decoration colours and border width, applied to every window at
/// creation (before rules run, so a rule's own `border_color`/`border_width`
/// still wins - see `WindowManager::add_window`) and read live by a
/// backend's titlebar rendering.
///
/// Read from `theme.colors.*`/`theme.decorations.*` in `crates/srdwm/src/
/// main.rs`'s `apply_general_settings`. Before that wiring existed, these
/// were hardcoded Rust constants scattered across `crates/wayland` (the
/// Nord palette every default here still matches, so an unconfigured
/// session looks identical to before) - found the same way `window_gap`
/// and `general.animations` were: config already validated/defaulted these
/// keys, nothing ever read them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeConfig {
    pub titlebar_bg: (u8, u8, u8),
    pub titlebar_fg_focused: (u8, u8, u8),
    pub titlebar_fg_unfocused: (u8, u8, u8),
    pub default_border_color: (u8, u8, u8),
    /// `4`, not the `2` this used to default to - at `2`, the border
    /// strip's own rounded-corner cut (`decoration::render_border_top`/
    /// `_bottom`, continuing the titlebar's larger radius outward) only
    /// ever had two rows of pixels to draw an arc into, which - even
    /// anti-aliased (`decoration::blend_corner_pixel`) - reads as barely
    /// more than a single soft pixel, not a curve. That's most visible on
    /// an undecorated/CSD window (no compositor-drawn titlebar to anchor
    /// a bigger curve nearby, Firefox concretely): reported live as
    /// "not all windows curved". Twice the rows makes the same curve
    /// actually legible without touching content rounding at all, which
    /// stays a real, deliberate per-backend cost/default tradeoff (see
    /// `rounded_corners_pixman`'s module doc comment) rather than
    /// something to paper over with a thicker border.
    pub default_border_width: u32,
    /// Titlebar/border-strip corner radius, in logical pixels - the same
    /// value `Window::corner_radius` copies onto every window at creation
    /// (see `WindowManager::add_window`), which a rule's own `corner_radius`
    /// action can still override afterward, same as `default_border_width`.
    /// `12`, not the original `6`: matches real macOS's own ~0.36 radius-to-
    /// titlebar-height proportion rather than this project's original,
    /// visibly tighter `0.2` (docs/TODO.md's macOS-comparison research).
    /// Moves in step with `TITLEBAR_HEIGHT` (currently `32`, matched
    /// directly against a live Firefox window) to keep that same ratio,
    /// not a separate size decision of its own.
    pub default_corner_radius: u32,
    /// Whether a newly created window gets srdwm's own titlebar
    /// (server-side decoration) by default, before any `xdg-decoration`
    /// negotiation or rule gets a say. Also what the Wayland backend
    /// initially *offers* a client that creates a decoration object but
    /// has no strong preference of its own (`XdgDecorationHandler::
    /// new_decoration`) - a client that explicitly asks for the other
    /// mode is still honored regardless of this value (see that handler's
    /// own doc comment).
    ///
    /// `true` (server-side) is srdwm's own longstanding default, matching
    /// the Windows/macOS-style consistent OS-drawn chrome this compositor
    /// is going for - and, among real desktop environments that still
    /// have titlebars at all, KDE/KWin's own choice too (confirmed via
    /// research, not assumed: KWin supports both and defaults to
    /// server-side). `false` (client-side) matches GNOME/Mutter's
    /// approach instead - srdwm steps back and lets every window draw its
    /// own chrome, including ones with no titlebar opinion of their own,
    /// which then get none at all. Live-settable (`srd set decoration_mode
    /// server|client`) specifically so both can be A/B tested against a
    /// real, broad set of installed apps rather than guessed at from two
    /// examples - see `theme.decorations.default_mode` in the Lua config
    /// for the persistent equivalent.
    pub default_decorated: bool,
    /// How much an unfocused window's border is dimmed from its own
    /// configured colour - `1.0` keeps it identical to focused, `0.0`
    /// removes the border entirely when unfocused. Was a hardcoded `0.35`
    /// constant in `state::effective_border_color` with no way to change it
    /// at all; `theme.decorations.border.inactive_dim` in the Lua config is
    /// the first way to actually reach it, closing the exact gap
    /// `apply_general_settings`'s own doc comment flagged (`border.
    /// inactive_color` was left unwired because setting an *explicit*
    /// colour would silently erase the dimming scheme for anyone who never
    /// touched it - a *factor* on top of the same scheme has no such
    /// footgun: the unconfigured default below reproduces the old
    /// hardcoded behaviour exactly).
    pub border_inactive_dim: f32,
    /// Centers the titlebar's title text instead of the longstanding
    /// left-aligned default - `theme.decorations.title_bar.text_align`
    /// in the Lua config (`"center"` sets this; anything else, including
    /// unset, keeps left-aligned). Explicitly requested as its own
    /// config knob, not a hardcoded switch - macOS centers title text by
    /// convention, GNOME/Windows both left-align, so neither is a
    /// universal default worth forcing.
    pub title_centered: bool,
    /// Titlebar buttons on the left (macOS convention: close, minimize,
    /// maximize, left to right) instead of the longstanding right-aligned
    /// default (Windows/GTK convention: minimize, maximize, close) --
    /// `theme.decorations.title_bar.button_side` in the Lua config
    /// (`"left"` sets this; anything else, including unset, keeps
    /// right-aligned). Researched against mutter's own `button-layout`
    /// GSettings key before choosing this shape (one config value, not a
    /// bespoke per-button scheme) - see `docs/TODO.md`.
    ///
    /// This has to stay in perfect agreement with `ResizeEdge::hit_test`'s
    /// own `buttons_left` parameter, not just `decoration::render_titlebar`'s
    /// rendering - a button that renders on one side but hit-tests on the
    /// other is worse than not being configurable at all, since every
    /// click would silently miss.
    pub buttons_left: bool,
    /// An explicit `close,minimize,maximize`-style override for the three
    /// buttons' relative order, applied to whichever side `buttons_left`
    /// already selects - `theme.decorations.button_order` in the Lua
    /// config, parsed by `window::parse_button_order`. `None` (the
    /// default, unset) keeps this project's own two built-in defaults
    /// exactly as they were before this field existed.
    ///
    /// Added after `buttons_left`'s own doc comment above had already
    /// deliberately chosen "one config value, not a bespoke per-button
    /// scheme" - revisited once a real comparison against KWin's
    /// `ButtonsOnLeft`/`ButtonsOnRight`, GNOME/Adwaita's own `decoration-
    /// layout` (confirmed, contrary to this project's own earlier
    /// assumption from Mutter's C source alone, to be a real per-button
    /// ordering string, not just a fixed convention), and Openbox's
    /// `titlelayout` found all three independently converged on exactly
    /// this shape. Additive, not a reversal: `buttons_left` still exists
    /// and still means what it always did.
    pub button_order: Option<crate::window::ButtonOrder>,
    /// The titlebar button glyph (dash/square/X) is always drawn instead
    /// of only fading in on hover - `theme.decorations.title_bar.
    /// button_glyph` in the Lua config (`"always"` sets this; anything
    /// else, including unset, keeps the animated hover-reveal default).
    ///
    /// Researched (DE-weighted, per explicit request) before defaulting to
    /// hover-reveal: real, extracted libadwaita CSS on this machine
    /// (`gresource extract` on the installed `.so`, not guessed) shows
    /// current GNOME/Adwaita actually keeps the glyph always visible and
    /// only animates the background circle's opacity on hover - the
    /// "always" mode here matches that. Classic macOS instead hides the
    /// glyph entirely at rest and animates it in on hover - the default,
    /// per this project's own explicit choice between the two once told
    /// they're genuinely different conventions, not the same thing
    /// assumed two different ways.
    pub button_glyph_always: bool,
    /// Whether the three titlebar buttons render as filled, coloured
    /// macOS-style traffic lights, or as plain glyphs directly on the
    /// titlebar's own background - `theme.decorations.title_bar.
    /// button_style` in the Lua config (`"traditional"` sets this to
    /// `false`; anything else, including unset, keeps the traffic-light
    /// default).
    ///
    /// Explicitly requested as a separate axis from `buttons_left`: the
    /// two defaulted to moving together (macOS convention is traffic
    /// lights on the left; this project's own original look was plain
    /// glyphs on the right), but neither implies the other - a caller can
    /// still combine plain glyphs with left-aligned buttons or the reverse.
    /// `false` swaps two things together, both handled in `decoration::
    /// render_titlebar`: no `fill_button_dot` call except a subtle, neutral
    /// hover backdrop (there's no coloured circle to brighten on hover
    /// instead), and the glyphs themselves draw in `foreground` (this
    /// project's original look; readable straight on the titlebar's own
    /// dark background) rather than the near-black shade a traffic light's
    /// own bright fill needs instead. Maximize also draws as a plain
    /// square glyph rather than the macOS "zoom" double-arrow, matching
    /// this convention's own (Windows/GNOME) maximize icon rather than
    /// borrowing the other convention's.
    pub traffic_light_buttons: bool,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            titlebar_bg: (0x2e, 0x34, 0x40),
            titlebar_fg_focused: (0x88, 0xc0, 0xd0),
            titlebar_fg_unfocused: (0x4c, 0x56, 0x6a),
            default_border_color: (136, 192, 208), // Nord accent, matches legacy theme default
            default_border_width: 4,
            default_corner_radius: 12,
            default_decorated: true,
            border_inactive_dim: 0.35,
            title_centered: false,
            buttons_left: false,
            button_order: None,
            button_glyph_always: false,
            traffic_light_buttons: true,
        }
    }
}

/// Parses a `"#rrggbb"` string into its channels. Returns `None` for
/// anything else - `crates/config` already validates this shape at load
/// time (`is_valid_hex_color`) and logs a warning for a malformed value, so
/// a caller here can fall back to a default silently rather than erroring
/// a second time.
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() != 7 || !s.starts_with('#') {
        return None;
    }
    let r = u8::from_str_radix(&s[1..3], 16).ok()?;
    let g = u8::from_str_radix(&s[3..5], 16).ok()?;
    let b = u8::from_str_radix(&s[5..7], 16).ok()?;
    Some((r, g, b))
}

/// [`parse_hex_color`]'s exact inverse - `#rrggbb`, lowercase, always six
/// hex digits (`{:02x}` per channel, so a channel below `0x10` doesn't
/// collapse to a five-character string). Exists for settings readback: a
/// caller reading `border_color` back over IPC should get the identical
/// string shape `srd set border_color` itself accepts, not a different
/// representation of the same colour.
pub fn format_hex_color(rgb: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_hex_color() {
        assert_eq!(parse_hex_color("#88c0d0"), Some((0x88, 0xc0, 0xd0)));
    }

    #[test]
    fn rejects_missing_hash_or_wrong_length() {
        assert_eq!(parse_hex_color("88c0d0"), None);
        assert_eq!(parse_hex_color("#88c0d"), None);
        assert_eq!(parse_hex_color("#88c0d00"), None);
    }

    #[test]
    fn rejects_non_hex_digits() {
        assert_eq!(parse_hex_color("#zzzzzz"), None);
    }

    #[test]
    fn format_hex_color_round_trips_through_parse_hex_color() {
        for rgb in [(0x88, 0xc0, 0xd0), (0, 0, 0), (0xff, 0xff, 0xff), (0x05, 0x0a, 0x0f)] {
            assert_eq!(parse_hex_color(&format_hex_color(rgb)), Some(rgb));
        }
    }

    #[test]
    fn format_hex_color_pads_low_channel_values() {
        assert_eq!(format_hex_color((0x05, 0x0a, 0x0f)), "#050a0f");
    }

    #[test]
    fn default_matches_the_legacy_hardcoded_nord_palette() {
        let t = ThemeConfig::default();
        assert_eq!(t.titlebar_bg, (0x2e, 0x34, 0x40));
        assert_eq!(t.titlebar_fg_focused, (0x88, 0xc0, 0xd0));
        assert_eq!(t.default_border_color, (136, 192, 208));
    }
}
