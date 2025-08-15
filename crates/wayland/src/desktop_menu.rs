//! Right-click desktop icon / bare-desktop menu - the sibling of
//! `context_menu.rs`'s titlebar window menu, same shape (own `open`/`row_
//! at`, rasterized via `decoration::render_context_menu`), for the two new
//! right-click targets desktop icons add: an icon itself, or bare desktop.

use crate::desktop_icons::{DesktopIcon, IconKind};

#[derive(Clone)]
pub(crate) enum DesktopMenuAction {
    /// Open the icon with this id - same action a double-click runs.
    OpenIcon(String),
    /// Shell out to `general.wallpaper_command` with this icon's path --
    /// only ever offered for an image-file icon, and only when that
    /// config key is actually set (see `WindowManager::wallpaper_command`'s
    /// own doc comment).
    SetWallpaper(String),
    NewFolder,
    Refresh,
}

pub(crate) struct DesktopMenu {
    pub(crate) pos: (i32, i32),
    pub(crate) width: u32,
    pub(crate) row_height: u32,
    pub(crate) items: Vec<(&'static str, DesktopMenuAction)>,
}

const MENU_WIDTH: u32 = 170;
const ROW_HEIGHT: u32 = 28;

/// Extensions `render_desktop_icon`'s `IconKind::File` icons treat as an
/// image for "Set as Wallpaper" purposes - not a real mimetype sniff (no
/// such capability exists anywhere in this workspace, see `desktop_icons.
/// rs`'s own doc comment on why icon art itself is hand-drawn, not
/// decoded), just the common raster formats a wallpaper tool actually
/// accepts.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif"];

fn is_image_path(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(|e| IMAGE_EXTENSIONS.iter().any(|ext| e.eq_ignore_ascii_case(ext))).unwrap_or(false)
}

impl DesktopMenu {
    /// Right-click on `icon` itself: "Open" always, plus "Set as Wallpaper"
    /// when `icon` is an image file and `wallpaper_command` is non-empty.
    pub(crate) fn open_for_icon(icon: &DesktopIcon, pos: (i32, i32), wallpaper_command: &str) -> Self {
        let mut items = vec![("Open", DesktopMenuAction::OpenIcon(icon.id.clone()))];
        if icon.kind == IconKind::File && !wallpaper_command.is_empty() && is_image_path(&icon.target) {
            items.push(("Set as Wallpaper", DesktopMenuAction::SetWallpaper(icon.id.clone())));
        }
        Self { pos, width: MENU_WIDTH, row_height: ROW_HEIGHT, items }
    }

    /// Right-click on bare desktop (no icon under the pointer): "New
    /// Folder" and "Refresh".
    pub(crate) fn open_for_desktop(pos: (i32, i32)) -> Self {
        let items = vec![("New Folder", DesktopMenuAction::NewFolder), ("Refresh", DesktopMenuAction::Refresh)];
        Self { pos, width: MENU_WIDTH, row_height: ROW_HEIGHT, items }
    }

    pub(crate) fn height(&self) -> i32 {
        self.row_height as i32 * self.items.len() as i32
    }

    /// Same shape as `ContextMenu::row_at`.
    pub(crate) fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        if x < self.pos.0 || x >= self.pos.0 + self.width as i32 {
            return None;
        }
        let rel_y = y - self.pos.1;
        if rel_y < 0 || rel_y >= self.height() {
            return None;
        }
        Some((rel_y / self.row_height as i32) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn icon(kind: IconKind, target: &str) -> DesktopIcon {
        DesktopIcon { id: "x".into(), label: "x".into(), kind, target: PathBuf::from(target), cell: (0, 0), selected: false }
    }

    #[test]
    fn image_file_with_a_configured_command_gets_the_wallpaper_row() {
        let menu = DesktopMenu::open_for_icon(&icon(IconKind::File, "pic.png"), (0, 0), "swww img");
        assert_eq!(menu.items.len(), 2);
        assert_eq!(menu.items[1].0, "Set as Wallpaper");
    }

    #[test]
    fn image_file_with_no_configured_command_has_no_wallpaper_row() {
        let menu = DesktopMenu::open_for_icon(&icon(IconKind::File, "pic.png"), (0, 0), "");
        assert_eq!(menu.items.len(), 1, "Open only");
    }

    #[test]
    fn non_image_file_has_no_wallpaper_row_even_with_a_command_configured() {
        let menu = DesktopMenu::open_for_icon(&icon(IconKind::File, "notes.txt"), (0, 0), "swww img");
        assert_eq!(menu.items.len(), 1, "Open only");
    }

    #[test]
    fn a_folder_never_gets_the_wallpaper_row() {
        let menu = DesktopMenu::open_for_icon(&icon(IconKind::Folder, "pic.png"), (0, 0), "swww img");
        assert_eq!(menu.items.len(), 1, "a directory named like an image is still not a file");
    }

    #[test]
    fn desktop_menu_offers_new_folder_and_refresh() {
        let menu = DesktopMenu::open_for_desktop((10, 10));
        assert_eq!(menu.items[0].0, "New Folder");
        assert_eq!(menu.items[1].0, "Refresh");
    }

    #[test]
    fn row_at_maps_a_point_to_the_right_row() {
        let menu = DesktopMenu::open_for_desktop((100, 100));
        assert_eq!(menu.row_at(150, 100), Some(0));
        assert_eq!(menu.row_at(150, 100 + ROW_HEIGHT as i32), Some(1));
        assert_eq!(menu.row_at(150, 100 + menu.height()), None, "just below the menu");
    }
}
