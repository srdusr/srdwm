//! `xdg_activation_v1`: a launcher hands a spawned app a token, and the
//! app's own first window presents it back to ask to be raised and focused.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};

use crate::state::CompState;

impl XdgActivationHandler for CompState {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    /// A launcher spawns an app after first getting a token
    /// (`get_activation_token`) and handing it to the new process (usually
    /// via `XDG_ACTIVATION_TOKEN`); the app's own first window then
    /// presents that same token back here via `activate`, asking to be
    /// raised. Without this, that request was silently ignored - the new
    /// window opened and just sat there unfocused behind everything,
    /// exactly the gap `docs/PANEL_SUPPORT_TODO.md`'s P1 flagged.
    ///
    /// No token bookkeeping of our own: `token_created`'s default already
    /// accepts every token (fine for a single-user session with no
    /// cross-client trust boundary to enforce), so all that's left is
    /// mapping the activating `surface` to a `WindowId` and reusing the
    /// exact same `focus_window` path a dock's "activate" request already
    /// goes through (`foreign_toplevel.rs`). If the surface isn't tracked
    /// yet - the activation raced ahead of this window's own mapping --
    /// there is nothing to focus yet, so this is a no-op rather than an
    /// error; the protocol doesn't require honoring every activation.
    fn request_activation(&mut self, _token: XdgActivationToken, _token_data: XdgActivationTokenData, surface: WlSurface) {
        if let Some(&id) = self.surface_to_id.get(&surface) {
            crate::input::focus_window(self, id);
        }
    }
}
