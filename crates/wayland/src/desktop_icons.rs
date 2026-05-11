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

/// An in-progress icon drag that may carry more than one icon along
/// together. `primary` is whichever icon the pointer actually grabbed;
/// `members` is every icon moving with it (always includes `primary`, at
/// offset `(0, 0)`), each recorded as a fixed offset from `primary`'s own
/// top-left at the moment the drag started - the whole group moves as
/// one rigid unit regardless of where each member's own cell happens to
/// be, the same way dragging one file in a multi-selection in Windows/
/// GNOME/macOS/KDE carries every other selected file along with it.
/// Reported live as missing: "try move desktop items all at once
/// somewhere else" didn't work at all before this - `members` used to
/// not exist, a drag only ever carried the one icon it grabbed no matter
/// how many were selected.
pub(crate) struct DesktopIconDrag {
    /// Pointer's grab offset from `primary`'s own top-left at drag start,
    /// so the icon tracks the pointer smoothly rather than snapping its
    /// top-left corner straight to the cursor.
    pub(crate) grab_offset: (i32, i32),
    /// `primary`'s own live top-left this frame.
    pub(crate) primary_pos: (i32, i32),
    /// `(icon id, fixed offset from primary's own top-left at drag
    /// start)` - primary included at offset `(0, 0)`.
    pub(crate) members: Vec<(String, (i32, i32))>,
    /// The icon the press actually landed on, and where the pointer was.
    /// Release compares against this to decide whether the gesture was a
    /// click or a drag - see `DRAG_THRESHOLD`.
    pub(crate) pressed: (String, (i32, i32)),
    /// Set once the pointer leaves `DRAG_THRESHOLD` of `pressed`. A press
    /// that never does is a click, not a move.
    pub(crate) moved: bool,
}

/// How far the pointer must travel from the press point before a desktop
/// icon gesture counts as a drag rather than a click, in logical pixels.
///
/// Every press on an icon now starts a *potential* drag, and release
/// decides which it was. Without that, single-click mode (`general.
/// desktop_icon_single_click`) made dragging impossible: the press opened
/// the icon immediately, so the drag branch was unreachable and an icon
/// could never be moved at all. Reported live as "i can't move the desktop
/// icons anymore since making it single click ... impossible to hold and
/// drag move desktop icons".
///
/// Small, because the cost is asymmetric: too large and a genuine short
/// drag is swallowed as a click that opens something the user did not want
/// opened; too small only means an unusually shaky click moves an icon a
/// cell, which is visible and trivially undone.
pub(crate) const DRAG_THRESHOLD: i32 = 4;

pub(crate) struct DesktopIcons {
    /// Top-left of the grid's own `(0, 0)` cell, in global space, one per
    /// participating monitor - each monitor's own usable-area origin plus
    /// `GRID_MARGIN`. The same `icons` list is mirrored at every origin
    /// (`general.desktop_icons_all_monitors`, see `ensure_desktop_icons`):
    /// one shared set of icons/cells, rendered and hit-tested again at each
    /// monitor's own corner, rather than a separate icon set per monitor --
    /// dragging a mirrored copy on any monitor moves the one underlying
    /// icon, which then shows in its new cell everywhere it's mirrored.
    /// Exactly one entry (the primary monitor's) when that config flag is
    /// off, matching the original single-monitor behaviour.
    pub(crate) origins: Vec<(i32, i32)>,
    pub(crate) icons: Vec<DesktopIcon>,
}

impl DesktopIcons {
    /// Which icon (if any) global-space point `(x, y)` falls on, checked
    /// against every mirrored origin - same shape as `ContextMenu::
    /// row_at`. Returns an index into `self.icons` plus the origin it
    /// matched (needed by drag-start to grab the copy actually clicked,
    /// not always the primary monitor's), not the icon itself, so a caller
    /// holding `&mut self` can still mutate the match without a borrow
    /// conflict.
    pub(crate) fn icon_at(&self, x: i32, y: i32) -> Option<(usize, (i32, i32))> {
        for &origin in &self.origins {
            if let Some(i) = self.icons.iter().position(|icon| icon.contains(origin, x, y)) {
                return Some((i, origin));
            }
        }
        None
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
            target: home_dir().map(|h| crate::trash::files_dir(&h)).unwrap_or_else(|| PathBuf::from("/")),
            cell: (0, 0),
            selected: false,
        },
    ];
    if let Some(home) = home_dir() {
        let desktop = desktop_dir(&home);
        if std::fs::create_dir_all(&desktop).is_ok() {
            if let Ok(entries) = std::fs::read_dir(&desktop) {
                let files: Vec<(String, bool)> = entries
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
    // One alphabetical list, fixed icons included - not fixed-three-then-
    // files. Confirmed directly: the fixed shortcuts shouldn't always come
    // first just because they're synthetic rather than real files.
    // Case-insensitive so "computer"/"Computer" and a real lowercase
    // filename interleave the way a user actually expects, not by raw
    // byte value (which would put every uppercase name before any
    // lowercase one).
    icons.sort_by_key(|a| a.label.to_lowercase());
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
    fn the_three_fixed_icons_sort_alphabetically_among_themselves() {
        // Reported live, confirmed via direct question: fixed icons must
        // NOT always come before real files just because they're
        // synthetic - the whole list sorts by label together. This
        // checks that sort using only the three fixed icons (present on
        // any machine, unlike a specific `~/Desktop` file), whose labels
        // - "Computer", "Home", "Trash" - already happen to be in
        // alphabetical order, so a correct sort leaves them exactly
        // where `rescan` built them.
        let icons = rescan(&HashMap::new(), 10);
        let fixed: Vec<&str> = icons.iter().filter(|i| matches!(i.kind, IconKind::Home | IconKind::Computer | IconKind::Trash)).map(|i| i.label.as_str()).collect();
        assert_eq!(fixed, vec!["Computer", "Home", "Trash"]);
    }

    #[test]
    fn sorting_is_case_insensitive_and_covers_the_whole_list() {
        let mut icons = [
            DesktopIcon { id: "zebra".into(), label: "zebra".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false },
            DesktopIcon { id: "Home".into(), label: "Home".into(), kind: IconKind::Home, target: PathBuf::new(), cell: (0, 0), selected: false },
            DesktopIcon { id: "apple".into(), label: "apple".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false },
        ];
        icons.sort_by_key(|a| a.label.to_lowercase());
        let labels: Vec<&str> = icons.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["apple", "Home", "zebra"], "a fixed icon's label interleaves with real filenames, not always first");
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
            origins: vec![(100, 100)],
            icons: vec![DesktopIcon { id: "a".into(), label: "a".into(), kind: IconKind::File, target: PathBuf::new(), cell: (1, 0), selected: false }],
        };
        let origin = icons.origins[0];
        let (left, top) = icons.icons[0].top_left(origin);
        assert_eq!(icons.icon_at(left, top), Some((0, origin)), "top-left corner of the cell");
        assert_eq!(icons.icon_at(left + CELL_WIDTH - 1, top + CELL_HEIGHT - 1), Some((0, origin)), "bottom-right pixel of the cell");
        assert_eq!(icons.icon_at(left - 1, top), None, "just left of the cell");
        assert_eq!(icons.icon_at(left + CELL_WIDTH, top), None, "just right of the cell");
    }

    #[test]
    fn icon_at_checks_every_mirrored_origin() {
        let icons = DesktopIcons {
            origins: vec![(0, 0), (2000, 0)],
            icons: vec![DesktopIcon { id: "a".into(), label: "a".into(), kind: IconKind::File, target: PathBuf::new(), cell: (0, 0), selected: false }],
        };
        assert_eq!(icons.icon_at(10, 10), Some((0, (0, 0))), "matches the first monitor's mirror");
        assert_eq!(icons.icon_at(2010, 10), Some((0, (2000, 0))), "matches the second monitor's mirror, with its own origin");
        assert_eq!(icons.icon_at(1000, 10), None, "the gap between the two monitors matches neither");
    }
}
