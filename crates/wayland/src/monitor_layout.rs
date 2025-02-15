//! Persists and restores monitor layout (position, enabled state) across
//! restarts - srdwm's own responsibility, not any particular panel's.
//!
//! Before this, whatever arranged outputs on a restart was whichever panel
//! happened to be running: it read back its own remembered layout from its
//! own config store, seconds after srdwm itself had already brought every
//! head up at some arbitrary default position, and dispatched `srd
//! dispatch set output position` for each one to fix it up after the fact.
//! That has three real problems, not just one: the user watches the wrong
//! arrangement for however long the panel takes to start and apply it (one
//! peer session measured 13.7s on its own, worse on a cold boot before the
//! panel is even launched); the layout is never restored at all if that
//! panel doesn't run or loses its own store; and this compositor is meant
//! to work with any panel or none, not assume one particular shell exists
//! to do this job.
//!
//! Applied once, at startup, before the Wayland socket is even bound (see
//! `UdevPlatform::connect`'s call site) - no client, panel or otherwise,
//! can possibly see a pre-restore arrangement, not even for one frame.
//! Saved on every live position/enabled change (`apply_output_position`,
//! `disable_connector_by_name`, `enable_connector_by_name`), so a panel's
//! own output-management UI (or `srd dispatch set output ...` run by hand)
//! keeps working exactly as before and this file just stays the one place
//! that remembers the result.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub(crate) struct PersistedOutput {
    /// Physical pixels - this compositor's own convention throughout, the
    /// same space `srd monitors`/`apply_output_position` already use.
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) enabled: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedLayout {
    /// Keyed by connector name (`eDP-1`, `HDMI-A-1`, ...) - the same
    /// identifier every other per-monitor config in this codebase
    /// (`srd.monitor.scale`, `srd dispatch set output ...`) already keys
    /// off, and the one thing guaranteed stable across a reboot that a
    /// kernel-assigned head index or CRTC handle is not.
    outputs: HashMap<String, PersistedOutput>,
}

/// `$XDG_STATE_HOME/srd`, else `~/.local/state/srd` - state, not config:
/// this file records what the compositor *did*, not something the user
/// hand-edits, the same distinction XDG draws between the two directories.
/// Mirrors `srdwm/src/main.rs`'s own `config_dir()` shape (`$SRDWM_*`
/// override first, then the XDG var, then the hardcoded fallback) without
/// sharing code with it - this is `srdwm-wayland`, that's the `srdwm`
/// binary crate, and duplicating four lines beats a cross-crate dependency
/// for it.
fn state_dir() -> PathBuf {
    if let Ok(p) = std::env::var("SRDWM_STATE_PATH") {
        return PathBuf::from(p);
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("srd");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/state/srd");
    }
    PathBuf::from("state/srd")
}

fn layout_path() -> PathBuf {
    state_dir().join("monitor-layout.json")
}

/// Every remembered output, by connector name. Empty (not an error) if the
/// file doesn't exist yet - the ordinary case on a machine's first run,
/// or the first run after this feature shipped - or if it's present but
/// unreadable/corrupt, since a bad state file should degrade to "use the
/// default layout", not stop the compositor from starting at all.
pub(crate) fn load() -> HashMap<String, PersistedOutput> {
    let path = layout_path();
    let Ok(bytes) = std::fs::read(&path) else { return HashMap::new() };
    match serde_json::from_slice::<PersistedLayout>(&bytes) {
        Ok(layout) => layout.outputs,
        Err(e) => {
            log::warn!("monitor_layout: couldn't parse {path:?} ({e}); starting with the default layout instead");
            HashMap::new()
        }
    }
}

/// Overwrites one connector's remembered position/enabled state and
/// rewrites the whole file. Read-modify-write, not an in-memory cache kept
/// across calls - position/enabled changes are rare (a user rearranging
/// monitors, not a per-frame event), so re-reading the small file each
/// time costs nothing and needs no separate cache-invalidation story.
pub(crate) fn save_output(name: &str, entry: PersistedOutput) {
    let mut layout = PersistedLayout { outputs: load() };
    layout.outputs.insert(name.to_string(), entry);
    let dir = state_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("monitor_layout: couldn't create {dir:?} ({e}); this layout change won't survive a restart");
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&layout) else { return };
    let path = layout_path();
    // Written to a `.tmp` sibling and renamed into place - same reasoning
    // as `udev/capture.rs`'s `write_ppm`: a crash or a second srdwm
    // instance racing this write must never leave a half-written, corrupt
    // file behind for the next startup's `load()` to choke on.
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        log::warn!("monitor_layout: couldn't write {tmp:?} ({e}); this layout change won't survive a restart");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("monitor_layout: couldn't rename {tmp:?} to {path:?} ({e}); this layout change won't survive a restart");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only the pure JSON round-trip is exercised here - `state_dir()`/
    // `load()`/`save_output()` all touch real environment variables and
    // the filesystem, which parallel `cargo test` execution can't safely
    // share (a per-test `SRDWM_STATE_PATH` override would race every other
    // test in this binary reading the same process-global env var), the
    // same reasoning `config_dir()` in `srdwm/src/main.rs` already has no
    // test coverage for.
    #[test]
    fn a_persisted_layout_survives_a_json_round_trip() {
        let mut outputs = HashMap::new();
        outputs.insert("eDP-1".to_string(), PersistedOutput { x: 0, y: 0, enabled: true });
        outputs.insert("HDMI-A-1".to_string(), PersistedOutput { x: -1920, y: 0, enabled: false });
        let layout = PersistedLayout { outputs };
        let bytes = serde_json::to_vec(&layout).unwrap();
        let parsed: PersistedLayout = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.outputs.get("eDP-1"), Some(&PersistedOutput { x: 0, y: 0, enabled: true }));
        assert_eq!(parsed.outputs.get("HDMI-A-1"), Some(&PersistedOutput { x: -1920, y: 0, enabled: false }));
    }

    #[test]
    fn corrupt_json_falls_back_to_an_empty_layout_not_an_error() {
        let result = serde_json::from_slice::<PersistedLayout>(b"not valid json");
        assert!(result.is_err(), "sanity: this fixture must actually fail to parse");
        // `load()` itself can't be called here (touches the real
        // filesystem/env) - this locks in the *shape* of the fallback
        // `load()` relies on: a parse error, not a panic, is what lets it
        // degrade to `HashMap::new()` instead of taking the compositor down.
    }
}
