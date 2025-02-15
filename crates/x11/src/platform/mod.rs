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
        _NET_WM_STRUT,
        _NET_WM_STRUT_PARTIAL,
        UTF8_STRING,
        // Global-menu properties - see `read_global_menu`'s doc comment.
        // A native X11 client is exactly the same GTK/Qt app the Wayland
        // backend's `xwayland.rs::read_global_menu` already reads these
        // from (XWayland is just another X server as far as a toolkit is
        // concerned), so this is the identical atom set for the identical
        // reason.
        _GTK_UNIQUE_BUS_NAME,
        _GTK_APPLICATION_OBJECT_PATH,
        _GTK_WINDOW_OBJECT_PATH,
        _GTK_MENUBAR_OBJECT_PATH,
        _GTK_APP_MENU_OBJECT_PATH,
        _UNITY_OBJECT_PATH,
        // KWin's own global-menu property pair - what `libdbusmenu-qt`'s
        // KDE integration sets. Already a complete, unambiguous
        // `com.canonical.dbusmenu` address on its own (no classification
        // needed the way the GTK/Unity atoms above need), and checked
        // first in `read_global_menu` for exactly that reason - see that
        // method's doc comment.
        _KDE_NET_WM_APPMENU_SERVICE_NAME,
        _KDE_NET_WM_APPMENU_OBJECT_PATH,
    }
}

struct Frame {
    frame: XWindow,
    client: XWindow,
    supports_delete: bool,
}

/// One window's own `_NET_WM_STRUT_PARTIAL` (or the older, span-free
/// `_NET_WM_STRUT`) reservation - the X11 equivalent of a Wayland
/// layer-shell surface's exclusive zone (`zwlr_layer_surface_v1::set_
/// exclusive_zone`, read on the Wayland backends via `layer_map_for_
/// output(...).non_exclusive_zone()`). At most one edge is ever nonzero
/// for a real panel/dock (a bar reserves *one* strip, not several), but
/// the property itself allows all four at once, so all four are kept.
///
/// Values are in root-window (screen-global) pixels, matching every other
/// X11 geometry value in this backend - there is no separate logical/
/// physical scale to convert between the way the Wayland backends' own
/// `zone_physical` conversion needs (`udev/platform.rs`'s `monitors()`),
/// since this compositor's X11 backend has no independent output-scale
/// concept at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Strut {
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
    left_start_y: i32,
    left_end_y: i32,
    right_start_y: i32,
    right_end_y: i32,
    top_start_x: i32,
    top_end_x: i32,
    bottom_start_x: i32,
    bottom_end_x: i32,
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
    /// `com.canonical.AppMenu.Registrar` - the classic Qt/`appmenu-qt5`
    /// global-menu source, see `srdwm_platform::appmenu_registrar`'s module
    /// doc comment. Unlike the Wayland backend (where this is `None` until
    /// XWayland finishes starting up), a native X11 session always has a
    /// real X server the moment this struct exists, so it's started
    /// unconditionally in `connect` - still `Option` because starting the
    /// D-Bus service itself can independently fail (see that module's own
    /// `None` handling).
    appmenu_registrar: Option<srdwm_platform::AppmenuRegistrarState>,
    /// Every currently-mapped window's own `_NET_WM_STRUT_PARTIAL`/`_NET_
    /// WM_STRUT` reservation, keyed by its X window id - populated on
    /// `MapNotify` (see `events.rs`) and kept fresh via `PropertyNotify`
    /// on the same two atoms, removed on `UnmapNotify`/`DestroyNotify`.
    /// Read by `monitors()` to shrink each monitor's own usable rect the
    /// same way the Wayland backends' layer-shell exclusive zones already
    /// do - see that method's own doc comment. Deliberately keyed on
    /// *any* mapped window that sets the property, not gated on `_NET_WM_
    /// WINDOW_TYPE_DOCK` specifically: the EWMH spec's actual reservation
    /// mechanism is the strut property itself, and real struts also come
    /// from override-redirect panels that never go through `manage_new_
    /// window`/`xid_to_core` at all (confirmed live by a peer session's
    /// own aegis bar, an override-redirect `_NET_WM_WINDOW_TYPE_DOCK`
    /// window) - so this can't be folded into the existing managed-
    /// window bookkeeping.
    struts: HashMap<XWindow, Strut>,
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
mod global_menu;
mod struts;
mod trait_impl;
mod window;

#[cfg(test)]
use struts::usable_rect;

#[cfg(test)]
mod tests;
