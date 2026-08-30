//! `zxdg_decoration_manager_v1`: negotiates whether a toplevel draws its own
//! (client-side) chrome or lets us draw it (server-side).

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::state::CompState;

impl XdgDecorationHandler for CompState {
    /// Offers whichever mode `theme.decorations.default_mode`/`srd set
    /// decoration_mode` currently prefers - a client with a real opinion
    /// of its own still overrides this via `request_mode` below regardless
    /// of what's offered here; this only decides what a client with *no*
    /// preference ends up with. See `srdwm_core::ThemeConfig::
    /// default_decorated`'s own doc comment for why this is configurable
    /// rather than hardcoded to one mode.
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        let offer = if self.wm.borrow().theme.default_decorated { DecorationMode::ServerSide } else { DecorationMode::ClientSide };
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(offer);
        });
    }

    /// Honors whichever mode the client actually asked for, rather than
    /// always forcing server-side - and mirrors the result into our own
    /// `Window.decorated`, so a client drawing its own titlebar doesn't
    /// *also* get one drawn on top of it by us.
    ///
    /// Always forcing `ServerSide` (what this used to do) is why some
    /// clients ended up with two sets of window buttons: Firefox requests
    /// client-side decoration when its own "use system titlebar" setting
    /// is off, and draws its own close/minimize/maximize row regardless of
    /// what the compositor grants - so forcing server-side just added
    /// srdwm's row on top of the one Firefox was drawing anyway, instead
    /// of preventing it. Respecting the request means srdwm steps out of
    /// the way for exactly those clients, while everything that accepts
    /// (or has no preference and gets offered) server-side still gets our
    /// titlebar as before.
    /// `theme.decorations.force_server_side` overrides the client's request
    /// - see `ThemeConfig::force_server_side` for why that is allowed and
    ///   why it is off by default.
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        let forced = self.wm.borrow().theme.force_server_side;
        let mode = if forced { DecorationMode::ServerSide } else { mode };
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        toplevel.send_configure();
        self.set_decorated_from_mode(toplevel.wl_surface(), mode == DecorationMode::ServerSide);
    }

    /// The client dropped its decoration-mode preference. `new_decoration`
    /// already offers the configured default as the mode the next
    /// configure will carry, so mirror that same default here rather than
    /// leaving whatever mode was negotiated before this - otherwise a
    /// client that requests one mode, then later unsets it expecting the
    /// default back, would stay stuck in that mode forever.
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let default_decorated = self.wm.borrow().theme.default_decorated;
        let mode = if default_decorated { DecorationMode::ServerSide } else { DecorationMode::ClientSide };
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        toplevel.send_configure();
        self.set_decorated_from_mode(toplevel.wl_surface(), default_decorated);
    }
}
