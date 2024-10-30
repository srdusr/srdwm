//! `gtk_shell1`/`gtk_surface1`: the Wayland-native half of global-menu
//! support - see `srdwm_core::GlobalMenu`'s doc comment and `xwayland.rs`'s
//! `EwmhState::read_global_menu` for the XWayland half and the full
//! rationale (carry the D-Bus *address*, never the menu content itself).
//!
//! GTK4 only calls `gtk_surface1.set_dbus_properties` - the one request
//! this module actually needs - once it knows the compositor supports the
//! protocol at all, which it establishes by binding `gtk_shell1` and
//! getting a `capabilities` event back. Everything else in the protocol
//! (startup-notification IDs, tiled-state events, modal hints, the
//! titlebar-gesture request GNOME Shell uses for its own double/right/
//! middle-click-titlebar handling) is real but out of scope for what this
//! pass is actually for - acknowledged with a no-op rather than silently
//! ignored, so a future pass extending this has a clear list of what was
//! deliberately left alone.

use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};

use crate::gtk_shell_protocol::server::gtk_shell1::{self, GtkShell1};
use crate::gtk_shell_protocol::server::gtk_surface1::{self, GtkSurface1};
use crate::state::CompState;

const PROTOCOL_VERSION: u32 = 7;

/// `global_app_menu | global_menu_bar` - both bits set unconditionally.
/// srdwm has no reason to advertise one without the other: both map onto
/// the same `GlobalMenu`, just the app-menu-only fallback for a client
/// that never got around to exporting a real menu bar.
const CAPABILITIES: u32 = 0b011;

pub struct GtkShellState {
    _global: GlobalId,
}

impl GtkShellState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<GtkShell1, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, GtkShell1, _>(PROTOCOL_VERSION, ()) }
    }
}

/// The `wl_surface` a `gtk_surface1` was created for - the only thing
/// `set_dbus_properties` needs to know which `WindowId` to attach its
/// payload to.
pub struct GtkSurfaceData {
    surface: WlSurface,
}

impl GlobalDispatch<GtkShell1, (), CompState> for CompState {
    fn bind(_state: &mut CompState, _dh: &DisplayHandle, _client: &Client, resource: New<GtkShell1>, _global_data: &(), data_init: &mut DataInit<'_, CompState>) {
        let shell = data_init.init(resource, ());
        // Sent once, right after bind - see this module's doc comment for
        // why this has to happen unprompted rather than waiting to be
        // asked: GTK only bothers calling `set_dbus_properties` later if it
        // saw this first.
        shell.capabilities(CAPABILITIES);
    }
}

impl Dispatch<GtkShell1, ()> for CompState {
    fn request(state: &mut CompState, _client: &Client, _resource: &GtkShell1, request: gtk_shell1::Request, _data: &(), _dh: &DisplayHandle, data_init: &mut DataInit<'_, CompState>) {
        if let gtk_shell1::Request::GetGtkSurface { gtk_surface, surface } = request {
            data_init.init(gtk_surface, GtkSurfaceData { surface });
        }
        // `set_startup_id`/`system_bell`/`notify_launch`: no startup-
        // notification or accessibility-bell feature exists to wire these
        // into yet - see this module's doc comment.
        let _ = state;
    }
}

impl Dispatch<GtkSurface1, GtkSurfaceData> for CompState {
    fn request(state: &mut CompState, _client: &Client, _resource: &GtkSurface1, request: gtk_surface1::Request, data: &GtkSurfaceData, _dh: &DisplayHandle, _data_init: &mut DataInit<'_, CompState>) {
        let gtk_surface1::Request::SetDbusProperties { menubar_path, app_menu_path, window_object_path, application_object_path, unique_bus_name, .. } = request else {
            // `set_modal`/`unset_modal`/`present`/`request_focus`/`release`/
            // `titlebar_gesture`/`set_a11y_properties`: real requests, none
            // of which this pass has a feature behind yet.
            return;
        };
        let Some(&id) = state.surface_to_id.get(&data.surface) else { return };
        // A `None`/empty bus name means the client is *clearing* its menu
        // (or never had one) - matches `xwayland.rs`'s `read_global_menu`
        // returning `None` for the same case, so a panel sees the same
        // shape regardless of which backend a window came from.
        //
        // `source` used to be hardcoded `MenuSource::Gtk` unconditionally
        // here, on the reasoning that this protocol "has no Unity-style
        // equivalent to carry" - true of the *wire message*, but not of
        // what's actually behind it: `appmenu-gtk-module` is the same
        // module regardless of whether it signals its address via X11
        // atoms (`xwayland.rs`) or `gtk_surface1.set_dbus_properties`
        // (here), and exports a plain `Gtk.Window`'s menu (no
        // `GtkApplication`) through its Unity-compatibility shim --
        // `unity.`-prefixed actions and all - on *either* path. That's
        // exactly the misclassification `classify_menu_source` was written
        // to fix for the X11 side (see its own doc comment and
        // `xwayland.rs::read_global_menu`); this call site just never
        // adopted it, so a native-Wayland GTK window hitting the same
        // shim case stayed permanently mislabeled `Gtk` while its XWayland
        // counterpart got the fix. Confirmed live: Nemo (native Wayland,
        // not XWayland - only Spotify was in `_NET_CLIENT_LIST` at the
        // time) reported `source: "gtk"` here, but its actual exported
        // menu content, read directly off the bus, used `unity.`-prefixed
        // actions throughout (File/Edit/View/Go/Bookmarks/Help) - the
        // exact "every item renders, none of them are ever clickable"
        // failure `classify_menu_source`'s own tests exist to catch.
        let is_real_gtk_application = application_object_path.as_deref().is_some_and(|s| !s.is_empty()) || window_object_path.as_deref().is_some_and(|s| !s.is_empty());
        let gtk_menu_path = menubar_path.filter(|s| !s.is_empty()).or_else(|| app_menu_path.filter(|s| !s.is_empty()));
        let (menu_path, source) = srdwm_core::classify_menu_source(gtk_menu_path, is_real_gtk_application, None);
        let menu = unique_bus_name.filter(|s| !s.is_empty()).map(|bus_name| srdwm_core::GlobalMenu {
            bus_name,
            menu_path,
            app_path: application_object_path.filter(|s| !s.is_empty()),
            window_path: window_object_path.filter(|s| !s.is_empty()),
            source,
        });
        if let Some(w) = state.wm.borrow_mut().window_mut(id) {
            w.global_menu = menu;
        }
    }

    fn destroyed(state: &mut CompState, _client: smithay::reexports::wayland_server::backend::ClientId, _resource: &GtkSurface1, data: &GtkSurfaceData) {
        // The surface itself may already be gone (this fires on the normal
        // window-close teardown path too, not just an explicit `release`)
        // - `surface_to_id` simply won't resolve in that case, same as
        // any other post-close lookup elsewhere in this codebase.
        if let Some(&id) = state.surface_to_id.get(&data.surface) {
            if let Some(w) = state.wm.borrow_mut().window_mut(id) {
                w.global_menu = None;
            }
        }
    }
}
