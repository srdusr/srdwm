//! Requesting a virtual-pointer pin to a specific window - Phase 2 of
//! this project's own multi-cursor plan (see `docs/TODO.md`'s "Multi-
//! cursor Phase 2" entry, and `crates/wayland/src/virtual_pointer.rs`'s
//! own module doc comment for the full design). Split out the same way
//! `lock.rs` is: everything here is plain `impl WindowManager` methods:
//! see `super` (`mod.rs`) for `WindowManager`'s field definitions.

use super::*;

impl WindowManager {
    /// Queues a request to pin (`window` is `Some`) or unpin (`None`)
    /// every virtual pointer object owned by the client with process id
    /// `pid` - the only caller today is the IPC `"pin_input"` dispatch,
    /// the compositor-agnostic side of `srd dispatch pin input`/`unpin
    /// input`. Core has no real Wayland protocol object to reach into
    /// itself (that's backend-owned, same as `output_position_requests`);
    /// the Wayland backend drains and applies this on its own next poll.
    ///
    /// Replaces (not accumulates) any still-pending request for the same
    /// `pid`, the same "last write wins" semantics `request_output_
    /// position` already has - only the *latest* requested pin for a
    /// given pid matters if several arrive before the backend's next
    /// drain.
    pub fn request_pin_input(&mut self, pid: i32, window: Option<WindowId>) {
        self.pin_input_requests.retain(|(existing, _)| *existing != pid);
        self.pin_input_requests.push((pid, window));
    }

    /// Takes every currently-queued pin-input request, leaving the queue
    /// empty. The backend calls this once per poll, same as `drain_
    /// output_position_requests`.
    pub fn drain_pin_input_requests(&mut self) -> Vec<(i32, Option<WindowId>)> {
        std::mem::take(&mut self.pin_input_requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_request_is_reported_once_then_the_queue_is_empty() {
        let mut wm = WindowManager::new();
        assert!(wm.drain_pin_input_requests().is_empty());
        wm.request_pin_input(1234, Some(7));
        assert_eq!(wm.drain_pin_input_requests(), vec![(1234, Some(7))]);
        assert!(wm.drain_pin_input_requests().is_empty());
    }

    #[test]
    fn a_second_request_for_the_same_pid_replaces_the_first_before_a_drain() {
        let mut wm = WindowManager::new();
        wm.request_pin_input(1234, Some(7));
        wm.request_pin_input(1234, Some(9));
        assert_eq!(wm.drain_pin_input_requests(), vec![(1234, Some(9))]);
    }

    #[test]
    fn unpinning_is_a_real_queued_request_too_not_a_no_op() {
        let mut wm = WindowManager::new();
        wm.request_pin_input(1234, None);
        assert_eq!(wm.drain_pin_input_requests(), vec![(1234, None)]);
    }
}
