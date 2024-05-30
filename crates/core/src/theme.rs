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
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            titlebar_bg: (0x2e, 0x34, 0x40),
            titlebar_fg_focused: (0x88, 0xc0, 0xd0),
            titlebar_fg_unfocused: (0x4c, 0x56, 0x6a),
            default_border_color: (136, 192, 208), // Nord accent, matches legacy theme default
            default_border_width: 2,
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
