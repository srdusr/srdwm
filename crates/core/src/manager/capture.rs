//! Requesting an off-screen render of a workspace's window tree to a file.
//! Split out the same way `lock.rs` is - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions.

use super::*;

/// One queued `srd capture workspace <id> <path> [WxH]` request. Core has
/// no renderer of its own (that's backend-owned, same boundary
/// `request_lock`/`request_output_position` already cross) - this is
/// just the request's data, drained and acted on by whichever backend is
/// actually running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRequest {
    pub workspace: WorkspaceId,
    pub path: String,
    /// `None` renders at the workspace's own monitor resolution.
    pub size: Option<(u32, u32)>,
}

impl WindowManager {
    /// Queues an off-screen capture of `workspace`'s window tree, written
    /// to `path` as a PPM image once the backend's next poll drains it.
    /// Exists for exactly one reason: a workspace switcher (AGS's Overview)
    /// wanting a thumbnail of a workspace that is not the one currently on
    /// screen. `wlr-screencopy` (what `grim`, and this compositor's own
    /// `screencopy.rs`, use) can only ever see what an output is actually
    /// presenting - it has no way to see a workspace that is not the
    /// active one, which is exactly the case a workspace-switcher preview
    /// needs most. This is not a re-implementation of screencopy; it is
    /// the one thing screencopy structurally cannot do, requested the same
    /// cross-boundary way `request_lock` is.
    ///
    /// Multiple requests for the same workspace queue independently (unlike
    /// `request_lock`'s single flag) - a caller asking for two different
    /// sizes, or overwriting a previous request for the same workspace
    /// before the backend gets to it, are both legitimate.
    pub fn request_capture_workspace(&mut self, workspace: WorkspaceId, path: String, size: Option<(u32, u32)>) {
        self.capture_requests.push(CaptureRequest { workspace, path, size });
    }

    /// Takes every currently-queued capture request, leaving none pending.
    /// The backend calls this once per poll, same as
    /// `drain_output_position_requests`.
    pub fn drain_capture_requests(&mut self) -> Vec<CaptureRequest> {
        std::mem::take(&mut self.capture_requests)
    }

    /// Front-to-back window ids for an arbitrary (not necessarily current)
    /// workspace - the same "topmost first" convention
    /// `visible_windows_front_to_back` already gives the current one, but
    /// that method is hardcoded to `self.current_workspace`, and a capture
    /// request's whole reason for existing is targeting a workspace that
    /// usually is *not* the current one. `order` is back-to-front (see its
    /// own field doc comment), hence the same `.rev()` that method uses.
    pub fn window_ids_on_workspace_front_to_back(&self, workspace: WorkspaceId) -> Vec<WindowId> {
        self.order.iter().rev().filter(|&&id| self.windows.get(&id).is_some_and(|w| w.workspace == workspace && !w.minimized)).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_requests_drain_in_arrival_order() {
        let mut wm = WindowManager::new();
        wm.request_capture_workspace(0, "/tmp/a.ppm".into(), None);
        wm.request_capture_workspace(1, "/tmp/b.ppm".into(), Some((100, 100)));
        assert_eq!(
            wm.drain_capture_requests(),
            vec![
                CaptureRequest { workspace: 0, path: "/tmp/a.ppm".into(), size: None },
                CaptureRequest { workspace: 1, path: "/tmp/b.ppm".into(), size: Some((100, 100)) },
            ]
        );
        assert!(wm.drain_capture_requests().is_empty(), "must not report the same requests twice");
    }
}
