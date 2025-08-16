//! freedesktop.org Trash specification - move-to-trash and empty-trash.
//! Same-filesystem case only: `~/Desktop` and `~/.local/share/Trash` are
//! virtually always the same filesystem in practice, and a cross-
//! filesystem move needs its own per-mountpoint `<mountpoint>/.Trash-$uid`
//! directory the spec defines separately - not attempted here.
//! `std::fs::rename` across filesystems fails with a clear `EXDEV` error
//! rather than silently doing the wrong thing, so this degrades to a
//! reported failure, never silent data loss.
//!
//! No confirmation prompt anywhere in this module, by design: moving
//! something to trash is the reversible operation, not the destructive
//! one - every mainstream file manager (Nemo included) treats it exactly
//! this way, gating only a *permanent* delete (which nothing here does)
//! behind a dialog.

use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME/Trash`, else `~/.local/share/Trash`.
fn trash_root(home: &Path) -> PathBuf {
    let data_home = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local/share"));
    data_home.join("Trash")
}

pub(crate) fn files_dir(home: &Path) -> PathBuf {
    trash_root(home).join("files")
}

fn info_dir(home: &Path) -> PathBuf {
    trash_root(home).join("info")
}

// Not called yet - reserved for the Trash icon's own full/empty glyph
// selection, landing later in this same round alongside real icon-theme
// rendering.
#[allow(dead_code)]
pub(crate) fn is_empty(home: &Path) -> bool {
    std::fs::read_dir(files_dir(home)).map(|mut d| d.next().is_none()).unwrap_or(true)
}

/// De-duplicates `name` against whatever's already in `dir` - same
/// `"name (2)"`, `"name (3)"`, ... scheme as `desktop_icons::new_desktop_
/// folder`'s own "New Folder (2)". The spec requires unique names in
/// `files/` but doesn't mandate a specific collision scheme.
fn dedup_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), Some(e.to_string())),
        _ => (name.to_string(), None),
    };
    let mut n = 2;
    loop {
        let candidate = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Moves `path` into the trash, writing its `.trashinfo` metadata first --
/// the spec requires that file to exist before the item lands in `files/`,
/// so a reader never sees a trashed item with no metadata explaining it.
/// Rolls the info file back if the actual move then fails, so a failure
/// here never leaves an orphaned `.trashinfo` for something that's still
/// sitting exactly where it started.
pub(crate) fn move_to_trash(path: &Path) -> std::io::Result<()> {
    let home = std::env::var("HOME").map(PathBuf::from).map_err(|_| std::io::Error::other("HOME not set"))?;
    let files = files_dir(&home);
    let info = info_dir(&home);
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no filename"))?;
    let trashed_name = dedup_name(&files, name);
    let info_content = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", percent_encode_path(path), now_iso8601());
    let info_path = info.join(format!("{trashed_name}.trashinfo"));
    std::fs::write(&info_path, info_content)?;
    if let Err(e) = std::fs::rename(path, files.join(&trashed_name)) {
        let _ = std::fs::remove_file(&info_path);
        return Err(e);
    }
    Ok(())
}

/// Removes every entry under both `files/` and `info/`.
pub(crate) fn empty(home: &Path) {
    for dir in [files_dir(home), info_dir(home)] {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let result = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            if let Err(e) = result {
                log::warn!("trash: couldn't remove {path:?}: {e}");
            }
        }
    }
}

/// Minimal percent-encoding for the `.trashinfo` `Path=` field - only
/// needs to be a *safe* encoding (every byte a later parser could choke
/// on gets escaped), not a byte-perfect implementation, since nothing in
/// this codebase ever reads this field back; a real trash-viewing tool
/// (Nemo) is what actually parses it.
fn percent_encode_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `YYYY-MM-DDTHH:MM:SS`, local system clock - hand-rolled rather than
/// pulling in `chrono`/`time` for one timestamp field. `civil_from_days`
/// is Howard Hinnant's well-known public-domain algorithm for converting
/// a day count into a proleptic-Gregorian (year, month, day); this
/// codebase has no other date-formatting need that would justify a real
/// date/time dependency.
fn now_iso8601() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let days = (now.as_secs() / 86400) as i64;
    let secs_of_day = now.as_secs() % 86400;
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_name_keeps_an_unused_name_as_is() {
        let dir = std::env::temp_dir().join(format!("srdwm-trash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(dedup_name(&dir, "report.txt"), "report.txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dedup_name_numbers_a_collision_preserving_the_extension() {
        let dir = std::env::temp_dir().join(format!("srdwm-trash-test-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.txt"), b"").unwrap();
        assert_eq!(dedup_name(&dir, "report.txt"), "report (2).txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dedup_name_numbers_a_collision_with_no_extension() {
        let dir = std::env::temp_dir().join(format!("srdwm-trash-test-noext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("New Folder"), b"").unwrap();
        assert_eq!(dedup_name(&dir, "New Folder"), "New Folder (2)");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn percent_encode_path_leaves_ordinary_path_characters_alone() {
        assert_eq!(percent_encode_path(Path::new("/home/x/Desktop/report.txt")), "/home/x/Desktop/report.txt");
    }

    #[test]
    fn percent_encode_path_escapes_a_space() {
        assert_eq!(percent_encode_path(Path::new("/home/x/New Folder")), "/home/x/New%20Folder");
    }

    #[test]
    fn civil_from_days_matches_the_unix_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_matches_a_known_recent_date() {
        // 2026-08-26 is 20,691 days after the epoch (verified independently:
        // `python3 -c "import datetime; print((datetime.date(2026,8,26) -
        // datetime.date(1970,1,1)).days)"`).
        assert_eq!(civil_from_days(20691), (2026, 8, 26));
    }

    #[test]
    fn move_to_trash_and_empty_round_trip_on_a_real_temp_file() {
        let home = std::env::temp_dir().join(format!("srdwm-trash-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        // `move_to_trash` reads real $HOME/$XDG_DATA_HOME - not safely
        // parallel-testable via env var override (same reasoning `monitor_
        // layout.rs`'s own tests give), so this test exercises `empty`/
        // `is_empty` directly against a fabricated trash layout instead of
        // going through `move_to_trash`'s own env lookup.
        let files = files_dir(&home);
        let info = info_dir(&home);
        std::fs::create_dir_all(&files).unwrap();
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(files.join("old.txt"), b"x").unwrap();
        std::fs::write(info.join("old.txt.trashinfo"), b"[Trash Info]\n").unwrap();
        assert!(!is_empty(&home));
        empty(&home);
        assert!(is_empty(&home));
        assert_eq!(std::fs::read_dir(&info).unwrap().count(), 0);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
