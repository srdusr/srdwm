//! Right-click desktop icon / bare-desktop menu - the sibling of
//! `context_menu.rs`'s titlebar window menu, same shape (own `open`/`row_
//! at`, rasterized via `decoration::render_context_menu`), for the two new
//! right-click targets desktop icons add: an icon itself, or bare desktop.

use crate::desktop_icons::{DesktopIcon, IconKind};

#[derive(Clone)]
pub(crate) enum DesktopMenuAction {
    /// Open the icon with this id - same action a double-click runs.
    OpenIcon(String),
    /// Enter inline rename mode for this real file/folder icon - see
    /// `CompState::renaming_icon`'s own doc comment.
    Rename(String),
    /// Move this real file/folder into `~/.local/share/Trash` - no
    /// confirmation, same as every mainstream file manager: this is the
    /// reversible move-to-trash, not a permanent delete.
    Delete(String),
    /// Empties `~/.local/share/Trash` entirely - same no-confirmation
    /// convention as `Delete`.
    EmptyTrash,
    NewFolder,
    /// "not even new file" - see `CompState::new_desktop_text_file`'s own
    /// doc comment.
    NewTextFile,
    /// Spawns a terminal with `~/Desktop` as its working directory --
    /// `general.terminal`, or a common-binary fallback list if unset.
    OpenTerminalHere,
    /// Opens `~/Desktop` itself in `general.file_manager`/`xdg-open` - the
    /// concrete path to a real file manager's own richer menu (cut/copy/
    /// paste, properties, ...), deliberately not reimplemented here.
    OpenInFileManager,
    /// Selects every desktop icon at once - the one bare-desktop menu
    /// action every mainstream file manager/desktop offers (Explorer,
    /// Nautilus, Finder) that this menu had no equivalent for at all,
    /// reported live as this menu needing "a lot more items".
    SelectAll,
    Refresh,
    /// A purely visual divider row - see `context_menu::MenuAction::
    /// Separator`'s own doc comment (same shape, same reason, separate
    /// enum since this menu and the titlebar one don't share one).
    Separator,
}

pub(crate) struct DesktopMenu {
    pub(crate) pos: (i32, i32),
    pub(crate) width: u32,
    pub(crate) row_height: u32,
    pub(crate) items: Vec<(&'static str, DesktopMenuAction)>,
}

const MENU_WIDTH: u32 = 170;
const ROW_HEIGHT: u32 = 28;

impl DesktopMenu {
    /// Right-click on `icon` itself - the action set depends on what kind
    /// of icon it is, not one fixed list: a real file/folder gets Open/
    /// Rename/Delete (real filesystem operations); Home/Computer (fixed
    /// shortcuts to somewhere, not real files of their own) get Open only,
    /// since renaming or deleting the shortcut itself isn't a meaningful
    /// action; Trash gets Open/Empty Trash instead of Rename/Delete, since
    /// "delete the trash" and "rename the trash" aren't real trash
    /// operations the way "empty it" is.
    pub(crate) fn open_for_icon(icon: &DesktopIcon, pos: (i32, i32)) -> Self {
        let items = match icon.kind {
            IconKind::Trash => vec![("Open", DesktopMenuAction::OpenIcon(icon.id.clone())), ("Empty Trash", DesktopMenuAction::EmptyTrash)],
            IconKind::Home | IconKind::Computer => vec![("Open", DesktopMenuAction::OpenIcon(icon.id.clone()))],
            IconKind::Folder | IconKind::File => vec![
                ("Open", DesktopMenuAction::OpenIcon(icon.id.clone())),
                ("Rename", DesktopMenuAction::Rename(icon.id.clone())),
                ("Delete", DesktopMenuAction::Delete(icon.id.clone())),
            ],
        };
        Self { pos, width: MENU_WIDTH, row_height: ROW_HEIGHT, items }
    }

    /// Right-click on bare desktop (no icon under the pointer).
    pub(crate) fn open_for_desktop(pos: (i32, i32)) -> Self {
        let items = vec![
            ("New Folder", DesktopMenuAction::NewFolder),
            ("New Text Document", DesktopMenuAction::NewTextFile),
            ("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", DesktopMenuAction::Separator),
            ("Open Terminal Here", DesktopMenuAction::OpenTerminalHere),
            ("Open in File Manager", DesktopMenuAction::OpenInFileManager),
            ("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", DesktopMenuAction::Separator),
            ("Select All", DesktopMenuAction::SelectAll),
            ("Refresh", DesktopMenuAction::Refresh),
        ];
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

    fn icon(kind: IconKind) -> DesktopIcon {
        DesktopIcon { id: "x".into(), label: "x".into(), kind, target: PathBuf::new(), cell: (0, 0), selected: false }
    }

    #[test]
    fn a_real_file_gets_open_rename_and_delete() {
        let menu = DesktopMenu::open_for_icon(&icon(IconKind::File), (0, 0));
        let labels: Vec<&str> = menu.items.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, vec!["Open", "Rename", "Delete"]);
    }

    #[test]
    fn a_real_folder_gets_open_rename_and_delete_too() {
        let menu = DesktopMenu::open_for_icon(&icon(IconKind::Folder), (0, 0));
        let labels: Vec<&str> = menu.items.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, vec!["Open", "Rename", "Delete"]);
    }

    #[test]
    fn home_and_computer_get_open_only() {
        for kind in [IconKind::Home, IconKind::Computer] {
            let menu = DesktopMenu::open_for_icon(&icon(kind), (0, 0));
            let labels: Vec<&str> = menu.items.iter().map(|(l, _)| *l).collect();
            assert_eq!(labels, vec!["Open"], "shortcuts aren't real files - no rename/delete");
        }
    }

    #[test]
    fn trash_gets_open_and_empty_trash_not_rename_or_delete() {
        let menu = DesktopMenu::open_for_icon(&icon(IconKind::Trash), (0, 0));
        let labels: Vec<&str> = menu.items.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, vec!["Open", "Empty Trash"]);
    }

    #[test]
    fn desktop_menu_offers_the_full_set() {
        let menu = DesktopMenu::open_for_desktop((10, 10));
        let real_actions: Vec<&str> =
            menu.items.iter().filter(|(_, a)| !matches!(a, DesktopMenuAction::Separator)).map(|(l, _)| *l).collect();
        assert_eq!(real_actions, vec!["New Folder", "New Text Document", "Open Terminal Here", "Open in File Manager", "Select All", "Refresh"]);
    }

    #[test]
    fn row_at_maps_a_point_to_the_right_row() {
        let menu = DesktopMenu::open_for_desktop((100, 100));
        assert_eq!(menu.row_at(150, 100), Some(0));
        assert_eq!(menu.row_at(150, 100 + ROW_HEIGHT as i32), Some(1));
        assert_eq!(menu.row_at(150, 100 + menu.height()), None, "just below the menu");
    }
}
