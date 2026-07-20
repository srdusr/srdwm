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
//! - xdg-decoration offers server-side mode by default (`theme.
//!   default_decorated`/`srd set decoration_mode`), but a client that
//!   explicitly requests client-side is honored rather than overridden --
//!   see `XdgDecorationHandler::request_mode` in `protocols.rs` for why
//!   forcing server-side unconditionally used to give some clients (Firefox,
//!   concretely) two overlapping sets of window buttons.

mod appmenu;
mod blur;
mod color_filter;
mod context_menu;
mod snap_flyout;
mod cursor;
mod decoration;
mod desktop_icons;
mod desktop_icons_state;
mod desktop_menu;
mod elements;
mod foreign_toplevel;
mod gamma_control;
mod gtk_shell;
mod gtk_shell_protocol;
mod icon_theme;
mod input;
mod lock;
mod monitor_layout;
mod native_lock;
mod output_management;
mod output_power;
mod protocols;
mod rounded_corners;
mod rounded_corners_pixman;
mod screencopy;
mod state;
mod trash;
mod udev;
mod virtual_pointer;
mod window_memory;
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
/// host compositor to nest under - see `udev`) and this module's winit
/// backend (nested window), the same way real compositors decide
/// nested-vs-native. Falls back to winit if udev initialization fails for
/// any reason (no seat access, no DRM device, ...), logging why rather than
/// failing outright.
static NESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True when this srdwm runs nested inside another compositor's session --
/// a window on someone else's desktop - rather than owning the machine's
/// own outputs.
///
/// Anything that writes to the *user's own desktop configuration or state*
/// must be gated on this. A nested instance is a test or development
/// window: it shares `HOME` with the real session, but it is not the shell,
/// and it has no business rewriting settings the real session is using.
/// Learned twice - a nested run rewrote the real
/// `~/.config/gtk-3.0/srdwm-buttons.css` from its scratch config's
/// defaults, changing every GTK app's window buttons in the live session;
/// and `window_memory::save_all` would do the same to where every
/// application opens.
///
/// Recorded by `connect` at the moment the backend is chosen, and read
/// afterward, because it cannot be re-derived later: both backends set
/// `WAYLAND_DISPLAY` on themselves once they bind their own socket, so
/// "is there a WAYLAND_DISPLAY" answers yes for a real udev session too
/// the moment it is up.
pub fn running_nested() -> bool {
    NESTED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn connect(wm: Rc<RefCell<WindowManager>>, bound_keys: &[String], repeat_keys: &[String]) -> PlatformResult<Box<dyn Platform>> {
    let no_host_display = std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none();
    if no_host_display {
        match udev::UdevPlatform::connect(wm.clone(), bound_keys, repeat_keys) {
            Ok(platform) => return Ok(Box::new(platform)),
            Err(e) => log::warn!("udev/DRM backend unavailable ({e}); falling back to nested winit backend"),
        }
    }
    // Only the winit backend is reached from here, and it is reached only
    // when there is a host session to nest inside - either one was found
    // above, or udev failed and this is a fallback into someone else's
    // session. Set before `WaylandPlatform::connect`, which binds a socket
    // and overwrites `WAYLAND_DISPLAY` with its own.
    NESTED.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(Box::new(WaylandPlatform::connect(wm, bound_keys, repeat_keys)?))
}
