//! Platform abstraction: the trait every backend (X11, Wayland, Windows,
//! macOS) implements, plus auto-detection of which backend to use.
//!
//! This mirrors the intent of the legacy C++ `Platform` interface
//! (see docs/PRIOR_ART.md) with one deliberate change: `poll_events` is the
//! only place backends need to bridge their native event model (X11's
//! blocking `XNextEvent`, Wayland's callback-driven dispatch, Win32's message
//! pump, macOS's event taps) into the common [`srdwm_core::Event`] queue -
//! everything downstream of that is platform-independent.

mod appmenu_registrar;
pub use appmenu_registrar::{AppmenuRegistrarState, RegistrarEvent};

mod ipc;
pub use ipc::{replay_live_settings, IpcServer};

#[cfg(unix)]
mod pam_auth;
#[cfg(unix)]
pub use pam_auth::authenticate;

use srdwm_core::{Monitor, Rect, Window, WindowId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    X11,
    Wayland,
    Windows,
    MacOS,
}

impl PlatformKind {
    pub fn name(self) -> &'static str {
        match self {
            PlatformKind::X11 => "x11",
            PlatformKind::Wayland => "wayland",
            PlatformKind::Windows => "windows",
            PlatformKind::MacOS => "macos",
        }
    }
}

/// Chooses a backend the way the legacy `PlatformFactory` did: prefer
/// Wayland when a compositor is reachable, fall back to X11, otherwise use
/// the compile-time native backend on Windows/macOS.
///
/// X11 is only chosen when there's actual evidence of a running X server
/// (`DISPLAY` set) and no Wayland evidence - `srdwm_x11::X11Platform`
/// only ever *connects to* an existing server (Xephyr, or the real system
/// Xorg started separately), it never spawns one itself. Every other case,
/// including a bare TTY with neither env var set, resolves to Wayland:
/// `srdwm_wayland::connect` is the only backend that can run standalone
/// there, via its udev/DRM backend.
pub fn detect() -> PlatformKind {
    #[cfg(target_os = "windows")]
    {
        return PlatformKind::Windows;
    }
    #[cfg(target_os = "macos")]
    {
        return PlatformKind::MacOS;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        detect_unix(
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
            std::env::var("XDG_SESSION_TYPE").map(|v| v == "wayland").unwrap_or(false),
            std::env::var_os("DISPLAY").is_some(),
        )
    }
}

/// The env-var decision logic behind [`detect`]'s unix branch, pulled out
/// as a pure function so it's testable without mutating real process env
/// vars (which would be racy across parallel test threads).
#[cfg(all(unix, not(target_os = "macos")))]
fn detect_unix(wayland_display: bool, xdg_session_type_wayland: bool, display: bool) -> PlatformKind {
    let wayland_evidence = wayland_display || xdg_session_type_wayland;
    if display && !wayland_evidence {
        PlatformKind::X11
    } else {
        PlatformKind::Wayland
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_set_alone_picks_x11() {
        assert_eq!(detect_unix(false, false, true), PlatformKind::X11);
    }

    #[test]
    fn bare_tty_with_nothing_set_picks_wayland() {
        assert_eq!(detect_unix(false, false, false), PlatformKind::Wayland);
    }

    #[test]
    fn wayland_display_set_picks_wayland_even_if_display_also_set() {
        // XWayland-style setups often have both DISPLAY and WAYLAND_DISPLAY
        // set; Wayland should win.
        assert_eq!(detect_unix(true, false, true), PlatformKind::Wayland);
    }

    #[test]
    fn xdg_session_type_wayland_picks_wayland() {
        assert_eq!(detect_unix(false, true, false), PlatformKind::Wayland);
    }
}

#[derive(thiserror::Error, Debug)]
pub enum PlatformError {
    #[error("failed to connect to display server: {0}")]
    ConnectionFailed(String),
    #[error("another window manager is already running")]
    AnotherWmRunning,
    #[error("unsupported operation on this platform: {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// A platform backend: owns the connection to the display server and
/// translates between real surfaces/windows and srdwm-core's `Window` state.
pub trait Platform {
    fn kind(&self) -> PlatformKind;

    /// Blocks until at least one event is available (or a short timeout
    /// elapses), then drains everything currently pending.
    fn poll_events(&mut self) -> Result<Vec<srdwm_core::Event>>;

    fn monitors(&mut self) -> Result<Vec<Monitor>>;

    fn apply_geometry(&mut self, window: WindowId, geometry: Rect) -> Result<()>;
    fn set_title(&mut self, window: WindowId, title: &str) -> Result<()>;
    fn focus(&mut self, window: WindowId) -> Result<()>;
    fn minimize(&mut self, window: WindowId) -> Result<()>;
    fn restore(&mut self, window: WindowId) -> Result<()>;
    fn close(&mut self, window: WindowId) -> Result<()>;

    fn set_decorated(&mut self, window: WindowId, decorated: bool) -> Result<()>;
    fn set_border_color(&mut self, window: WindowId, rgb: (u8, u8, u8)) -> Result<()>;
    fn set_border_width(&mut self, window: WindowId, width: u32) -> Result<()>;

    /// Redraws the decoration (titlebar + buttons) for `window`, e.g. after
    /// a focus change or resize. No-op on platforms with native
    /// decorations (Windows/macOS).
    fn redraw_decoration(&mut self, window: WindowId, win: &Window, focused: bool) -> Result<()>;

    fn grab_keyboard(&mut self) -> Result<()>;
    fn ungrab_keyboard(&mut self) -> Result<()>;

    /// The active XKB layout's own human-readable name (e.g. `"English
    /// (US)"`) - read once at startup so `WindowManager::keyboard_layout`
    /// (surfaced over `srd`, for an AGS peer session's keyboard-layout
    /// badge) has a real value before the first cycle, not an empty string
    /// until the user cycles once. `Err(Unsupported)` on a backend with no
    /// real XKB-backed seat to ask (X11, and the honest-stub Windows/macOS
    /// backends) - same convention `grab_keyboard` already uses for "this
    /// capability genuinely doesn't exist here" rather than inventing a
    /// second one.
    fn keyboard_layout(&mut self) -> Result<String>;

    /// Cycles to the next configured XKB layout (wrapping past the last
    /// one back to the first) and returns its name. A no-op that returns
    /// the same name back is correct, not a bug, when only one layout is
    /// configured - there's nothing to cycle *to*, same as every other
    /// desktop's layout switcher under the same condition.
    fn cycle_keyboard_layout(&mut self) -> Result<String>;
}
