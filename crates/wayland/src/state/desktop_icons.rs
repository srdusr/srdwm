//! `CompState` glue for desktop icons - open/rescan/select/drag/persist,
//! plus the right-click `DesktopMenu`'s own open/close/run-action. Same
//! shape as `state/menu.rs`'s `ContextMenu`/`SnapFlyout` glue.

use super::*;
use crate::desktop_icons::{DesktopIcons, IconKind, CELL_HEIGHT, CELL_WIDTH, GRID_MARGIN};
use crate::desktop_menu::{DesktopMenu, DesktopMenuAction};

/// Common terminal binaries tried, in order, when `general.terminal` is
/// unset - first one actually found on `$PATH` wins. There's no `xdg-
/// open`-equivalent dispatcher for "a shell" the way there is for a file,
/// so unlike `file_manager`'s empty-means-`xdg-open` fallback, this needs
/// a real candidate list.
const TERMINAL_CANDIDATES: &[&str] = &["alacritty", "kitty", "wezterm", "foot", "gnome-terminal", "konsole", "xterm"];

impl CompState {
    /// Populates `self.desktop_icons` on first call, then keeps its
    /// `origin` continuously re-anchored to the primary monitor's own
    /// *current* usable geometry on every later call - cheap enough
    /// (one tuple comparison) to run unconditionally at the top of every
    /// render pass. Does nothing at all when the config flag is off.
    ///
    /// The re-anchoring is the fix for a real, reported bug: this used to
    /// return immediately once `self.desktop_icons` was `Some`, computing
    /// `origin` exactly once, on whichever render pass happened to be
    /// first. AGS's own top bar registers its exclusive zone (`general.
    /// desktop_icons`' fix's own PR: `srd monitors`, `Monitor::geometry`
    /// already excludes it) only once that separate client has connected
    /// and committed - reliably *after* this compositor's own first
    /// render pass, confirmed live via a temporary diagnostic log: origin
    /// baked in at `(1936, 16)` (bar not yet registered, `geometry.y` was
    /// still `0`) and never moved again even once `srd monitors` reported
    /// the bar's real 34-40px reservation moments later - reported live
    /// as "Home is still being overlapped by AGS's top bar." Re-deriving
    /// `origin` every call (not rebuilding the icon list - render
    /// position reads `origin` fresh at push time, never baked into a
    /// cached glyph buffer) closes this permanently, for the bar, a dock
    /// on any edge, or any later exclusive-zone change alike.
    pub(crate) fn ensure_desktop_icons(&mut self) {
        if !self.wm.borrow().desktop_icons_enabled {
            return;
        }
        let Some(monitor) = self.wm.borrow().monitors().iter().find(|m| m.primary).cloned() else { return };
        let origin = (monitor.geometry.x + GRID_MARGIN, monitor.geometry.y + GRID_MARGIN);
        if let Some(icons) = &mut self.desktop_icons {
            icons.origin = origin;
            return;
        }
        let rows = ((monitor.geometry.height as i32 - 2 * GRID_MARGIN) / CELL_HEIGHT).max(1);
        let saved = crate::desktop_icons_state::load();
        let icons = crate::desktop_icons::rescan(&saved, rows);
        self.desktop_icons = Some(DesktopIcons { origin, icons });
        self.desktop_icon_buffers.clear();
    }

    /// Re-derives the icon list from the real filesystem (a new/removed
    /// `~/Desktop` entry) without disturbing any already-persisted cell --
    /// `rescan` itself already only assigns a fresh default cell to an
    /// icon `saved` has no entry for.
    pub(crate) fn refresh_desktop_icons(&mut self) {
        let Some(icons) = &self.desktop_icons else { return };
        let rows = ((self.primary_monitor_height()) / CELL_HEIGHT).max(1);
        let saved = crate::desktop_icons_state::load();
        let origin = icons.origin;
        let icons = crate::desktop_icons::rescan(&saved, rows);
        self.desktop_icons = Some(DesktopIcons { origin, icons });
        self.desktop_icon_buffers.clear();
    }

    fn primary_monitor_height(&self) -> i32 {
        self.wm.borrow().monitors().iter().find(|m| m.primary).map(|m| m.geometry.height as i32 - 2 * GRID_MARGIN).unwrap_or(600)
    }

    /// Rasterises (or re-rasterises) one icon's buffer - called whenever
    /// that icon's selection state or drag position changes, never per
    /// frame; `render_udev_frame`/`render_frame` just read whatever's
    /// already cached here.
    fn rebuild_icon_buffer(&mut self, id: &str) {
        let Some(icons) = &self.desktop_icons else { return };
        let Some(icon) = icons.icons.iter().find(|i| i.id == id) else { return };
        let theme = self.wm.borrow().theme;
        let label_color = (240, 240, 240);
        // While this exact icon is mid-rename, show the live-edited buffer
        // (with a trailing caret) instead of its real, on-disk label - so
        // typing is visible without touching the filesystem until Enter
        // actually commits it (`desktop_icon_rename_key`).
        let label: std::borrow::Cow<str> = match &self.renaming_icon {
            Some((rid, buf)) if rid == id => format!("{buf}_").into(),
            _ => icon.label.as_str().into(),
        };
        let data = decoration::render_desktop_icon(
            CELL_WIDTH as u32,
            CELL_HEIGHT as u32,
            icon.kind,
            &label,
            icon.selected,
            theme.titlebar_fg_focused,
            label_color,
            theme.default_border_color,
        );
        let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (CELL_WIDTH, CELL_HEIGHT), 1, Transform::Normal, None);
        self.desktop_icon_buffers.insert(id.to_string(), buffer);
    }

    fn icon_buffer(&mut self, id: &str) -> Option<&MemoryRenderBuffer> {
        if !self.desktop_icon_buffers.contains_key(id) {
            self.rebuild_icon_buffer(id);
        }
        self.desktop_icon_buffers.get(id)
    }

    /// Every `(position, buffer)` pair the render loop needs to push this
    /// frame - lazily rebuilds any icon whose buffer isn't cached yet
    /// (a fresh icon, or one whose selection/drag state just changed),
    /// then returns everything already up to date. Global-space positions;
    /// the caller subtracts its own head origin, same as every other
    /// `custom_elements` push site.
    pub(crate) fn desktop_icon_render_list(&mut self) -> Vec<((i32, i32), MemoryRenderBuffer)> {
        let Some(icons) = &self.desktop_icons else { return Vec::new() };
        let ids: Vec<String> = icons.icons.iter().map(|i| i.id.clone()).collect();
        let origin = icons.origin;
        let dragging = self.desktop_icon_drag.clone();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let buffer = match self.icon_buffer(&id) {
                Some(b) => b.clone(),
                None => continue,
            };
            let icons = self.desktop_icons.as_ref().unwrap();
            let icon = icons.icons.iter().find(|i| i.id == id).unwrap();
            let pos = match &dragging {
                Some((drag_id, _, live_pos)) if *drag_id == id => *live_pos,
                _ => icon.top_left(origin),
            };
            out.push((pos, buffer));
        }
        out
    }

    /// Selects `id` (deselecting whatever was selected before, if
    /// anything) - both buffers rebuilt only if their selection state
    /// actually changed, not unconditionally.
    pub(crate) fn select_desktop_icon(&mut self, id: Option<&str>) {
        let Some(icons) = &mut self.desktop_icons else { return };
        let mut changed = Vec::new();
        for icon in &mut icons.icons {
            let should = Some(icon.id.as_str()) == id;
            if icon.selected != should {
                icon.selected = should;
                changed.push(icon.id.clone());
            }
        }
        for id in changed {
            self.rebuild_icon_buffer(&id);
        }
    }

    /// True when this press is the second of a double-click on the same
    /// icon - same 400ms threshold and reset-after-a-double shape as
    /// `is_double_click`, keyed by `DesktopIcon::id` since an icon has no
    /// `WindowId` of its own.
    pub(crate) fn is_double_click_icon(&mut self, id: &str, time: u32) -> bool {
        const DOUBLE_CLICK_MS: u32 = 400;
        let doubled = match &self.last_icon_click {
            Some((last_id, last_time)) => last_id == id && time.saturating_sub(*last_time) <= DOUBLE_CLICK_MS,
            None => false,
        };
        self.last_icon_click = if doubled { None } else { Some((id.to_string(), time)) };
        doubled
    }

    pub(crate) fn start_desktop_icon_drag(&mut self, id: &str, pointer: (i32, i32)) {
        let Some(icons) = &self.desktop_icons else { return };
        let Some(icon) = icons.icons.iter().find(|i| i.id == id) else { return };
        let top_left = icon.top_left(icons.origin);
        let grab_offset = (pointer.0 - top_left.0, pointer.1 - top_left.1);
        self.desktop_icon_drag = Some((id.to_string(), grab_offset, top_left));
    }

    /// Updates the live position of whichever icon is being dragged, if
    /// any - called from every pointer-motion event, same as
    /// `WindowManager::update_resize`'s own per-motion-event update.
    pub(crate) fn update_desktop_icon_drag(&mut self, pointer: (i32, i32)) {
        if let Some((_, grab_offset, live_pos)) = &mut self.desktop_icon_drag {
            *live_pos = (pointer.0 - grab_offset.0, pointer.1 - grab_offset.1);
        }
    }

    /// Ends an in-progress drag (if any): snaps to the nearest free grid
    /// cell (occupied cells other than the dragged icon's own previous one
    /// are avoided by walking outward from the raw target, closest first)
    /// and persists it.
    pub(crate) fn end_desktop_icon_drag(&mut self) {
        let Some((id, _, live_pos)) = self.desktop_icon_drag.take() else { return };
        let Some(icons) = &mut self.desktop_icons else { return };
        let origin = icons.origin;
        let raw = (live_pos.0 - origin.0, live_pos.1 - origin.1);
        let raw_cell = ((raw.0 as f64 / CELL_WIDTH as f64).round() as i32, (raw.1 as f64 / CELL_HEIGHT as f64).round() as i32).max_zero();
        let occupied: std::collections::HashSet<(i32, i32)> = icons.icons.iter().filter(|i| i.id != id).map(|i| i.cell).collect();
        let cell = nearest_free_cell(raw_cell, &occupied);
        if let Some(icon) = icons.icons.iter_mut().find(|i| i.id == id) {
            icon.cell = cell;
        }
        self.rebuild_icon_buffer(&id);
        crate::desktop_icons_state::save_icon(&id, cell);
    }

    pub(crate) fn open_desktop_icon(&mut self, id: &str) {
        let Some(icons) = &self.desktop_icons else { return };
        let Some(icon) = icons.icons.iter().find(|i| i.id == id) else { return };
        let target = icon.target.display().to_string();
        let file_manager = self.wm.borrow().file_manager.clone();
        if file_manager.is_empty() {
            spawn_shell(&format!("xdg-open {}", shell_quote(&target)));
        } else {
            spawn_shell(&format!("{file_manager} {}", shell_quote(&target)));
        }
    }

    /// Enters inline rename mode for `id`, pre-filled with its current
    /// label - only ever called for a real file/folder icon (`desktop_
    /// menu.rs`'s own `open_for_icon` never offers Rename for anything
    /// else), but harmless if that assumption is ever wrong: `commit_
    /// icon_rename` re-checks the icon's kind before touching the
    /// filesystem.
    pub(crate) fn start_rename_icon(&mut self, id: &str) {
        let Some(icons) = &self.desktop_icons else { return };
        let Some(icon) = icons.icons.iter().find(|i| i.id == id) else { return };
        self.renaming_icon = Some((id.to_string(), icon.label.clone()));
        self.rebuild_icon_buffer(id);
    }

    /// Routes one keystroke into the in-progress rename buffer - same
    /// shape as `native_lock_key`'s own `BackSpace`/`Return`/`Escape`/
    /// printable-character handling.
    pub(crate) fn desktop_icon_rename_key(&mut self, name: &str, utf8: &str) {
        let Some((id, mut buffer)) = self.renaming_icon.take() else { return };
        match name {
            "BackSpace" => {
                buffer.pop();
                self.renaming_icon = Some((id.clone(), buffer));
                self.rebuild_icon_buffer(&id);
            }
            "Return" | "KP_Enter" => self.commit_icon_rename(&id, &buffer),
            "Escape" => self.rebuild_icon_buffer(&id),
            _ => {
                if !utf8.is_empty() && utf8.chars().all(|c| !c.is_control()) {
                    buffer.push_str(utf8);
                }
                self.renaming_icon = Some((id.clone(), buffer));
                self.rebuild_icon_buffer(&id);
            }
        }
    }

    /// Renames the real file/folder on disk and carries its saved grid
    /// cell forward under the new name, so a rename doesn't also bump the
    /// icon to a fresh default position. A blank name, or renaming
    /// anything other than a real file/folder (shouldn't happen - see
    /// `start_rename_icon`'s own doc comment), just cancels with no
    /// filesystem change.
    fn commit_icon_rename(&mut self, id: &str, new_name: &str) {
        let new_name = new_name.trim();
        let Some(icons) = &self.desktop_icons else { return };
        let Some(icon) = icons.icons.iter().find(|i| i.id == id) else { return };
        if new_name.is_empty() || !matches!(icon.kind, IconKind::Folder | IconKind::File) {
            self.rebuild_icon_buffer(id);
            return;
        }
        let Some(parent) = icon.target.parent() else {
            self.rebuild_icon_buffer(id);
            return;
        };
        let new_path = parent.join(new_name);
        if let Err(e) = std::fs::rename(&icon.target, &new_path) {
            log::warn!("desktop_icons: couldn't rename {:?} to {new_name:?}: {e}", icon.target);
            self.rebuild_icon_buffer(id);
            return;
        }
        let cell = icon.cell;
        crate::desktop_icons_state::save_icon(new_name, cell);
        self.desktop_icon_buffers.remove(id);
        self.refresh_desktop_icons();
    }

    /// Moves the real file/folder into `~/.local/share/Trash` - see
    /// `trash.rs`'s own module doc comment for why this needs no
    /// confirmation.
    pub(crate) fn delete_desktop_icon(&mut self, id: &str) {
        let Some(icons) = &self.desktop_icons else { return };
        let Some(icon) = icons.icons.iter().find(|i| i.id == id) else { return };
        if let Err(e) = crate::trash::move_to_trash(&icon.target) {
            log::warn!("desktop_icons: couldn't move {:?} to trash: {e}", icon.target);
            return;
        }
        self.desktop_icon_buffers.remove(id);
        self.refresh_desktop_icons();
    }

    pub(crate) fn empty_trash(&mut self) {
        let Ok(home) = std::env::var("HOME").map(std::path::PathBuf::from) else { return };
        crate::trash::empty(&home);
        self.desktop_icon_buffers.remove("trash");
    }

    /// Spawns `general.terminal` (or the first of `TERMINAL_CANDIDATES`
    /// found on `$PATH`) with `~/Desktop` as its working directory.
    pub(crate) fn open_terminal_here(&mut self) {
        let Ok(home) = std::env::var("HOME") else { return };
        let desktop = format!("{home}/Desktop");
        let configured = self.wm.borrow().terminal.clone();
        let command = if !configured.is_empty() {
            Some(configured)
        } else {
            TERMINAL_CANDIDATES.iter().find(|bin| on_path(bin)).map(|s| s.to_string())
        };
        match command {
            Some(command) => spawn_shell_in_dir(&command, &desktop),
            None => log::warn!("desktop_icons: no terminal found on $PATH and general.terminal is unset"),
        }
    }

    /// Opens `~/Desktop` itself via `file_manager`/`xdg-open` - the
    /// concrete path to a real file manager's own richer menu.
    pub(crate) fn open_desktop_in_file_manager(&mut self) {
        let Ok(home) = std::env::var("HOME") else { return };
        let desktop = format!("{home}/Desktop");
        let file_manager = self.wm.borrow().file_manager.clone();
        if file_manager.is_empty() {
            spawn_shell(&format!("xdg-open {}", shell_quote(&desktop)));
        } else {
            spawn_shell(&format!("{file_manager} {}", shell_quote(&desktop)));
        }
    }

    /// Creates `~/Desktop/New Folder`, de-duplicated as `New Folder (2)`,
    /// `(3)`, ... against whatever's already there, then rescans so it
    /// shows up as an icon immediately.
    pub(crate) fn new_desktop_folder(&mut self) {
        let Ok(home) = std::env::var("HOME") else { return };
        let desktop = std::path::PathBuf::from(home).join("Desktop");
        let mut name = "New Folder".to_string();
        let mut n = 2;
        while desktop.join(&name).exists() {
            name = format!("New Folder ({n})");
            n += 1;
        }
        if let Err(e) = std::fs::create_dir(desktop.join(&name)) {
            log::warn!("desktop_icons: couldn't create {name:?}: {e}");
            return;
        }
        self.refresh_desktop_icons();
    }

    pub(crate) fn open_desktop_icon_menu(&mut self, icon_id: &str, pos: (i32, i32)) {
        let Some(icons) = &self.desktop_icons else { return };
        let Some(icon) = icons.icons.iter().find(|i| i.id == icon_id) else { return };
        let menu = DesktopMenu::open_for_icon(icon, pos);
        self.build_desktop_menu_buffer(menu);
    }

    pub(crate) fn open_desktop_menu(&mut self, pos: (i32, i32)) {
        let menu = DesktopMenu::open_for_desktop(pos);
        self.build_desktop_menu_buffer(menu);
    }

    fn build_desktop_menu_buffer(&mut self, menu: DesktopMenu) {
        let theme = self.wm.borrow().theme;
        let items: Vec<(&str, bool)> = menu.items.iter().map(|&(label, _)| (label, false)).collect();
        let data = decoration::render_context_menu(menu.width, menu.row_height, &items, theme.titlebar_bg, theme.titlebar_fg_focused, theme.titlebar_fg_unfocused, theme.default_border_color);
        let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (menu.width as i32, menu.height()), 1, Transform::Normal, None);
        self.desktop_menu_buffer = Some(buffer);
        self.desktop_menu = Some(menu);
    }

    pub(crate) fn close_desktop_menu(&mut self) {
        self.desktop_menu = None;
        self.desktop_menu_buffer = None;
    }

    pub(crate) fn run_desktop_menu_action(&mut self, action: DesktopMenuAction) {
        match action {
            DesktopMenuAction::OpenIcon(id) => self.open_desktop_icon(&id),
            DesktopMenuAction::Rename(id) => self.start_rename_icon(&id),
            DesktopMenuAction::Delete(id) => self.delete_desktop_icon(&id),
            DesktopMenuAction::EmptyTrash => self.empty_trash(),
            DesktopMenuAction::NewFolder => self.new_desktop_folder(),
            DesktopMenuAction::OpenTerminalHere => self.open_terminal_here(),
            DesktopMenuAction::OpenInFileManager => self.open_desktop_in_file_manager(),
            DesktopMenuAction::Refresh => self.refresh_desktop_icons(),
        }
    }
}

trait MaxZero {
    fn max_zero(self) -> Self;
}
impl MaxZero for (i32, i32) {
    fn max_zero(self) -> Self {
        (self.0.max(0), self.1.max(0))
    }
}

/// Breadth-first search outward from `target` over grid cells (`target`
/// itself first, then its 4-neighbours, then theirs, ...) for the first
/// one not in `occupied` - a dropped icon always lands *somewhere* rather
/// than silently failing to move when its raw target cell is already
/// taken.
fn nearest_free_cell(target: (i32, i32), occupied: &std::collections::HashSet<(i32, i32)>) -> (i32, i32) {
    if !occupied.contains(&target) && target.0 >= 0 && target.1 >= 0 {
        return target;
    }
    use std::collections::VecDeque;
    let mut seen = std::collections::HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(target);
    seen.insert(target);
    while let Some((c, r)) = queue.pop_front() {
        if c >= 0 && r >= 0 && !occupied.contains(&(c, r)) {
            return (c, r);
        }
        for (dc, dr) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = (c + dc, r + dr);
            if seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    target
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn spawn_shell(command: &str) {
    #[cfg(unix)]
    let result = std::process::Command::new("sh").arg("-c").arg(command).spawn();
    #[cfg(windows)]
    let result = std::process::Command::new("cmd").arg("/C").arg(command).spawn();
    if let Err(e) = result {
        log::warn!("desktop_icons: spawn '{command}' failed: {e}");
    }
}

/// Same fire-and-forget shell spawn as `spawn_shell`, but with a working
/// directory - `open_terminal_here`'s own reason to exist as a separate
/// function rather than reusing `spawn_shell` with a `cd` prefix, which
/// would need its own quoting for `dir`.
fn spawn_shell_in_dir(command: &str, dir: &str) {
    #[cfg(unix)]
    let result = std::process::Command::new("sh").arg("-c").arg(command).current_dir(dir).spawn();
    #[cfg(windows)]
    let result = std::process::Command::new("cmd").arg("/C").arg(command).current_dir(dir).spawn();
    if let Err(e) = result {
        log::warn!("desktop_icons: spawn '{command}' in {dir:?} failed: {e}");
    }
}

/// Whether `bin` resolves to a real, regular file somewhere on `$PATH` --
/// enough to pick a terminal candidate without a full `which`-equivalent
/// (no executable-bit/`PATHEXT` check on Windows; this whole feature is
/// Linux/BSD-desktop-shaped already, see `TERMINAL_CANDIDATES`' own
/// entries).
fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        assert_eq!(shell_quote("/home/x/pic.png"), "'/home/x/pic.png'");
    }

    #[test]
    fn shell_quote_escapes_an_embedded_single_quote() {
        assert_eq!(shell_quote("it's.png"), "'it'\\''s.png'");
    }

    #[test]
    fn nearest_free_cell_returns_the_target_when_its_free() {
        let occupied = std::collections::HashSet::new();
        assert_eq!(nearest_free_cell((2, 3), &occupied), (2, 3));
    }

    #[test]
    fn nearest_free_cell_finds_an_adjacent_slot_when_the_target_is_taken() {
        let mut occupied = std::collections::HashSet::new();
        occupied.insert((0, 0));
        let (c, r) = nearest_free_cell((0, 0), &occupied);
        assert!((c, r) != (0, 0));
        assert!(!occupied.contains(&(c, r)));
        assert_eq!(c.abs() + r.abs(), 1, "the nearest free cell is exactly one step away");
    }

    #[test]
    fn nearest_free_cell_never_returns_a_negative_column_or_row() {
        let mut occupied = std::collections::HashSet::new();
        occupied.insert((0, 0));
        occupied.insert((1, 0));
        occupied.insert((0, 1));
        let (c, r) = nearest_free_cell((0, 0), &occupied);
        assert!(c >= 0 && r >= 0);
    }
}
