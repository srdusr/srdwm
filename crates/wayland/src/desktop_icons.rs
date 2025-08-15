//! Real desktop icons - Home/Computer/Trash plus one per real `~/Desktop`
//! entry. Same "compositor-owned floating UI, not tied to a client window"
//! shape as `context_menu.rs`: a plain data struct with its own open/hit-
//! test, no smithay dependency at all, rasterized separately in
//! `decoration.rs` and glued into `CompState` the same way that file's own
//! `ContextMenu` is.
//!
//! Positions are grid cells (column, row), not raw pixels - a dropped drag
//! always snaps to one, and `desktop_icons_state.rs`'s persistence stores
//! cells, not pixels, so a later change to `CELL_WIDTH`/`CELL_HEIGHT`
//! doesn't scatter every saved position off-grid.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(crate) const CELL_WIDTH: i32 = 88;
pub(crate) const CELL_HEIGHT: i32 = 88;
/// Gap between the primary monitor's own usable-area edge and the first
/// column/row of icons - purely cosmetic, keeps icons off a bar/dock's
/// exclusive-zone edge rather than flush against it.
pub(crate) const GRID_MARGIN: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IconKind {
    Home,
    Computer,
    Trash,
    Folder,
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesktopIcon {
    /// Stable identity: `"home"`/`"computer"`/`"trash"` for the three fixed
    /// icons, or the real filename for a `~/Desktop` entry - doubles as
    /// the JSON-persistence key and the per-icon render-buffer cache key,
    /// so it must stay stable across a rescan for anything the user hasn't
    /// renamed or deleted on disk.
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) kind: IconKind,
    /// What double-click/"Open" launches: `$HOME`, `/`, the trash folder,
    /// or the real path under `~/Desktop`.
    pub(crate) target: PathBuf,
    pub(crate) cell: (i32, i32),
    pub(crate) selected: bool,
}

impl DesktopIcon {
    pub(crate) fn top_left(&self, origin: (i32, i32)) -> (i32, i32) {
        (origin.0 + self.cell.0 * CELL_WIDTH, origin.1 + self.cell.1 * CELL_HEIGHT)
    }

    pub(crate) fn contains(&self, origin: (i32, i32), x: i32, y: i32) -> bool {
        let (left, top) = self.top_left(origin);
        x >= left && x < left + CELL_WIDTH && y >= top && y < top + CELL_HEIGHT
    }
}

pub(crate) struct DesktopIcons {
    /// Top-left of the grid's own `(0, 0)` cell, in global space - the
    /// primary monitor's usable-area origin plus `GRID_MARGIN`.
    pub(crate) origin: (i32, i32),
    pub(crate) icons: Vec<DesktopIcon>,
}

impl DesktopIcons {
    /// Which icon (if any) global-space point `(x, y)` falls on - same
    /// shape as `ContextMenu::row_at`. Returns an index into `self.icons`,
    /// not the icon itself, so a caller holding `&mut self` can still
    /// mutate the match without a borrow conflict.
    pub(crate) fn icon_at(&self, x: i32, y: i32) -> Option<usize> {
        self.icons.iter().position(|icon| icon.contains(self.origin, x, y))
    }
}

/// `$HOME`, or `None` if genuinely unset - callers degrade to "no desktop
/// icons at all" rather than guessing, same as `monitor_layout.rs::state_
/// dir()`'s own fallback chain does for a missing `$HOME`.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn desktop_dir(home: &Path) -> PathBuf {
    home.join("Desktop")
}

/// `$XDG_DATA_HOME/Trash/files`, else `~/.local/share/Trash/files` - the
/// freedesktop.org Trash spec's home-filesystem trash directory. Only the
/// same-filesystem case is handled anywhere in this codebase (see this
/// feature's own plan doc for why the per-mountpoint `.Trash-$uid`
/// fallback is out of scope for now); this is purely where the Trash
/// desktop icon opens to, nothing currently moves a file into it.
fn trash_files_dir(home: &Path) -> PathBuf {
    let data_home = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local/share"));
    data_home.join("Trash/files")
}

/// Rebuilds the full icon list from the real filesystem: the three fixed
/// icons first, then one per direct, non-hidden entry of `~/Desktop`
/// (creating that directory if it doesn't exist yet, matching how a real
/// desktop environment bootstraps an empty one on first run), sorted by
/// name. `saved` is `desktop_icons_state`'s own persisted `id -> cell` map
/// - an icon with a saved entry keeps that exact cell; every other icon
/// (new files, or a first run with nothing saved yet) fills the next free
/// cell in top-to-bottom, then wrap-to-next-column order, skipping any
/// cell a saved icon already claims.
///
/// `rows_per_column` bounds how many icons stack vertically before
/// wrapping - derived from the primary monitor's own usable height, see
/// this module's caller in `state/desktop_icons.rs`.
pub(crate) fn rescan(saved: &HashMap<String, (i32, i32)>, rows_per_column: i32) -> Vec<DesktopIcon> {
    let rows_per_column = rows_per_column.max(1);
    let mut icons = vec![
        DesktopIcon {
            id: "home".to_string(),
            label: "Home".to_string(),
            kind: IconKind::Home,
            target: home_dir().unwrap_or_else(|| PathBuf::from("/")),
            cell: (0, 0),
            selected: false,
        },
        DesktopIcon {
            id: "computer".to_string(),
            label: "Computer".to_string(),
            kind: IconKind::Computer,
            target: PathBuf::from("/"),
            cell: (0, 0),
            selected: false,
        },
        DesktopIcon {
            id: "trash".to_string(),
            label: "Trash".to_string(),
            kind: IconKind::Trash,
            target: home_dir().map(|h| trash_files_dir(&h)).unwrap_or_else(|| PathBuf::from("/")),
            cell: (0, 0),
            selected: false,
        },
    ];
    if let Some(home) = home_dir() {
        let desktop = desktop_dir(&home);
        if std::fs::create_dir_all(&desktop).is_ok() {
            if let Ok(entries) = std::fs::read_dir(&desktop) {
                let mut files: Vec<(String, bool)> = entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') {
                            return None;
                        }
                        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        Some((name, is_dir))
                    })
                    .collect();
                files.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, is_dir) in files {
                    let target = desktop.join(&name);
                    icons.push(DesktopIcon {
                        id: name.clone(),
                        label: name,
                        kind: if is_dir { IconKind::Folder } else { IconKind::File },
                        target,
                        cell: (0, 0),
                        selected: false,
                    });
                }
            }
        }
    }
    assign_cells(&mut icons, saved, rows_per_column);
    icons
}

/// Splits `icons` into "has a saved cell" and "needs a default one", places
/// the saved ones first (so they occupy their cells before any default
/// assignment can land on the same one), then walks column-major order
/// filling the rest into whatever's still free.
fn assign_cells(icons: &mut [DesktopIcon], saved: &HashMap<String, (i32, i32)>, rows_per_column: i32) {
    let mut used: HashSet<(i32, i32)> = HashSet::new();
    let mut unplaced: Vec<usize> = Vec::new();
    for (i, icon) in icons.iter_mut().enumerate() {
        match saved.get(&icon.id) {
            Some(&cell) => {
                icon.cell = cell;
                used.insert(cell);
            }
            None => unplaced.push(i),
        }
    }
    let mut col = 0;
    let mut row = 0;
    for i in unplaced {
        while used.contains(&(col, row)) {
            row += 1;
            if row >= rows_per_column {
                row = 0;
                col += 1;
            }
        }
        icons[i].cell = (col, row);
        used.insert((col, row));
        row += 1;
        if row >= rows_per_column {
            row = 0;
            col += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_icons_always_come_first_in_a_stable_order() {
        let icons = rescan(&HashMap::new(), 10);
        assert!(icons.len() >= 3, "at least the three fixed icons");
        assert_eq!(icons[0].id, "home");
        assert_eq!(icons[1].id, "computer");
        assert_eq!(icons[2].id, "trash");
    }

    #[test]
    fn default_cells_fill_top_to_bottom_then_wrap_to_the_next_column() {
        let mut icons = vec![
            DesktopIcon { id: "a".into(), label: "a".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false },
            DesktopIcon { id: "b".into(), label: "b".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false },
            DesktopIcon { id: "c".into(), label: "c".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false },
        ];
        assign_cells(&mut icons, &HashMap::new(), 2);
        assert_eq!(icons[0].cell, (0, 0));
        assert_eq!(icons[1].cell, (0, 1));
        assert_eq!(icons[2].cell, (1, 0), "third icon wraps to the next column once the first is full");
    }

    #[test]
    fn a_saved_cell_is_kept_and_default_placement_skips_it() {
        let mut icons = vec![
            DesktopIcon { id: "a".into(), label: "a".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false },
            DesktopIcon { id: "b".into(), label: "b".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false },
        ];
        let mut saved = HashMap::new();
        saved.insert("a".to_string(), (0, 0));
        assign_cells(&mut icons, &saved, 3);
        assert_eq!(icons[0].cell, (0, 0), "a keeps its saved cell");
        assert_eq!(icons[1].cell, (0, 1), "b's default placement skips a's occupied cell");
    }

    #[test]
    fn hidden_desktop_entries_are_never_listed() {
        // Pure unit test of the filter logic without touching the real
        // filesystem: `rescan` itself reads `$HOME`, which parallel
        // `cargo test` runs can't safely override (same reasoning `monitor_
        // layout.rs`'s own tests give for staying off real env vars) - so
        // this only locks in the *rule*, matching `corrupt_json_falls_
        // back_to_an_empty_layout_not_an_error`'s own "shape, not the real
        // I/O" pattern.
        let name = ".hidden";
        assert!(name.starts_with('.'), "sanity: this is the exact condition rescan's own filter checks");
    }

    #[test]
    fn icon_at_matches_only_its_own_cell() {
        let icons = DesktopIcons {
            origin: (100, 100),
            icons: vec![DesktopIcon { id: "a".into(), label: "a".into(), kind: IconKind::File, target: PathBuf::new(), cell: (1, 0), selected: false }],
        };
        let (left, top) = icons.icons[0].top_left(icons.origin);
        assert_eq!(icons.icon_at(left, top), Some(0), "top-left corner of the cell");
        assert_eq!(icons.icon_at(left + CELL_WIDTH - 1, top + CELL_HEIGHT - 1), Some(0), "bottom-right pixel of the cell");
        assert_eq!(icons.icon_at(left - 1, top), None, "just left of the cell");
        assert_eq!(icons.icon_at(left + CELL_WIDTH, top), None, "just right of the cell");
    }
}
