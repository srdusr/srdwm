//! Focus queries and directional/cyclic focus movement.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
    // ---- Focus ----------------------------------------------------------

    pub fn focused_window(&self) -> Option<&Window> {
        self.focused.and_then(|id| self.windows.get(&id))
    }

    pub fn focused_id(&self) -> Option<WindowId> {
        self.focused
    }

    pub fn focus_window(&mut self, id: WindowId) {
        if self.windows.contains_key(&id) {
            self.focused = Some(id);
            self.raise_window(id);
        }
    }

    pub(super) fn cycle_focus(&mut self, forward: bool) {
        let ids: Vec<WindowId> = self.windows_on_workspace(self.current_workspace).filter(|w| !w.minimized).map(|w| w.id).collect();
        if ids.is_empty() {
            self.focused = None;
            return;
        }
        let cur_pos = self.focused.and_then(|f| ids.iter().position(|&i| i == f));
        let next = match cur_pos {
            None => 0,
            Some(p) if forward => (p + 1) % ids.len(),
            Some(p) => (p + ids.len() - 1) % ids.len(),
        };
        self.focus_window(ids[next]);
    }

    pub fn focus_next(&mut self) {
        self.cycle_focus(true);
    }

    pub fn focus_previous(&mut self) {
        self.cycle_focus(false);
    }

    /// Vim-style directional focus: picks the nearest window whose center
    /// lies in `dir` relative to the focused window's center, on the same
    /// workspace. Returns the newly focused window, if any.
    /// Nearest window to the focused one in `dir`, by a distance biased
    /// toward the requested axis so a window that's mostly to the left
    /// (small |dy|) beats a diagonally-placed one - matching how
    /// i3/sway-style directional focus feels.
    ///
    /// Shared by [`Self::focus_direction`] and [`Self::move_window_direction`]
    /// so "the window to the left" means the same thing whether you're
    /// focusing it or swapping with it.
    pub fn neighbour_in(&self, dir: Direction) -> Option<WindowId> {
        let (fx, fy, fid) = {
            let focused = self.focused_window()?;
            let (fx, fy) = focused.geometry.center();
            (fx, fy, focused.id)
        };
        let workspace = self.current_workspace;
        let mut best: Option<(WindowId, i64)> = None;
        for w in self.windows_on_workspace(workspace).filter(|w| w.id != fid && !w.minimized) {
            let (cx, cy) = w.geometry.center();
            let (dx, dy) = ((cx - fx) as i64, (cy - fy) as i64);
            let matches = match dir {
                Direction::Left => dx < 0,
                Direction::Right => dx > 0,
                Direction::Up => dy < 0,
                Direction::Down => dy > 0,
            };
            if !matches {
                continue;
            }
            let (primary, secondary) = match dir {
                Direction::Left | Direction::Right => (dx, dy),
                Direction::Up | Direction::Down => (dy, dx),
            };
            let dist = primary * primary + secondary * secondary * 4;
            if best.is_none_or(|(_, d)| dist < d) {
                best = Some((w.id, dist));
            }
        }
        best.map(|(id, _)| id)
    }

    pub fn focus_direction(&mut self, dir: Direction) -> Option<WindowId> {
        let target = self.neighbour_in(dir);
        if let Some(id) = target {
            self.focus_window(id);
        }
        target
    }

    /// Moves the focused window in `dir` by swapping places with its
    /// neighbour there - the `movewindow l/r/u/d` gesture.
    ///
    /// Swapping (rather than nudging by a fixed step) is what makes this
    /// useful in both of srdwm's modes: under tiling it reorders the layout,
    /// and in dynamic/floating mode two windows trade positions, which is
    /// predictable either way. With no neighbour in that direction the
    /// window is pushed to the corresponding edge of its monitor instead, so
    /// the key still does something sensible.
    pub fn move_window_direction(&mut self, dir: Direction) -> Option<WindowId> {
        let focused = self.focused_id()?;
        match self.neighbour_in(dir) {
            Some(other) => {
                let a = self.windows.get(&focused)?.geometry;
                let b = self.windows.get(&other)?.geometry;
                if let Some(w) = self.windows.get_mut(&focused) {
                    w.geometry = b;
                }
                if let Some(w) = self.windows.get_mut(&other) {
                    w.geometry = a;
                }
                // Keep stacking order in step so a tiling layout, which
                // assigns slots from `order`, actually reflects the swap.
                let (ia, ib) = (
                    self.order.iter().position(|&id| id == focused)?,
                    self.order.iter().position(|&id| id == other)?,
                );
                self.order.swap(ia, ib);
                Some(other)
            }
            None => {
                let mon = self.windows.get(&focused).and_then(|w| self.monitor_for(w.monitor))?.geometry;
                let w = self.windows.get_mut(&focused)?;
                match dir {
                    Direction::Left => w.geometry.x = mon.x,
                    Direction::Right => w.geometry.x = mon.right() - w.geometry.width as i32,
                    Direction::Up => w.geometry.y = mon.y,
                    Direction::Down => w.geometry.y = mon.bottom() - w.geometry.height as i32,
                }
                None
            }
        }
    }

}
