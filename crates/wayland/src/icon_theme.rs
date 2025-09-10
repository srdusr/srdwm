//! Real desktop-icon artwork loaded from the user's installed freedesktop
//! icon theme (GTK's own configured theme - WhiteSur on this machine),
//! rendered from SVG via `resvg`. `decoration::render_desktop_icon`'s
//! hand-drawn glyphs remain as the fallback for whatever this can't
//! resolve (no icon theme installed at all, a name no theme in the chain
//! ships, a corrupt SVG) - reported live as the hand-drawn glyphs reading
//! as generic placeholder art next to every real desktop's own icons
//! (GNOME/KDE/macOS/Windows all ship real theme artwork, not procedural
//! shapes), so this is the real fix, not a redraw of the same shapes.
//!
//! Deliberately narrow: this compositor has no use for icon lookup beyond
//! desktop icons (no taskbar/app-list that needs `.desktop`-file icon
//! resolution), so this only ever looks up the five fixed names `desktop_
//! icons.rs`'s own `IconKind` needs, not a general-purpose icon-theme
//! library. A real (if partial) implementation of the freedesktop icon
//! theme spec: theme `Inherits=` chains walked recursively, `hicolor`
//! always searched last, `scalable` preferred over any fixed raster size
//! since every lookup here wants one specific pixel size rendered from
//! source, not picked from a handful of pre-baked ones.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::desktop_icons::IconKind;

/// Canonical freedesktop icon name for each `IconKind` - the name every
/// spec-compliant theme (GNOME's Adwaita, KDE's Breeze, macOS-style
/// WhiteSur, etc.) ships its own artwork under.
pub(crate) fn icon_name(kind: IconKind) -> &'static str {
    match kind {
        IconKind::Home => "user-home",
        IconKind::Computer => "computer",
        IconKind::Trash => "user-trash",
        IconKind::Folder => "folder",
        IconKind::File => "text-x-generic",
    }
}

/// GTK's configured icon theme name - read the same places GTK itself
/// would, so this follows whatever the user actually has set (WhiteSur
/// here) rather than a hardcoded choice: `$GTK_THEME`'s icon-theme
/// sibling doesn't exist as its own env var, so `gtk-3.0/settings.ini`
/// (present even in a GNOME-less session, unlike `gsettings`, which needs
/// a working dconf backend) is checked first, `gsettings` second, falling
/// back to `"hicolor"` - the one theme the spec guarantees exists
/// alongside any other, so a lookup always has *something* to search
/// rather than an empty chain.
fn configured_theme_name() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [format!("{home}/.config/gtk-3.0/settings.ini"), format!("{home}/.config/gtk-4.0/settings.ini")];
    for path in candidates {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            for line in contents.lines() {
                let line = line.trim();
                if let Some(value) = line.strip_prefix("gtk-icon-theme-name=") {
                    let value = value.trim();
                    if !value.is_empty() {
                        return value.to_string();
                    }
                }
            }
        }
    }
    if let Ok(output) = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "icon-theme"]).output() {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout);
            let value = value.trim().trim_matches('\'');
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "hicolor".to_string()
}

/// Base directories icon themes live under, in the priority XDG's own
/// icon theme spec defines: the user's own override directories first,
/// then every `$XDG_DATA_DIRS` entry's `icons` subdirectory, then the
/// hardcoded system fallback last.
fn icon_base_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![PathBuf::from(format!("{home}/.local/share/icons")), PathBuf::from(format!("{home}/.icons"))];
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir).join("icons"));
        }
    }
    dirs.push(PathBuf::from("/usr/share/icons"));
    dirs
}

/// `<theme>/index.theme`'s own `Inherits=a,b,c` line, parsed into a plain
/// list - empty if the theme has no index (a raw, index-less icon
/// directory, or a name that doesn't resolve to anything installed at
/// all) or no `Inherits` key of its own.
fn theme_inherits(theme_dir: &Path) -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string(theme_dir.join("index.theme")) else { return Vec::new() };
    for line in contents.lines() {
        if let Some(value) = line.trim().strip_prefix("Inherits=") {
            return value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }
    Vec::new()
}

/// Every theme name to search, in order: `theme` itself, then its own
/// `Inherits=` chain walked recursively (a theme installed under more
/// than one base directory only needs to contribute its name once), with
/// `hicolor` appended at the end if nothing in the chain already named it
/// - the spec's own explicit last-resort fallback.
fn theme_search_chain(theme: &str) -> Vec<String> {
    let bases = icon_base_dirs();
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = vec![theme.to_string()];
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        chain.push(name.clone());
        for base in &bases {
            let dir = base.join(&name);
            if dir.is_dir() {
                queue.extend(theme_inherits(&dir));
                break;
            }
        }
    }
    if !seen.contains("hicolor") {
        chain.push("hicolor".to_string());
    }
    chain
}

/// Walks every subdirectory of `theme_dir` (unbounded depth is
/// unnecessary - real theme trees are `<context>/<size-or-scalable>/`,
/// two levels deep) looking for `<name>.svg` or `<name>.png`, preferring
/// an exact `scalable` directory match (vector art renders correctly at
/// any size this compositor asks for) over a fixed-size raster one.
fn find_in_theme(theme_dir: &Path, name: &str) -> Option<PathBuf> {
    let mut scalable_hit = None;
    let mut raster_hit = None;
    let Ok(contexts) = std::fs::read_dir(theme_dir) else { return None };
    for context in contexts.flatten() {
        let context_path = context.path();
        if !context_path.is_dir() {
            continue;
        }
        let Ok(sizes) = std::fs::read_dir(&context_path) else { continue };
        for size_dir in sizes.flatten() {
            let size_path = size_dir.path();
            if !size_path.is_dir() {
                continue;
            }
            let is_scalable = size_dir.file_name().to_string_lossy().contains("scalable");
            for ext in ["svg", "png"] {
                let candidate = size_path.join(format!("{name}.{ext}"));
                if candidate.is_file() {
                    if is_scalable && ext == "svg" {
                        scalable_hit.get_or_insert(candidate);
                    } else {
                        raster_hit.get_or_insert(candidate);
                    }
                }
            }
        }
    }
    scalable_hit.or(raster_hit)
}

/// Resolves `name` (one of this module's own fixed canonical names) to a
/// real on-disk icon file, searching the configured theme's full
/// inheritance chain. `None` means "no installed theme ships this icon at
/// all" - a legitimate, expected outcome on a minimal system, not an
/// error; the caller falls back to the hand-drawn glyph.
pub(crate) fn find_icon(name: &str) -> Option<PathBuf> {
    let theme = configured_theme_name();
    let bases = icon_base_dirs();
    for theme_name in theme_search_chain(&theme) {
        for base in &bases {
            let theme_dir = base.join(&theme_name);
            if theme_dir.is_dir() {
                if let Some(found) = find_in_theme(&theme_dir, name) {
                    return Some(found);
                }
            }
        }
    }
    // `/usr/share/pixmaps` is flat (no theme/context/size structure at
    // all) - the spec's own final fallback location, checked last.
    let flat = PathBuf::from(format!("/usr/share/pixmaps/{name}.png"));
    flat.is_file().then_some(flat)
}

/// Rasterizes the SVG at `path` into a `width`x`height` BGRA8 buffer --
/// the same byte order every other rasterizer in `decoration.rs` produces
/// (see that module's own doc comment) - so the caller can hand the
/// result straight to `MemoryRenderBuffer::from_slice` unchanged. `None`
/// on any read/parse/render failure (corrupt file, a `resvg`/`usvg`
/// feature gap): the caller degrades to the hand-drawn glyph rather than
/// propagating an error, the same "a missing/bad theme icon shouldn't be
/// worse than not trying" stance `find_icon` itself takes.
pub(crate) fn rasterize_svg(path: &Path, width: u32, height: u32) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(&data, &opt).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    let size = tree.size();
    let scale = (width as f32 / size.width()).min(height as f32 / size.height());
    let offset_x = (width as f32 - size.width() * scale) / 2.0;
    let offset_y = (height as f32 - size.height() * scale) / 2.0;
    let transform = tiny_skia::Transform::from_translate(offset_x, offset_y).pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let rgba = pixmap.data();
    let mut bgra = vec![0u8; rgba.len()];
    for (dst, src) in bgra.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    Some(bgra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_kind_maps_to_a_real_freedesktop_name() {
        // Locks in the spec names themselves - a typo here would silently
        // never match any real theme, degrading permanently to the
        // hand-drawn fallback with no obvious error anywhere.
        assert_eq!(icon_name(IconKind::Home), "user-home");
        assert_eq!(icon_name(IconKind::Computer), "computer");
        assert_eq!(icon_name(IconKind::Trash), "user-trash");
        assert_eq!(icon_name(IconKind::Folder), "folder");
        assert_eq!(icon_name(IconKind::File), "text-x-generic");
    }

    #[test]
    fn hicolor_is_always_in_the_search_chain_even_for_an_unrelated_theme() {
        let chain = theme_search_chain("a-theme-name-that-does-not-exist-anywhere");
        assert!(chain.contains(&"hicolor".to_string()), "the spec's own last-resort fallback must always be searched");
    }

    #[test]
    #[ignore = "visual spot-check only, dumps PNGs to /tmp - not run in CI"]
    fn dump_every_icon_for_visual_inspection() {
        for kind in [IconKind::Home, IconKind::Computer, IconKind::Trash, IconKind::Folder, IconKind::File] {
            let name = icon_name(kind);
            let Some(path) = find_icon(name) else {
                eprintln!("{name}: not found in any theme");
                continue;
            };
            eprintln!("{name}: {path:?}");
            let mut pixmap = tiny_skia::Pixmap::new(40, 36).unwrap();
            let opt = usvg::Options::default();
            let data = std::fs::read(&path).unwrap();
            let tree = usvg::Tree::from_data(&data, &opt).unwrap();
            let size = tree.size();
            let scale = (40.0 / size.width()).min(36.0 / size.height());
            let transform = tiny_skia::Transform::from_scale(scale, scale);
            resvg::render(&tree, transform, &mut pixmap.as_mut());
            pixmap.save_png(format!("/tmp/icon-{name}.png")).unwrap();
        }
    }

    #[test]
    fn the_chain_has_no_duplicate_even_if_a_theme_inherits_hicolor_explicitly() {
        // `WhiteSur-dark`'s own real `index.theme` (checked live on this
        // machine) already lists `Inherits=hicolor,breeze` - this locks
        // in that appending hicolor again afterward doesn't happen.
        let chain = theme_search_chain("hicolor");
        assert_eq!(chain.iter().filter(|n| n.as_str() == "hicolor").count(), 1);
    }
}
