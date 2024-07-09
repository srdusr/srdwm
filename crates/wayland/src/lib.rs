//! Wayland backend for srdwm: a smithay-based compositor.
//!
//! Unlike the X11 backend, this has essentially no working prior art to
//! port from - the legacy C++ `wayland_platform.cc` created the wlroots
//! backend/renderer/compositor/seat/xdg-shell objects but never wired a
//! single `wl_signal_add` listener, so no window was ever actually managed
//! (see docs/PRIOR_ART.md). This is a from-scratch implementation using
//! `smithay` (the Rust analog of wlroots), following the same object-graph
//! shape smithay's own bundled `examples/` (compositor.rs, seat.rs) use.
//!
//! Scope for this pass, chosen to keep the implementation honest about what
//! is and isn't real:
//! - Runs via smithay's **winit backend**: a nested window on the host
//!   compositor/X server, analogous to how the X11 backend was verified
//!   under Xephyr. A DRM/udev backend for running as the actual system
//!   compositor on a TTY is not implemented - see docs/IMPLEMENTATION_STATUS.md.
//! - xdg-shell toplevels are tracked exactly like the X11 backend tracks
//!   X windows: through `srdwm_core::WindowManager`, so layout, smart
//!   placement, and drag/resize hit-testing are the *same* code path as X11
//!   (`srdwm_core::window::ResizeEdge::hit_test`), not a reimplementation.
//! - Decorations are a titlebar band rendered in software (`decoration.rs`:
//!   solid background plus the actual window title, rasterized via
//!   `fontdue` against whatever monospace font is found on the system) and
//!   uploaded per-frame through smithay's `MemoryRenderBuffer`, the same
//!   band geometry and button hit-testing as the X11 backend's drawn
//!   titlebar.
//! - Global keybindings are matched precisely: `WaylandPlatform::connect`
//!   takes the config's actual bound-key combo strings (the same
//!   `"Mod4+Shift+Return"` format `srd.bind` uses and the X11 backend grabs
//!   via `XGrabKey`), and every keypress is translated to that same combo
//!   string (via the keysym table shared with X11, `srdwm_core::keysyms`)
//!   and checked against the set. Only a match is withheld from the focused
//!   client; everything else is forwarded, mirroring X11's grab-specific-keys
//!   behavior instead of the coarser "any Super-held key is ours" heuristic
//!   an earlier pass used.
//! - xdg-decoration is forced to server-side mode (`Mode::ServerSide`) so
//!   well-behaved clients don't also draw their own client-side titlebar.

mod context_menu;
mod cursor;
mod decoration;
mod elements;
mod foreign_toplevel;
mod gamma_control;
mod gtk_shell;
mod gtk_shell_protocol;
mod input;
mod lock;
mod output_management;
mod output_power;
mod protocols;
mod rounded_corners;
mod rounded_corners_pixman;
mod screencopy;
mod state;
mod udev;
mod winit;
mod workspace;
mod xkb_config;
mod xwayland;

use std::cell::RefCell;
use std::rc::Rc;

use srdwm_core::WindowManager;
use srdwm_platform::{Platform, PlatformError, Result as PlatformResult};

pub use winit::WaylandPlatform;

/// Shared error shim: every backend turns foreign errors into
/// `PlatformError::Other` the same way.
pub(crate) fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(e.to_string())
}

/// Connects to Wayland, choosing between the udev/DRM backend (bare TTY, no
/// host compositor to nest under - see `udev.rs`) and this module's winit
/// backend (nested window), the same way real compositors decide
/// nested-vs-native. Falls back to winit if udev initialization fails for
/// any reason (no seat access, no DRM device, ...), logging why rather than
/// failing outright.
pub fn connect(wm: Rc<RefCell<WindowManager>>, bound_keys: &[String], repeat_keys: &[String]) -> PlatformResult<Box<dyn Platform>> {
    let no_host_display = std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none();
    if no_host_display {
        match udev::UdevPlatform::connect(wm.clone(), bound_keys, repeat_keys) {
            Ok(platform) => return Ok(Box::new(platform)),
            Err(e) => log::warn!("udev/DRM backend unavailable ({e}); falling back to nested winit backend"),
        }
    }
    Ok(Box::new(WaylandPlatform::connect(wm, bound_keys, repeat_keys)?))
}
