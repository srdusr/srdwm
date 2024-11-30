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
    pub default_border_width: u32,
    /// Titlebar/border-strip corner radius, in logical pixels - the same
    /// value `Window::corner_radius` copies onto every window at creation
    /// (see `WindowManager::add_window`), which a rule's own `corner_radius`
    /// action can still override afterward, same as `default_border_width`.
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
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            titlebar_bg: (0x2e, 0x34, 0x40),
            titlebar_fg_focused: (0x88, 0xc0, 0xd0),
            titlebar_fg_unfocused: (0x4c, 0x56, 0x6a),
            default_border_color: (136, 192, 208), // Nord accent, matches legacy theme default
            default_border_width: 2,
            default_corner_radius: 6,
            default_decorated: true,
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
    fn default_matches_the_legacy_hardcoded_nord_palette() {
        let t = ThemeConfig::default();
        assert_eq!(t.titlebar_bg, (0x2e, 0x34, 0x40));
        assert_eq!(t.titlebar_fg_focused, (0x88, 0xc0, 0xd0));
        assert_eq!(t.default_border_color, (136, 192, 208));
    }
}
