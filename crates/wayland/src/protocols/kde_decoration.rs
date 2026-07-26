//! `org_kde_kwin_server_decoration`: KDE's older decoration protocol, and
//! the only decoration protocol GTK actually speaks.
//!
//! GTK has never implemented `xdg-decoration`. It does implement this one:
//! `org_kde_kwin_server_decoration_manager` is present in libgtk-4 on this
//! machine (confirmed by reading the library's own symbol strings), and it
//! is how a KDE session gets GTK applications to stop drawing their own
//! frame. A compositor that advertises only `xdg-decoration` is invisible
//! to those clients, which is why "set the decoration once and every
//! application follows" stopped at srdwm's own titlebars.
//!
//! Both protocols are advertised, and both answer with the same policy
//! (`theme.default_decorated`, `theme.force_server_side`), so a client is
//! told the same thing whichever one it asks through.
//!
//! The protocol's own warning applies here: a client may ignore the mode
//! the compositor suggests and ask for its own. That is honoured the same
//! way `xdg_decoration.rs` honours it, and for the same reason - a client
//! that draws its own titlebar regardless (Firefox with its system-titlebar
//! setting off) would otherwise get srdwm's row on top of its own.

use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
    Mode, OrgKdeKwinServerDecoration,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::WEnum;
use smithay::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};

use crate::state::CompState;

impl KdeDecorationHandler for CompState {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.kde_decoration_state
    }

    /// Tells a client, the moment it asks, which mode this compositor
    /// wants - the same answer `XdgDecorationHandler::new_decoration`
    /// gives through the other protocol.
    ///
    /// The manager's own default mode (set once, at startup) is what a
    /// client sees before it creates a decoration object at all; this is
    /// what it sees afterward, and it has to agree, or a client that reads
    /// both ends up with two different answers.
    fn new_decoration(&mut self, surface: &WlSurface, decoration: &OrgKdeKwinServerDecoration) {
        let server = self.wm.borrow().theme.default_decorated;
        decoration.mode(if server { Mode::Server } else { Mode::Client });
        self.set_decorated_from_mode(surface, server);
    }

    /// Honours what the client asked for, unless `force_server_side` says
    /// otherwise - identical policy to the xdg-decoration path, and the
    /// mode is echoed back either way because the protocol requires the
    /// compositor to confirm what it decided.
    fn request_mode(&mut self, surface: &WlSurface, decoration: &OrgKdeKwinServerDecoration, mode: WEnum<Mode>) {
        let WEnum::Value(requested) = mode else { return };
        let forced = self.wm.borrow().theme.force_server_side;
        // `Mode::None` means no decoration at all, which for srdwm's
        // purposes is a client saying it wants nothing drawn around it --
        // treated as client-side, the same as `Mode::Client`, rather than
        // as a request for a titlebar.
        let server = forced || requested == Mode::Server;
        let granted = if server { Mode::Server } else { requested };
        decoration.mode(granted);
        self.set_decorated_from_mode(surface, server);
    }

    /// The client is going away, or has dropped its decoration object.
    /// Nothing to undo: `remove_window` already clears everything keyed on
    /// this surface, and a surface with no decoration object keeps
    /// whatever mode it last negotiated, exactly as before.
    fn release(&mut self, _decoration: &OrgKdeKwinServerDecoration, _surface: &WlSurface) {}
}

smithay::delegate_kde_decoration!(CompState);
