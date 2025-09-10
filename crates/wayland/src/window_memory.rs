//! Persists `WindowManager::remembered_geometry` (per-`app_id` last
//! floating position+size) across a restart - see that field's own doc
//! comment in `srdwm_core` for what it is and why it's read at window-map
//! time. Same load/save-at-the-platform-layer split, same JSON-file-under-
//! `$XDG_STATE_HOME/srd` shape, and same atomic tmp-then-rename write this
//! project already established twice (`monitor_layout.rs`, `desktop_icons_
//! state.rs`) for exactly this kind of small, rarely-written, must-survive-
//! a-crash state - deliberately not sharing code with either, matching
//! this codebase's own "a few duplicated lines beats a shared abstraction
//! for three near-identical small stores" precedent.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub(crate) struct PersistedGeometry {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedWindowMemory {
    /// Keyed by `app_id` - the same identifier `remembered_geometry`
    /// itself is keyed by, and the one thing guaranteed stable across a
    /// restart that a per-session `WindowId` is not.
    apps: HashMap<String, PersistedGeometry>,
}

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

fn memory_path() -> PathBuf {
    state_dir().join("window-memory.json")
}

/// Every remembered app's geometry, by `app_id`. Empty (not an error) if
/// the file doesn't exist yet or is present but unreadable/corrupt - a
/// bad state file degrades to "nothing remembered yet", not a startup
/// failure.
pub(crate) fn load() -> HashMap<String, PersistedGeometry> {
    let path = memory_path();
    let Ok(bytes) = std::fs::read(&path) else { return HashMap::new() };
    match serde_json::from_slice::<PersistedWindowMemory>(&bytes) {
        Ok(memory) => memory.apps,
        Err(e) => {
            log::warn!("window_memory: couldn't parse {path:?} ({e}); starting with nothing remembered");
            HashMap::new()
        }
    }
}

/// Overwrites the whole persisted table from `entries` - called after
/// every drag/resize-end (see `input/pointer.rs`'s call site), which are
/// rare, real user actions, not a per-frame event, so writing the whole
/// small file each time costs nothing and needs no separate dirty-tracking
/// story (the same reasoning `monitor_layout::save_output` and `desktop_
/// icons_state`'s own saver already settled on for the identical shape of
/// problem).
pub(crate) fn save_all<'a>(entries: impl Iterator<Item = (&'a str, (i32, i32, u32, u32))>) {
    let apps: HashMap<String, PersistedGeometry> =
        entries.map(|(app_id, (x, y, width, height))| (app_id.to_string(), PersistedGeometry { x, y, width, height })).collect();
    let memory = PersistedWindowMemory { apps };
    let dir = state_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("window_memory: couldn't create {dir:?} ({e}); this session's window positions/sizes won't survive a restart");
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&memory) else { return };
    let path = memory_path();
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        log::warn!("window_memory: couldn't write {tmp:?} ({e}); this session's window positions/sizes won't survive a restart");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("window_memory: couldn't rename {tmp:?} to {path:?} ({e}); this session's window positions/sizes won't survive a restart");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only the pure JSON round-trip is exercised here - `state_dir()`/
    // `load()`/`save_all()` all touch real environment variables and the
    // filesystem, which parallel `cargo test` execution can't safely share
    // - same reasoning `monitor_layout.rs`'s own tests give for staying
    // off real env vars.
    #[test]
    fn a_persisted_table_survives_a_json_round_trip() {
        let mut apps = HashMap::new();
        apps.insert("alacritty".to_string(), PersistedGeometry { x: 100, y: 100, width: 800, height: 600 });
        apps.insert("firefox".to_string(), PersistedGeometry { x: -500, y: 0, width: 1280, height: 900 });
        let memory = PersistedWindowMemory { apps };
        let bytes = serde_json::to_vec(&memory).unwrap();
        let parsed: PersistedWindowMemory = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.apps.get("alacritty"), Some(&PersistedGeometry { x: 100, y: 100, width: 800, height: 600 }));
        assert_eq!(parsed.apps.get("firefox"), Some(&PersistedGeometry { x: -500, y: 0, width: 1280, height: 900 }));
    }

    #[test]
    fn corrupt_json_falls_back_to_an_empty_table_not_an_error() {
        let result = serde_json::from_slice::<PersistedWindowMemory>(b"not valid json");
        assert!(result.is_err(), "sanity: this fixture must actually fail to parse");
        // `load()` itself can't be called here (touches the real
        // filesystem/env) - this locks in the *shape* of the fallback
        // `load()` relies on: a parse error, not a panic, is what lets it
        // degrade to `HashMap::new()` instead of taking the compositor down.
    }
}
