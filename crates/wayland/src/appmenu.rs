//! `org_kde_kwin_appmenu`: the Wayland-native equivalent of the `_GTK_*`/
//! `_UNITY_OBJECT_PATH` X11 properties `xwayland.rs`'s `read_global_menu`
//! reads - lets a client link a `wl_surface` straight to a
//! `com.canonical.dbusmenu` D-Bus address, no XWayland/X11 property
//! round-trip involved at all.
//!
//! This is the only real gap X11 property reading structurally can't
//! close: `xwayland.rs::read_global_menu` only ever runs for a window that
//! `x11_surface()` resolves (an XWayland client), so a genuinely
//! Wayland-native GTK4/Qt6 window - no XWayland involved - could never
//! have exported a menu srdwm would see, regardless of how correct the X11
//! side is. There is no Wayland-native equivalent of `_GTK_MENUBAR_OBJECT_
//! PATH`/GMenuModel (GTK's own menu export is X11-property-only, by
//! GTK's own design, XWayland or not), but Qt/KDE's side of this - the
//! same `com.canonical.dbusmenu` content `appmenu-qt5`'s Unity-registrar
//! path exports over X11 - does have a real Wayland-native protocol for
//! it, and it's what this file implements. Closes the other open half of
//! the Qt global-menu gap found this session: `read_global_menu`'s
//! `_UNITY_OBJECT_PATH` handling only ever reaches a Qt app that still
//! goes through XWayland, which a `QT_QPA_PLATFORM=wayland` app (the
//! default this session's `env.conf` port now sets) does not.
//!
//! Always `MenuSource::DbusMenu`, never `MenuSource::Unity`: this
//! protocol's own description says exactly what it addresses - "a
//! `com.canonical.dbusmenu` interface" - and despite the name, `Unity`
//! does *not* mean dbusmenu content in this codebase (see that variant's
//! own doc comment, corrected after an AGS peer session caught this exact
//! mislabeling live: `appmenu-gtk-module`'s `_UNITY_OBJECT_PATH` points at
//! an `org.gtk.Menus` object, not a dbusmenu one). `DbusMenu` is the one
//! that means what this protocol carries. No classification heuristic
//! needed the way `xwayland.rs::classify_menu_source` needs one for the
//! X11 side, though: this protocol only ever carries the one kind of
//! content.
//!
//! No smithay helper exists for this protocol (same as `gamma_control.rs`/
//! `output_power.rs`/`screencopy.rs`), so the `GlobalDispatch`/`Dispatch`
//! plumbing below is hand-written against the raw `wayland-protocols-
//! plasma` server bindings.

use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New};
use wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu::{self, OrgKdeKwinAppmenu};
use wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu_manager::{self, OrgKdeKwinAppmenuManager};

use crate::state::CompState;

/// The manager global. Held by `CompState` purely to keep the global alive
/// for the compositor's lifetime - same reasoning as `GammaControlManagerState`.
pub struct AppmenuManagerState {
    _global: GlobalId,
}

impl AppmenuManagerState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<OrgKdeKwinAppmenuManager, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, OrgKdeKwinAppmenuManager, _>(2, ()) }
    }
}

/// Which `wl_surface` an `org_kde_kwin_appmenu` object addresses, resolved
/// once at creation - same reasoning as `GammaControlData`'s output.
pub struct AppmenuData {
    surface: WlSurface,
}

impl GlobalDispatch<OrgKdeKwinAppmenuManager, ()> for CompState {
    fn bind(_state: &mut Self, _dh: &DisplayHandle, _client: &Client, manager: New<OrgKdeKwinAppmenuManager>, _data: &(), data_init: &mut DataInit<'_, Self>) {
        data_init.init(manager, ());
    }
}

impl Dispatch<OrgKdeKwinAppmenuManager, ()> for CompState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &OrgKdeKwinAppmenuManager,
        request: org_kde_kwin_appmenu_manager::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let org_kde_kwin_appmenu_manager::Request::Create { id, surface } = request else { return };
        data_init.init(id, AppmenuData { surface });
    }
}

impl Dispatch<OrgKdeKwinAppmenu, AppmenuData> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &OrgKdeKwinAppmenu,
        request: org_kde_kwin_appmenu::Request,
        data: &AppmenuData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let org_kde_kwin_appmenu::Request::SetAddress { service_name, object_path } = request else { return };
        let Some(id) = state.surface_to_id.get(&data.surface).copied() else { return };
        if let Some(w) = state.wm.borrow_mut().window_mut(id) {
            w.global_menu = Some(srdwm_core::GlobalMenu {
                bus_name: service_name,
                menu_path: Some(object_path),
                app_path: None,
                window_path: None,
                source: srdwm_core::MenuSource::DbusMenu,
            });
        }
    }

    /// The protocol's own doc comment: "If not applicable, clients should
    /// remove this object" - releasing (or disconnecting) is a real,
    /// expected way for a client to say "no menu after all", not just
    /// cleanup, so the address needs actually clearing here rather than
    /// left stale for whichever window this surface maps to.
    fn destroyed(state: &mut Self, _client: ClientId, _resource: &OrgKdeKwinAppmenu, data: &AppmenuData) {
        let Some(id) = state.surface_to_id.get(&data.surface).copied() else { return };
        if let Some(w) = state.wm.borrow_mut().window_mut(id) {
            w.global_menu = None;
        }
    }
}
