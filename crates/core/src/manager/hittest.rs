//! Pointer hit-testing against a window's titlebar/border/content.
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
    // ---- Hit testing ------------------------------------------------------

    /// Topmost window whose frame contains `(x, y)`, along with what part of
    /// its titlebar/border was hit (button, drag area, resize edge).
    ///
    /// Restricted to the current workspace, same as `visible_windows`/
    /// `visible_windows_front_to_back` - a window on another workspace is
    /// never minimized (that's a separate flag from "not currently shown"),
    /// so without this a click landing on its old on-screen geometry hit
    /// *that* window instead of whatever the user could actually see.
    /// Reported live: clicking a window while a differently-workspaced one
    /// happened to occupy the same screen coordinates sent the click to the
    /// invisible one.
    pub fn hit_test(&self, x: i32, y: i32) -> Option<(WindowId, TitlebarHit)> {
        for w in self.order.iter().rev().filter_map(|id| self.windows.get(id)) {
            if w.minimized || w.workspace != self.current_workspace {
                continue;
            }
            if let Some(hit) = ResizeEdge::hit_test(w.geometry, x, y, w.decorated, w.border_width, self.resize_margin) {
                return Some((w.id, hit));
            }
        }
        None
    }

    /// Topmost non-minimised window on the current workspace containing a
    /// point, ignoring decorations. Used for modifier+drag, where the grab
    /// applies anywhere in the window rather than only on the titlebar
    /// (`hit_test`). See `hit_test`'s doc comment for why the workspace
    /// check is load-bearing, not redundant with `minimized`.
    pub fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
        self.order
            .iter()
            .rev()
            .filter_map(|id| self.windows.get(id))
            .find(|w| !w.minimized && w.workspace == self.current_workspace && w.geometry.contains_point(x, y))
            .map(|w| w.id)
    }

    /// The corner of `id` nearest a point, for modifier+right-drag resize:
    /// grabbing the closest corner is what makes the gesture feel like it
    /// pulls the edge you aimed at (matching Hyprland's `resizewindow`).
    pub fn nearest_corner(&self, id: WindowId, x: i32, y: i32) -> ResizeEdge {
        let Some(w) = self.windows.get(&id) else { return ResizeEdge::BottomRight };
        let (cx, cy) = w.geometry.center();
        match (x < cx, y < cy) {
            (true, true) => ResizeEdge::TopLeft,
            (false, true) => ResizeEdge::TopRight,
            (true, false) => ResizeEdge::BottomLeft,
            (false, false) => ResizeEdge::BottomRight,
        }
    }

}
