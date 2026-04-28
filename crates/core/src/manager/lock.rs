//! Requesting srdwm's own session lock. Split out of the original single
//! `manager.rs` - see `super` (`mod.rs`) for `WindowManager`'s field
//! definitions; everything here is plain `impl WindowManager` methods.

use super::*;

impl WindowManager {
    /// Queues a request for the backend to enter its own lock UI - the
    /// only caller today is the IPC `"lock"` dispatch, the compositor-
    /// agnostic side of `srd dispatch lock`. Core cannot lock the screen
    /// itself (that's real rendering/input-routing, backend-owned); see
    /// `lock_requested`'s own doc comment for why this has to cross the
    /// core/backend boundary as a queued request rather than a direct call.
    ///
    /// Idempotent by construction (a plain `bool`, not a counter): asking
    /// to lock twice before the backend's next poll drains it is exactly
    /// as locked as asking once.
    pub fn request_lock(&mut self) {
        self.lock_requested = true;
    }

    /// Takes the current lock request, if any, leaving none pending. The
    /// backend calls this once per poll, same as `drain_output_position_
    /// requests`.
    pub fn drain_lock_request(&mut self) -> bool {
        std::mem::take(&mut self.lock_requested)
    }

    /// Queues a request for the config layer to re-read `init.lua` and fire
    /// the `srd.on("refresh", ...)` handler.
    ///
    /// Same core/backend split as `request_lock` above, for the same
    /// reason: core owns no Lua state, so it cannot reload a config or run
    /// a handler itself. The desktop menu's own "Refresh" row is the
    /// caller.
    ///
    /// Asked for as "does refresh refresh configs in a function list in the
    /// config ie refresh os, etc, ags/aegis/polybar/waybar". Refresh used
    /// to re-scan the desktop icon grid and nothing else, so there was no
    /// way to make it reload anything the user actually cared about. What
    /// "refresh" *means* beyond srdwm's own config is deliberately the
    /// config's decision, not a hardcoded list of other people's tools --
    /// this compositor has no business knowing whether the user runs
    /// waybar or AGS.
    pub fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    /// Takes the current refresh request, if any. Drained once per poll,
    /// same as `drain_lock_request`.
    pub fn drain_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.refresh_requested)
    }

    /// Records that `key` was set live to `value_json`. See
    /// `live_settings`' own doc comment.
    ///
    /// Last write wins, so setting the same key twice leaves one entry and
    /// the replay applies each key exactly once.
    pub fn record_live_setting(&mut self, key: &str, value_json: String) {
        self.live_settings.insert(key.to_string(), value_json);
    }

    /// Every live setting recorded so far, for replay after a config
    /// reload. Cloned rather than borrowed: the replay mutates the same
    /// `WindowManager` this came from.
    pub fn live_settings(&self) -> Vec<(String, String)> {
        self.live_settings.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_lock_request_is_true_once_then_false() {
        let mut wm = WindowManager::new();
        assert!(!wm.drain_lock_request(), "nothing requested yet");
        wm.request_lock();
        assert!(wm.drain_lock_request(), "must report the pending request");
        assert!(!wm.drain_lock_request(), "must not report the same request twice");
    }

    #[test]
    fn drain_refresh_request_is_true_once_then_false() {
        let mut wm = WindowManager::new();
        assert!(!wm.drain_refresh_request(), "nothing requested yet");
        wm.request_refresh();
        assert!(wm.drain_refresh_request(), "must report the pending request");
        assert!(!wm.drain_refresh_request(), "must not report the same request twice");
    }

    #[test]
    fn a_refresh_request_is_independent_of_a_lock_request() {
        let mut wm = WindowManager::new();
        wm.request_refresh();
        assert!(!wm.drain_lock_request(), "refresh must not look like a lock");
        assert!(wm.drain_refresh_request());
    }

    #[test]
    fn requesting_lock_twice_before_a_drain_is_still_just_one_pending_request() {
        let mut wm = WindowManager::new();
        wm.request_lock();
        wm.request_lock();
        assert!(wm.drain_lock_request());
        assert!(!wm.drain_lock_request());
    }
}
