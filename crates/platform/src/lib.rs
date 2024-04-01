//! Platform abstraction: the trait every backend (X11, Wayland, Windows,
//! macOS) implements, plus auto-detection of which backend to use.
//!
//! This mirrors the intent of the legacy C++ `Platform` interface
//! (see docs/PRIOR_ART.md) with one deliberate change: `poll_events` is the
//! only place backends need to bridge their native event model (X11's
//! blocking `XNextEvent`, Wayland's callback-driven dispatch, Win32's message
//! pump, macOS's event taps) into the common [`srdwm_core::Event`] queue -
//! everything downstream of that is platform-independent.

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
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").map(|v| v == "wayland").unwrap_or(false);
        if wayland {
            PlatformKind::Wayland
        } else {
            PlatformKind::X11
        }
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
}
