//! macOS backend for srdwm.
//!
//! Like `srdwm-windows`, this crate is kept in the workspace so it
//! type-checks on every target, but its `cfg(target_os = "macos")` bodies
//! have never actually been built or run (this sandbox only has the
//! `x86_64-unknown-linux-gnu` target installed) - treat everything here as a
//! design sketch, not verified code.
//!
//! macOS gives no public API to draw a custom titlebar/border on another
//! process's window, so (per the legacy C++ design, see docs/PRIOR_ART.md)
//! srdwm cannot *decorate* foreign windows the way the X11/Wayland backends
//! do. The plan carried over from the legacy code:
//!
//! 1. Real window control (move/resize/focus) goes through the
//!    Accessibility API (`AXUIElementSetAttributeValue` with
//!    `kAXPositionAttribute`/`kAXSizeAttribute`), which requires the user to
//!    grant Accessibility permission (`AXIsProcessTrustedWithOptions`).
//! 2. "Full title bar" parity is approximated with a separate, borderless,
//!    click-through **overlay window** drawn on top of the target window,
//!    reusing the same `srdwm_core::window::ResizeEdge` hit-testing as the
//!    other backends, repositioned every time the tracked window moves. The
//!    legacy C++ only stubbed this (`create_overlay_window` logged and did
//!    nothing); it is *not* implemented here either - it needs an `NSWindow`
//!    (via `objc2-app-kit` or similar) and per-window position polling or an
//!    AX notification observer, which is a substantial enough piece of work
//!    that faking it would be worse than leaving it as a documented TODO.
//! 3. Closing a window isn't a single AX attribute - it requires finding the
//!    window's close-button `AXUIElement` (`kAXCloseButtonAttribute`) and
//!    performing `kAXPressAction` on it. Also left as TODO for the same reason.
//!
//! What *is* implemented for real below: monitor enumeration via
//! `CGGetActiveDisplayList`/`CGDisplayBounds` (stable, well-documented
//! low-level Core Graphics C API), which was also the one genuinely working
//! piece of the legacy macOS backend.

use srdwm_core::{Monitor, Rect, Window, WindowId};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result};

pub struct MacOsPlatform;

impl MacOsPlatform {
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(PlatformError::Unsupported("srdwm-macos was not compiled for a macOS target"))
        }
    }
}

impl Platform for MacOsPlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::MacOS
    }

    fn poll_events(&mut self) -> Result<Vec<srdwm_core::Event>> {
        // TODO: CGEventTap (kCGSessionEventTap) for global key/mouse input,
        // as the legacy backend set up but never actually forwarded into its
        // event queue.
        Err(PlatformError::Unsupported("poll_events"))
    }

    #[cfg(target_os = "macos")]
    fn monitors(&mut self) -> Result<Vec<Monitor>> {
        use core_graphics::display::{CGDisplay, CGMainDisplayID};

        let ids = CGDisplay::active_displays().map_err(|e| PlatformError::Other(format!("CGGetActiveDisplayList failed: {e}")))?;
        let main_id = unsafe { CGMainDisplayID() };
        let monitors = ids
            .into_iter()
            .enumerate()
            .map(|(i, id)| {
                let display = CGDisplay::new(id);
                let bounds = display.bounds();
                let mut m = Monitor::new(
                    i as u32,
                    format!("display-{id}"),
                    Rect::new(bounds.origin.x as i32, bounds.origin.y as i32, bounds.size.width as u32, bounds.size.height as u32),
                );
                m.primary = id == main_id;
                m
            })
            .collect();
        Ok(monitors)
    }
    #[cfg(not(target_os = "macos"))]
    fn monitors(&mut self) -> Result<Vec<Monitor>> {
        Err(PlatformError::Unsupported("monitors"))
    }

    fn apply_geometry(&mut self, _window: WindowId, _geometry: Rect) -> Result<()> {
        // TODO: AXUIElementSetAttributeValue(kAXPositionAttribute / kAXSizeAttribute).
        Err(PlatformError::Unsupported("apply_geometry (needs AXUIElement + Accessibility permission)"))
    }

    fn set_title(&mut self, _window: WindowId, _title: &str) -> Result<()> {
        Err(PlatformError::Unsupported("set_title (macOS windows are titled by their owning app, not the WM)"))
    }

    fn focus(&mut self, _window: WindowId) -> Result<()> {
        // TODO: AXUIElementSetAttributeValue(kAXFrontmostAttribute, true) on
        // the owning application, or CGWindowListCopyWindowInfo + activate.
        Err(PlatformError::Unsupported("focus"))
    }

    fn minimize(&mut self, _window: WindowId) -> Result<()> {
        // TODO: AXUIElementSetAttributeValue(kAXMinimizedAttribute, true).
        Err(PlatformError::Unsupported("minimize"))
    }

    fn restore(&mut self, _window: WindowId) -> Result<()> {
        Err(PlatformError::Unsupported("restore"))
    }

    fn close(&mut self, _window: WindowId) -> Result<()> {
        // TODO: find kAXCloseButtonAttribute then AXUIElementPerformAction(kAXPressAction).
        Err(PlatformError::Unsupported("close"))
    }

    fn set_decorated(&mut self, _window: WindowId, _decorated: bool) -> Result<()> {
        Err(PlatformError::Unsupported("set_decorated (needs the overlay-window design, see module docs)"))
    }

    fn set_border_color(&mut self, _window: WindowId, _rgb: (u8, u8, u8)) -> Result<()> {
        Err(PlatformError::Unsupported("set_border_color (needs the overlay-window design, see module docs)"))
    }

    fn set_border_width(&mut self, _window: WindowId, _width: u32) -> Result<()> {
        Err(PlatformError::Unsupported("set_border_width (needs the overlay-window design, see module docs)"))
    }

    fn redraw_decoration(&mut self, _window: WindowId, _win: &Window, _focused: bool) -> Result<()> {
        Err(PlatformError::Unsupported("redraw_decoration (needs the overlay-window design, see module docs)"))
    }

    fn grab_keyboard(&mut self) -> Result<()> {
        Err(PlatformError::Unsupported("grab_keyboard (needs a CGEventTap + Input Monitoring permission)"))
    }

    fn ungrab_keyboard(&mut self) -> Result<()> {
        Err(PlatformError::Unsupported("ungrab_keyboard"))
    }

    fn keyboard_layout(&mut self) -> Result<String> {
        Err(PlatformError::Unsupported("keyboard_layout"))
    }

    fn cycle_keyboard_layout(&mut self) -> Result<String> {
        Err(PlatformError::Unsupported("cycle_keyboard_layout"))
    }
}
