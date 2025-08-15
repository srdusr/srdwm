//! Persists desktop icons' grid cells across restarts - same shape as
//! `monitor_layout.rs`, deliberately not sharing code with it (its own doc
//! comment gives the reasoning: two small, independent files beat a
//! cross-module dependency for four lines of env-var lookup).
//!
//! Only icons the user has actually dragged get an entry here - see
//! `desktop_icons::rescan`'s own doc comment for why a fresh/unmoved icon
//! deliberately has no row in this file at all.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
struct PersistedIcons {
    /// Keyed by `DesktopIcon::id` - `"home"`/`"computer"`/`"trash"`, or a
    /// real `~/Desktop` filename.
    icons: HashMap<String, (i32, i32)>,
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

fn icons_path() -> PathBuf {
    state_dir().join("desktop-icons.json")
}

/// Every remembered icon cell, by icon id. Empty (not an error) if the file
/// doesn't exist yet or is unreadable/corrupt - a bad state file degrades
/// to "every icon gets its default grid slot", not a startup failure.
pub(crate) fn load() -> HashMap<String, (i32, i32)> {
    let path = icons_path();
    let Ok(bytes) = std::fs::read(&path) else { return HashMap::new() };
    match serde_json::from_slice::<PersistedIcons>(&bytes) {
        Ok(persisted) => persisted.icons,
        Err(e) => {
            log::warn!("desktop_icons_state: couldn't parse {path:?} ({e}); starting with default icon positions instead");
            HashMap::new()
        }
    }
}

/// Overwrites one icon's remembered cell and rewrites the whole file --
/// read-modify-write, same reasoning as `monitor_layout.rs::save_output`:
/// a drag-and-drop is a rare, human-paced event, not a per-frame one, so
/// re-reading the small file each time costs nothing.
pub(crate) fn save_icon(id: &str, cell: (i32, i32)) {
    let mut persisted = PersistedIcons { icons: load() };
    persisted.icons.insert(id.to_string(), cell);
    let dir = state_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("desktop_icons_state: couldn't create {dir:?} ({e}); this icon's position won't survive a restart");
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&persisted) else { return };
    let path = icons_path();
    // `.tmp`-sibling-then-rename, same crash-safety reasoning as
    // `monitor_layout.rs::save_output`.
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        log::warn!("desktop_icons_state: couldn't write {tmp:?} ({e}); this icon's position won't survive a restart");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("desktop_icons_state: couldn't rename {tmp:?} to {path:?} ({e}); this icon's position won't survive a restart");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same reasoning as `monitor_layout.rs`'s own tests: only the pure JSON
    // round-trip is exercised here, not `load()`/`save_icon()` themselves,
    // since both touch real environment variables and the filesystem that
    // parallel `cargo test` runs can't safely share.
    #[test]
    fn a_persisted_icon_map_survives_a_json_round_trip() {
        let mut icons = HashMap::new();
        icons.insert("home".to_string(), (0, 0));
        icons.insert("report.txt".to_string(), (2, 3));
        let persisted = PersistedIcons { icons };
        let bytes = serde_json::to_vec(&persisted).unwrap();
        let parsed: PersistedIcons = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.icons.get("home"), Some(&(0, 0)));
        assert_eq!(parsed.icons.get("report.txt"), Some(&(2, 3)));
    }

    #[test]
    fn corrupt_json_falls_back_to_an_empty_map_not_an_error() {
        let result = serde_json::from_slice::<PersistedIcons>(b"not valid json");
        assert!(result.is_err(), "sanity: this fixture must actually fail to parse");
    }
}
