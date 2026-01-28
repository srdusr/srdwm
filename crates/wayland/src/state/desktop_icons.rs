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

/// The hand-drawn glyphs' base colour - a dedicated, deliberately blue
/// tone, not read from `theme.titlebar_fg_focused` (that field is
/// whatever the user's own titlebar accent happens to be, which could be
/// any colour at all depending on their config; these glyphs want a
/// consistent, recognisable icon palette of their own, matching the
/// "slightly blue, polished look" requested directly, independent of
/// theme). `decoration::render_desktop_icon` derives a lighter top tone
/// and a darker outline/detail tone from this one value via `color::
/// brighten`/`darken`, the same directional-shading helpers the titlebar
/// buttons already use.
const ICON_COLOR: (u8, u8, u8) = (74, 144, 226);

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
        let origins = self.desktop_icon_origins();
        if origins.is_empty() {
            return;
        }
        // Rows (for the default-cell-assignment grid) are always derived
        // from the *primary* monitor's own height, even when mirroring onto
        // every monitor - one shared cell layout for the shared icon list,
        // not a different grid shape per monitor.
        let rows_source = self.wm.borrow().monitors().iter().find(|m| m.primary).map(|m| m.geometry.height as i32).unwrap_or(600);
        if let Some(icons) = &mut self.desktop_icons {
            icons.origins = origins;
            return;
        }
        let rows = ((rows_source - 2 * GRID_MARGIN) / CELL_HEIGHT).max(1);
        let saved = crate::desktop_icons_state::load();
        let icons = crate::desktop_icons::rescan(&saved, rows);
        self.desktop_icons = Some(DesktopIcons { origins, icons });
        self.desktop_icon_buffers.clear();
    }

    /// One grid origin per monitor icons should mirror onto - see
    /// `icon_origins_for`'s own doc comment for the actual selection
    /// logic. Sorted by monitor id first so the list (and therefore which
    /// origin `icon_at` matches first) is stable call to call, not at the
    /// mercy of `WindowManager::monitors()`'s own iteration order.
    fn desktop_icon_origins(&self) -> Vec<(i32, i32)> {
        let wm = self.wm.borrow();
        let mut monitors = wm.monitors().to_vec();
        monitors.sort_by_key(|m| m.id);
        icon_origins_for(&monitors, wm.desktop_icons_all_monitors)
    }

    /// Re-derives the icon list from the real filesystem (a new/removed
    /// `~/Desktop` entry) without disturbing any already-persisted cell --
    /// `rescan` itself already only assigns a fresh default cell to an
    /// icon `saved` has no entry for.
    pub(crate) fn refresh_desktop_icons(&mut self) {
        let Some(icons) = &self.desktop_icons else { return };
        let rows = ((self.primary_monitor_height()) / CELL_HEIGHT).max(1);
        let saved = crate::desktop_icons_state::load();
        let origins = icons.origins.clone();
        let icons = crate::desktop_icons::rescan(&saved, rows);
        self.desktop_icons = Some(DesktopIcons { origins, icons });
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
        // Real icon-theme artwork (WhiteSur, or whatever the user has
        // configured) when it resolves; `None` - no installed theme ships
        // this name, or the file it found didn't parse/render - falls
        // back to `render_desktop_icon`'s own hand-drawn glyph rather than
        // a blank box. Re-looked-up on every rebuild (icon selection
        // toggling, a rename) rather than cached separately: rebuilds are
        // already infrequent (see this function's own doc comment), and a
        // theme change while running should just work on the next one
        // without a separate cache-invalidation path to get wrong.
        let glyph_box = decoration::desktop_icon_glyph_box(CELL_WIDTH as u32, CELL_HEIGHT as u32);
        let (glyph_w, glyph_h) = ((glyph_box.2 - glyph_box.0).max(1) as u32, (glyph_box.3 - glyph_box.1).max(1) as u32);
        let real_icon = crate::icon_theme::find_icon(crate::icon_theme::icon_name(icon.kind))
            .and_then(|path| crate::icon_theme::rasterize_svg(&path, glyph_w, glyph_h));
        let data = decoration::render_desktop_icon(
            CELL_WIDTH as u32,
            CELL_HEIGHT as u32,
            icon.kind,
            &label,
            icon.selected,
            ICON_COLOR,
            label_color,
            theme.default_border_color,
            real_icon.as_deref(),
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
        let origins = icons.origins.clone();
        // `(primary_pos, members)` - see `DesktopIconDrag`'s own doc
        // comment. Every dragged icon (not just the one actually grabbed)
        // follows the live pointer as a rigid group; every other mirror
        // (if any) and every non-dragged icon stays put at its own
        // origin's cell position, same as before this could carry more
        // than one icon.
        let dragging = self.desktop_icon_drag.as_ref().map(|d| (d.primary_pos, d.members.clone()));
        let mut out = Vec::with_capacity(ids.len() * origins.len());
        for id in ids {
            let buffer = match self.icon_buffer(&id) {
                Some(b) => b.clone(),
                None => continue,
            };
            let icons = self.desktop_icons.as_ref().unwrap();
            let icon = icons.icons.iter().find(|i| i.id == id).unwrap();
            let drag_pos = dragging
                .as_ref()
                .and_then(|(primary_pos, members)| members.iter().find(|(mid, _)| *mid == id).map(|(_, offset)| (primary_pos.0 + offset.0, primary_pos.1 + offset.1)));
            match drag_pos {
                Some(pos) => out.push((pos, buffer)),
                None => {
                    for &origin in &origins {
                        out.push((icon.top_left(origin), buffer.clone()));
                    }
                }
            }
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

    /// Selects every desktop icon at once - the bare-desktop menu's own
    /// "Select All" action (see `DesktopMenuAction::SelectAll`'s own doc
    /// comment). Same "only rebuild the buffers that actually changed"
    /// shape as `select_desktop_icon`.
    pub(crate) fn select_all_desktop_icons(&mut self) {
        let Some(icons) = &mut self.desktop_icons else { return };
        let mut changed = Vec::new();
        for icon in &mut icons.icons {
            if !icon.selected {
                icon.selected = true;
                changed.push(icon.id.clone());
            }
        }
        for id in changed {
            self.rebuild_icon_buffer(&id);
        }
    }

    /// Starts a rubber-band selection at `pos` (global space) - clears
    /// whatever was selected before, matching real desktop convention
    /// (Windows/GNOME/macOS all start a fresh marquee selection, not an
    /// additive one, unless a modifier like Ctrl/Shift is held - not
    /// implemented here, same as this menu's own already-documented "no
    /// multi-select via click" gap, just now closed for the drag case).
    pub(crate) fn start_desktop_marquee(&mut self, pos: (i32, i32)) {
        self.select_desktop_icon(None);
        self.desktop_marquee = Some((pos, pos));
    }

    /// Updates the live end corner of an in-progress marquee and re-
    /// selects whatever icon cells the resulting rectangle now overlaps --
    /// called from every pointer-motion event while a marquee is active,
    /// same shape as `update_desktop_icon_drag`.
    pub(crate) fn update_desktop_marquee(&mut self, pos: (i32, i32)) {
        let Some((start, _)) = self.desktop_marquee else { return };
        self.desktop_marquee = Some((start, pos));
        let (x0, y0) = (start.0.min(pos.0), start.1.min(pos.1));
        let (x1, y1) = (start.0.max(pos.0), start.1.max(pos.1));
        // Only the mirror on whichever monitor the marquee itself *started*
        // on - a drag on one monitor selecting another monitor's mirrored
        // copies (or double-selecting both) would be actively confusing,
        // not a feature. Falls back to every origin if no monitor claims
        // the start point at all (shouldn't happen in practice; `Compositor
        // ::pointer_monitor`-style clamping already keeps the pointer
        // inside some monitor's bounds).
        let origins: Vec<(i32, i32)> = {
            let wm = self.wm.borrow();
            match wm.monitors().iter().find(|m| m.full_geometry.contains_point(start.0, start.1)) {
                Some(m) => vec![(m.geometry.x + GRID_MARGIN, m.geometry.y + GRID_MARGIN)],
                None => self.desktop_icons.as_ref().map(|i| i.origins.clone()).unwrap_or_default(),
            }
        };
        let Some(icons) = &mut self.desktop_icons else { return };
        let mut changed = Vec::new();
        for icon in &mut icons.icons {
            let overlaps = origins.iter().any(|&origin| {
                let (left, top) = icon.top_left(origin);
                let (right, bottom) = (left + CELL_WIDTH, top + CELL_HEIGHT);
                left < x1 && right > x0 && top < y1 && bottom > y0
            });
            if icon.selected != overlaps {
                icon.selected = overlaps;
                changed.push(icon.id.clone());
            }
        }
        for id in changed {
            self.rebuild_icon_buffer(&id);
        }
    }

    /// Ends an in-progress marquee, if any - the final `update_desktop_
    /// marquee` call already left the right icons selected, so this only
    /// needs to clear the drag state itself (and the rendered outline with
    /// it).
    pub(crate) fn end_desktop_marquee(&mut self) {
        self.desktop_marquee = None;
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

    /// `origin` is whichever mirror was actually clicked (`icon_at`'s own
    /// return value, threaded through by the caller) - with the icon
    /// mirrored onto several monitors, grabbing it relative to the copy
    /// under the pointer, not always the primary monitor's, is what makes
    /// the drag track the cursor instead of jumping to a different
    /// monitor's copy the instant the drag starts.
    /// Starts a drag on `id`. If `id` is already part of a multi-
    /// selection, every other selected icon comes along as a `member`
    /// (see `DesktopIconDrag`'s own doc comment); otherwise this starts a
    /// fresh single-icon drag and selects just `id`, the same "grabbing
    /// something outside the current selection replaces it" convention
    /// real desktops use.
    pub(crate) fn start_desktop_icon_drag(&mut self, id: &str, origin: (i32, i32), pointer: (i32, i32)) {
        let Some(icons) = &mut self.desktop_icons else { return };
        let Some(primary) = icons.icons.iter().find(|i| i.id == id) else { return };
        let primary_top_left = primary.top_left(origin);
        let primary_selected = primary.selected;
        let grab_offset = (pointer.0 - primary_top_left.0, pointer.1 - primary_top_left.1);

        let mut changed = Vec::new();
        if !primary_selected {
            for icon in &mut icons.icons {
                let should = icon.id == id;
                if icon.selected != should {
                    icon.selected = should;
                    changed.push(icon.id.clone());
                }
            }
        }
        let members: Vec<(String, (i32, i32))> = icons
            .icons
            .iter()
            .filter(|i| i.id == id || i.selected)
            .map(|i| {
                let top_left = i.top_left(origin);
                (i.id.clone(), (top_left.0 - primary_top_left.0, top_left.1 - primary_top_left.1))
            })
            .collect();
        self.desktop_icon_drag = Some(crate::desktop_icons::DesktopIconDrag { grab_offset, primary_pos: primary_top_left, members });
        for id in changed {
            self.rebuild_icon_buffer(&id);
        }
    }

    /// Updates the live position of every icon in the current drag (the
    /// one actually grabbed, plus every icon carried along with it), if
    /// any - called from every pointer-motion event, same as
    /// `WindowManager::update_resize`'s own per-motion-event update.
    pub(crate) fn update_desktop_icon_drag(&mut self, pointer: (i32, i32)) {
        if let Some(drag) = &mut self.desktop_icon_drag {
            drag.primary_pos = (pointer.0 - drag.grab_offset.0, pointer.1 - drag.grab_offset.1);
        }
    }

    /// Ends an in-progress drag (if any): snaps every dragged icon (the
    /// one actually grabbed, plus every icon carried along with it) to
    /// its own nearest free grid cell, independently but never colliding
    /// with each other - walking outward from each one's own raw target,
    /// closest first - and persists all of them.
    pub(crate) fn end_desktop_icon_drag(&mut self) {
        let Some(drag) = self.desktop_icon_drag.take() else { return };
        // The cell math below needs the origin of whichever monitor the
        // group was actually dropped on, not always the first mirror --
        // recomputed fresh (same formula `desktop_icon_origins` uses)
        // rather than searched for in `icons.origins`, since a drop
        // just past every monitor's own strict icon-grid rect (but still
        // on-screen) should still resolve to that monitor's grid, not fall
        // through to a stale/wrong one. Based on the primary's own drop
        // position - the group drops together onto whichever monitor the
        // pointer is actually over.
        let origin = self
            .wm
            .borrow()
            .monitors()
            .iter()
            .find(|m| m.full_geometry.contains_point(drag.primary_pos.0, drag.primary_pos.1))
            .map(|m| (m.geometry.x + GRID_MARGIN, m.geometry.y + GRID_MARGIN))
            .unwrap_or(self.desktop_icons.as_ref().map(|i| i.origins.first().copied().unwrap_or((0, 0))).unwrap_or((0, 0)));
        let Some(icons) = &mut self.desktop_icons else { return };
        let dragged_ids: std::collections::HashSet<&str> = drag.members.iter().map(|(id, _)| id.as_str()).collect();
        // Each member's own nearest free cell, in recorded order (primary
        // first) - `newly_occupied` accumulates across the loop so two
        // dragged icons landing near each other never both claim the same
        // cell, on top of every cell a *non*-dragged icon already holds.
        let mut newly_occupied: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
        let mut placements: Vec<(String, (i32, i32))> = Vec::with_capacity(drag.members.len());
        for (id, offset) in &drag.members {
            let live_pos = (drag.primary_pos.0 + offset.0, drag.primary_pos.1 + offset.1);
            let raw = (live_pos.0 - origin.0, live_pos.1 - origin.1);
            let raw_cell = ((raw.0 as f64 / CELL_WIDTH as f64).round() as i32, (raw.1 as f64 / CELL_HEIGHT as f64).round() as i32).max_zero();
            let occupied: std::collections::HashSet<(i32, i32)> =
                icons.icons.iter().filter(|i| !dragged_ids.contains(i.id.as_str())).map(|i| i.cell).chain(newly_occupied.iter().copied()).collect();
            let cell = nearest_free_cell(raw_cell, &occupied);
            newly_occupied.insert(cell);
            placements.push((id.clone(), cell));
        }
        for (id, cell) in &placements {
            if let Some(icon) = icons.icons.iter_mut().find(|i| &i.id == id) {
                icon.cell = *cell;
            }
        }
        for (id, cell) in &placements {
            self.rebuild_icon_buffer(id);
            crate::desktop_icons_state::save_icon(id, *cell);
        }
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

    /// "New > Text Document" (Windows) / a blank file (macOS's own desktop
    /// menu has no direct equivalent, but every mainstream file manager
    /// does) - the concrete gap behind "not even new file" reported live
    /// against this menu next to Windows'/macOS' own. Same collision-
    /// avoidance and refresh as `new_desktop_folder` just above.
    pub(crate) fn new_desktop_text_file(&mut self) {
        let Ok(home) = std::env::var("HOME") else { return };
        let desktop = std::path::PathBuf::from(home).join("Desktop");
        let mut name = "New Text Document.txt".to_string();
        let mut n = 2;
        while desktop.join(&name).exists() {
            name = format!("New Text Document ({n}).txt");
            n += 1;
        }
        if let Err(e) = std::fs::write(desktop.join(&name), "") {
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
        // Not redesigned the way the titlebar menu was (`srdwm_core::
        // context_menu`'s own module doc comment) - this menu's rows are
        // still all one uniform height, `Separator` included, matching
        // its behaviour before `render_context_menu` grew a per-row
        // height/kind. Every row here is real content or `DesktopMenuAction
        // ::Separator`, never a non-interactive caption, so there is no
        // `MenuRowKind::Header` case to map to.
        let rows: Vec<(&str, bool, u32, decoration::MenuRowKind)> = menu
            .items
            .iter()
            .map(|(label, action)| {
                let kind = if matches!(action, crate::desktop_menu::DesktopMenuAction::Separator) { decoration::MenuRowKind::Separator } else { decoration::MenuRowKind::Item };
                (*label, false, menu.row_height, kind)
            })
            .collect();
        let data = decoration::render_context_menu(menu.width, &rows, theme.titlebar_bg, theme.titlebar_fg_focused, theme.titlebar_fg_unfocused, theme.default_border_color);
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
            DesktopMenuAction::NewTextFile => self.new_desktop_text_file(),
            DesktopMenuAction::OpenTerminalHere => self.open_terminal_here(),
            DesktopMenuAction::OpenInFileManager => self.open_desktop_in_file_manager(),
            DesktopMenuAction::SelectAll => self.select_all_desktop_icons(),
            DesktopMenuAction::Refresh => self.refresh_desktop_icons(),
            // Never actually reached - the click-dispatch site intercepts
            // `Separator` first, same as `context_menu::MenuAction::
            // Separator`'s own dispatch. Handled here too so this match
            // stays exhaustive.
            DesktopMenuAction::Separator => {}
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

/// The actual logic behind [`CompState::desktop_icon_origins`] - pulled
/// out so it's testable without a real `WindowManager`/`Output`, the same
/// reasoning `udev/outputs.rs::next_logical_x` already applies. `monitors`
/// must already be sorted by id.
///
/// One origin per real, physically distinct screen when `all_monitors` is
/// set - not one per `Monitor` entry, which `srd.monitor.split` can
/// multiply several of out of the *same* real output purely for
/// placement/tiling purposes (see `Monitor::split`'s own doc comment: "not
/// a second wl_output, not a second physical connector"). Mirroring the
/// full icon set onto every split part put two, visually side by side, on
/// what is still one continuous physical desktop - reported live as
/// "doesn't look like 2 more monitors, just showing double desktop icons"
/// the moment a two-way split was tried. A split part's own name is
/// always `"{connector}-{part}"` (`platform.rs`'s `sub_name`
/// construction) - recovering the real connector from it and keeping
/// only the first (lowest-id) part per connector collapses every split
/// group back to the one real screen it actually is, while a genuinely
/// separate additional monitor (real or fake, `split == false`) still
/// gets its own origin exactly as before. When `all_monitors` is unset,
/// just the primary monitor's origin, matching the original single-
/// monitor behaviour.
fn icon_origins_for(monitors: &[srdwm_core::Monitor], all_monitors: bool) -> Vec<(i32, i32)> {
    if all_monitors {
        let mut seen_split_connectors = std::collections::HashSet::new();
        monitors
            .iter()
            .filter(|m| {
                if !m.split {
                    return true;
                }
                let connector = m.name.rsplit_once('-').map(|(base, _)| base).unwrap_or(&m.name);
                seen_split_connectors.insert(connector.to_string())
            })
            .map(|m| (m.geometry.x + GRID_MARGIN, m.geometry.y + GRID_MARGIN))
            .collect()
    } else {
        monitors.iter().find(|m| m.primary).map(|m| vec![(m.geometry.x + GRID_MARGIN, m.geometry.y + GRID_MARGIN)]).unwrap_or_default()
    }
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

    fn split_part(id: srdwm_core::MonitorId, name: &str, x: i32) -> srdwm_core::Monitor {
        let mut m = srdwm_core::Monitor::new(id, name, srdwm_core::Rect::new(x, 0, 960, 1080));
        m.split = true;
        m
    }

    #[test]
    fn a_two_way_split_screen_gets_only_one_icon_origin_not_two() {
        // The live-reported bug: "doesn't look like 2 more monitors, just
        // showing double desktop icons" - a split screen is still one
        // physical desktop, so mirroring the icon column onto both halves
        // read as a visual duplication bug, not a second monitor.
        let monitors = vec![split_part(0, "eDP-1-1", 0), split_part(1, "eDP-1-2", 960)];
        let origins = icon_origins_for(&monitors, true);
        assert_eq!(origins.len(), 1, "a split screen must contribute exactly one icon origin, not one per split part");
        assert_eq!(origins[0], (GRID_MARGIN, GRID_MARGIN), "the one origin kept must be the first (lowest-id) split part's");
    }

    #[test]
    fn a_split_screen_plus_a_genuinely_separate_monitor_gets_two_origins() {
        let mut real_second = srdwm_core::Monitor::new(2, "HDMI-A-1", srdwm_core::Rect::new(1920, 0, 1920, 1080));
        real_second.split = false;
        let monitors = vec![split_part(0, "eDP-1-1", 0), split_part(1, "eDP-1-2", 960), real_second];
        let origins = icon_origins_for(&monitors, true);
        assert_eq!(origins.len(), 2, "a genuinely separate monitor must still get its own origin alongside the split screen's one");
    }

    #[test]
    fn two_different_split_connectors_each_get_their_own_origin() {
        // Guards the grouping logic against merging two *different* real
        // outputs that both happen to be split, not just deduplicating one
        // connector's own parts.
        let monitors = vec![split_part(0, "eDP-1-1", 0), split_part(1, "eDP-1-2", 960), split_part(2, "HDMI-A-1-1", 1920), split_part(3, "HDMI-A-1-2", 2880)];
        let origins = icon_origins_for(&monitors, true);
        assert_eq!(origins.len(), 2, "each split connector is its own physical screen and must get its own origin");
    }

    #[test]
    fn without_all_monitors_only_the_primary_gets_an_origin_even_when_split() {
        let mut a = split_part(0, "eDP-1-1", 0);
        a.primary = true;
        let b = split_part(1, "eDP-1-2", 960);
        let origins = icon_origins_for(&[a, b], false);
        assert_eq!(origins, vec![(GRID_MARGIN, GRID_MARGIN)]);
    }
}
