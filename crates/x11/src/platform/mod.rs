//! X11 backend for srdwm: a classic reparenting window manager that draws
//! its own title bar (close/maximize/minimize buttons, drag-to-move,
//! edge/corner resize) rather than relying on any toolkit's decorations -
//! this is what gives "full title bar support" parity with Windows/macOS on
//! X11.
//!
//! Compared to the legacy C++ `x11_platform.cc` (see docs/PRIOR_ART.md),
//! this fixes several real bugs rather than porting them:
//! - the titlebar was drawn at a hardcoded 800px width; here it's sized to
//!   the actual frame width every time (see `redraw_decoration`).
//! - there was no drag/resize/button hit-testing at all; here it's the
//!   shared `srdwm_core::window::ResizeEdge::hit_test` used by every backend.
//! - `check_for_other_wm` always returned `true` because its error handler
//!   discarded errors; here we do a *checked* `change_window_attributes`
//!   with `SUBSTRUCTURE_REDIRECT` and propagate a real `BadAccess` as
//!   [`PlatformError::AnotherWmRunning`].
//! - RandR monitor geometry used the output's physical size in
//!   millimeters instead of the CRTC's pixel mode; here it reads the CRTC.
//!
//! Not implemented (documented rather than faked): XKB-level keymaps (only
//! a hand-maintained keysym table covering common keys, shared with the
//! Wayland backend via `srdwm_core::keysyms`), ICCCM `WM_HINTS`/urgency, and
//! EWMH pager/taskbar hints beyond
//! `_NET_SUPPORTED`/`_NET_CLIENT_LIST`/`_NET_WM_STATE` maximize.

use srdwm_core::keysyms;
use srdwm_core::{Event, Modifiers, MouseButton, TitlebarHit, Window as CoreWindow, WindowId, TITLEBAR_HEIGHT};
use srdwm_core::{Monitor, Rect, WindowManager};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::rc::Rc;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    ButtonIndex, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    EventMask, GrabMode, ModMask, Rectangle, StackMode, Window as XWindow, WindowClass,
};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        WM_STATE,
        _NET_SUPPORTED,
        _NET_WM_NAME,
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_CLIENT_LIST,
        _NET_ACTIVE_WINDOW,
        UTF8_STRING,
    }
}

struct Frame {
    frame: XWindow,
    client: XWindow,
    supports_delete: bool,
}

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(e.to_string())
}

/// Finds which of `ModMask::M1`..`M5` a keycode is bound to, given a
/// `GetModifierMappingReply`'s flattened `keycodes` list (8 fixed slots --
/// Shift, Lock, Control, Mod1..Mod5 - each `keycodes_per_modifier` long,
/// zero-padded). Only scans the Mod1..Mod5 slots (indices 3..8): Shift/
/// Lock/Control are never where Num Lock lands in practice, and this is
/// only ever called looking for it. Returns an empty mask if the keycode
/// isn't bound to any modifier at all (a keyboard with no Num Lock key, or
/// a keycode of `0` from a lookup that found nothing).
fn modmask_for_keycode_in_mod_slots(keycode: u8, keycodes_per_modifier: usize, keycodes: &[u8]) -> ModMask {
    if keycode == 0 || keycodes_per_modifier == 0 {
        return ModMask::from(0u16);
    }
    (3..8usize)
        .find(|&slot| {
            let start = slot * keycodes_per_modifier;
            keycodes.get(start..start + keycodes_per_modifier).is_some_and(|ks| ks.contains(&keycode))
        })
        .map(|slot| ModMask::from(1u16 << slot))
        .unwrap_or(ModMask::from(0u16))
}

/// Packs an RGB triple into the `0x00RRGGBB` pixel value X11's
/// `border_pixel`/GC `foreground` etc. expect on a TrueColor visual --
/// matching the format the hardcoded titlebar colour constants in
/// `redraw_decoration` already use.
fn rgb_to_pixel((r, g, b): (u8, u8, u8)) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

pub struct X11Platform {
    conn: RustConnection,
    root: XWindow,
    atoms: Atoms,
    gc: x11rb::protocol::xproto::Gcontext,
    font: x11rb::protocol::xproto::Font,
    wm: Rc<RefCell<WindowManager>>,
    xid_to_core: HashMap<XWindow, WindowId>,
    frames: HashMap<WindowId, Frame>,
    min_keycode: u8,
    max_keycode: u8,
    keysyms_per_keycode: u8,
    keyboard_mapping: Vec<u32>,
    /// Whichever of `ModMask::M1`..`M5` the server has Num Lock bound to --
    /// see `grab_keybindings`'s doc comment for why this needs grabbing
    /// alongside every binding, not just the modifiers a config actually
    /// asked for.
    numlock_mask: ModMask,
    /// `srd`'s control socket - see `srdwm_platform::IpcServer`'s module
    /// doc comment. `None` if binding it failed (a stale socket from a
    /// still-running instance, an unwritable runtime dir): the compositor
    /// itself still starts either way, matching how the Wayland backends
    /// already treat this as non-fatal.
    ipc: Option<srdwm_platform::IpcServer>,
}



/// Small helper so we can grab an owned snapshot of a `&Window` out of a
/// `Ref<WindowManager>` borrow without holding the borrow across the redraw call.
trait ClonedForRender {
    fn cloned_for_render(self) -> Option<CoreWindow>;
}
impl ClonedForRender for Option<&CoreWindow> {
    fn cloned_for_render(self) -> Option<CoreWindow> {
        self.cloned()
    }
}

mod actions;
mod connect;
mod events;
mod trait_impl;
mod window;

#[cfg(test)]
mod tests;
