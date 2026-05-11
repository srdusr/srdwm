//! Configuration for srdwm's own session-lock UI
//! (`crates/wayland/src/native_lock.rs` does the actual rendering/input;
//! this is just the live-configurable knobs, same split `ThemeConfig`
//! already has between itself and the decoration-rendering code that
//! reads it).
//!
//! Not folded into `ThemeConfig` itself: that struct is `Copy` (read by
//! value on every decoration redraw), and a couple of these fields
//! (`fail_message`) need to be `String`/heap-allocated, which would force
//! every `ThemeConfig` copy to become a clone instead - cheap enough
//! given how rarely lock config is actually read (once per lock, not once
//! per frame), but no reason to pay that cost on every titlebar repaint
//! too.

/// Read from `theme.lock.*` in `crates/srdwm/src/main.rs`, and (like
/// `ThemeConfig`) live-settable via `srd set` - see `crates/platform/src/
/// ipc.rs`'s `lock_*` keys.
#[derive(Debug, Clone, PartialEq)]
pub struct LockConfig {
    pub box_bg: (u8, u8, u8),
    /// How opaque the password box's own panel is, `0.0`-`1.0`.
    ///
    /// `0.0` by default: no panel, no border, no rounded rectangle - just
    /// the password dots and status text over whatever the lock background
    /// already is (the blurred wallpaper, or whatever the user set).
    /// Reported live as "there is still a weird box where password is
    /// typed. should just be the blurred background or whatever user set.
    /// not that horrendus box".
    ///
    /// Raising it brings the panel back at that opacity, using `box_bg`
    /// and `box_border` as before, for anyone who wants a solid field to
    /// type into. It is a float rather than a bool so a faint scrim - the
    /// usual compromise when a wallpaper is too busy to read text against
    /// - is reachable without a second setting.
    pub box_opacity: f32,
    pub box_border: (u8, u8, u8),
    pub text_color: (u8, u8, u8),
    pub error_color: (u8, u8, u8),
    pub corner_radius: u32,
    /// Box-blur radius applied to the captured pre-lock screen content, in
    /// pixels - see `native_lock.rs`'s `box_blur` for why this is a box
    /// blur, not a true Gaussian one. `0` disables blurring entirely
    /// (just the captured content, unmodified) rather than being clamped
    /// up to some minimum - a legitimate configuration for a low-power
    /// device, not a mistake to guard against.
    pub blur_radius: u32,
    /// Drawn once per character typed, never the character itself.
    pub dot_char: char,
    pub show_caps_lock: bool,
    pub show_failed_attempts: bool,
    pub fail_message: String,
    /// A large time+date readout above the password box, plus a circular
    /// initial-letter avatar and the username - the set of things every
    /// mainstream lock screen (GNOME, macOS, Windows) shows and this one
    /// didn't, reported live as the box on its own "looks ugly/AI-like".
    /// `true` by default; `false` reduces the lock screen to just the
    /// password box, the previous look, for anyone who'd rather not have
    /// the time visible on a locked screen.
    pub show_clock: bool,
    /// An on-screen keyboard below the password box, for a session with no
    /// physical keyboard reachable (a touchscreen device, primarily) --
    /// see `native_lock.rs`'s own `render_keyboard`/`keyboard_hit_test`.
    /// `true` by default; a real physical keyboard still works identically
    /// either way, so this only ever adds a second input method, never
    /// removes the first.
    pub show_keyboard: bool,
    /// The avatar circle's fill colour - defaults to `box_border` (the
    /// same accent every other lock-screen element already uses) rather
    /// than a third independent colour to keep track of.
    pub avatar_bg: (u8, u8, u8),
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            box_bg: (0x2e, 0x34, 0x40),        // Nord dark, matches ThemeConfig::titlebar_bg
            box_opacity: 0.0,                  // no panel at all - see the field's own doc comment
            box_border: (0x88, 0xc0, 0xd0),    // Nord blue, matches ThemeConfig::default_border_color
            text_color: (0xec, 0xef, 0xf4),    // Nord light
            error_color: (0xbf, 0x61, 0x6a),   // Nord red, matches the theme's own `error` colour
            corner_radius: 10,
            blur_radius: 20,
            dot_char: '\u{25cf}', // "●"
            show_caps_lock: true,
            show_failed_attempts: true,
            fail_message: "Wrong password".to_string(),
            show_clock: true,
            show_keyboard: true,
            avatar_bg: (0x88, 0xc0, 0xd0), // Nord blue, matches box_border
        }
    }
}
