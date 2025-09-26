//! Requesting a fake (fully virtual, no real hardware) monitor be created
//! or removed - see `crates/wayland/src/udev/virtual_heads.rs`'s own
//! module doc comment for the full design. Split out the same way
//! `lock.rs`/`input_pin.rs` are: everything here is plain `impl
//! WindowManager` methods; see `super` (`mod.rs`) for `WindowManager`'s
//! field definitions.

use super::*;

impl WindowManager {
    /// Queues a request to create a fake monitor named `name` at
    /// `width`x`height` - the only caller today is the IPC
    /// `"create_fake_monitor"` dispatch, the compositor-agnostic side of
    /// `srd dispatch create fake-monitor`. Core has no real `wl_output`
    /// to create itself (backend-owned, same as every other cross-
    /// boundary request here); the Wayland backend drains and applies
    /// this on its own next poll.
    pub fn request_create_fake_monitor(&mut self, name: String, width: u32, height: u32) {
        self.create_fake_monitor_requests.push((name, width, height));
    }

    pub fn drain_create_fake_monitor_requests(&mut self) -> Vec<(String, u32, u32)> {
        std::mem::take(&mut self.create_fake_monitor_requests)
    }

    /// Same cross-boundary-request pattern, for removing a fake monitor
    /// by name - the IPC `"remove_fake_monitor"` dispatch.
    pub fn request_remove_fake_monitor(&mut self, name: String) {
        self.remove_fake_monitor_requests.push(name);
    }

    pub fn drain_remove_fake_monitor_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.remove_fake_monitor_requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_create_request_is_reported_once_then_the_queue_is_empty() {
        let mut wm = WindowManager::new();
        assert!(wm.drain_create_fake_monitor_requests().is_empty());
        wm.request_create_fake_monitor("FAKE-1".into(), 1920, 1080);
        assert_eq!(wm.drain_create_fake_monitor_requests(), vec![("FAKE-1".to_string(), 1920, 1080)]);
        assert!(wm.drain_create_fake_monitor_requests().is_empty());
    }

    #[test]
    fn a_remove_request_is_reported_once_then_the_queue_is_empty() {
        let mut wm = WindowManager::new();
        wm.request_remove_fake_monitor("FAKE-1".into());
        assert_eq!(wm.drain_remove_fake_monitor_requests(), vec!["FAKE-1".to_string()]);
        assert!(wm.drain_remove_fake_monitor_requests().is_empty());
    }

    #[test]
    fn multiple_create_requests_before_a_drain_are_all_preserved() {
        // Unlike `request_pin_input`'s own "replace, don't accumulate"
        // semantics (one pid can only ever have one current pin target),
        // two different fake-monitor names are two genuinely independent
        // creations - both must survive to the next drain.
        let mut wm = WindowManager::new();
        wm.request_create_fake_monitor("FAKE-1".into(), 1920, 1080);
        wm.request_create_fake_monitor("FAKE-2".into(), 1280, 720);
        assert_eq!(wm.drain_create_fake_monitor_requests(), vec![("FAKE-1".to_string(), 1920, 1080), ("FAKE-2".to_string(), 1280, 720)]);
    }
}
