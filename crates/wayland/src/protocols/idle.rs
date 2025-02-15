//! `ext_idle_notify_v1` + `zwp_idle_inhibit_manager_v1`.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::CompState;

/// `ext_idle_notify_v1`. All the real logic (per-notification timers,
/// resetting them on activity, honouring inhibition) already lives in
/// smithay's own `IdleNotifierState` - this is just the getter it needs.
/// See `input.rs`'s `notify_idle_activity` for the other half: nothing
/// calls `notify_activity` on its own, that has to happen from every real
/// input path.
impl smithay::wayland::idle_notify::IdleNotifierHandler for CompState {
    fn idle_notifier_state(&mut self) -> &mut smithay::wayland::idle_notify::IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}

/// `zwp_idle_inhibit_manager_v1`. A video player (or anything else that
/// wants the screen to stay on/unlocked while it runs) creates one of
/// these tied to its own surface; as long as at least one is alive,
/// `IdleNotifierState::set_is_inhibited` stops idle timers from firing at
/// all - see `idle_inhibiting_surfaces`'s doc comment on `CompState` for
/// the one simplification (not workspace-visibility-aware) this takes.
impl smithay::wayland::idle_inhibit::IdleInhibitHandler for CompState {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibiting_surfaces.push(surface);
        self.idle_notifier_state.set_is_inhibited(true);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibiting_surfaces.retain(|s| s != &surface);
        self.idle_notifier_state.set_is_inhibited(!self.idle_inhibiting_surfaces.is_empty());
    }
}
