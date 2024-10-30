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
    fn requesting_lock_twice_before_a_drain_is_still_just_one_pending_request() {
        let mut wm = WindowManager::new();
        wm.request_lock();
        wm.request_lock();
        assert!(wm.drain_lock_request());
        assert!(!wm.drain_lock_request());
    }
}
