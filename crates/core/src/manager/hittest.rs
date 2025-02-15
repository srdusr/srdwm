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
        self.hit_test_with(x, y, |_, geometry| geometry)
    }

    /// Same as [`Self::hit_test`], but lets the caller substitute a
    /// different rect than `w.geometry` for whichever window is being
    /// tested - `geometry_for(id, w.geometry)` is called once per window in
    /// the same topmost-first order, and its return value is what actually
    /// gets tested instead of `w.geometry` directly.
    ///
    /// This exists for exactly one reason: a backend that animates window
    /// geometry (currently only the Wayland one, via `window_anims` in
    /// `CompState`) draws the border/titlebar at the *interpolated* rect
    /// every frame (`WindowAnim::current_rect`), but `w.geometry` here is
    /// always the animation's *target* - core has no concept of animation
    /// at all, deliberately (`Window.geometry` is meant to be the single
    /// source of truth every other subsystem reads). Calling plain
    /// `hit_test` during an active animation (toggling maximize/fullscreen,
    /// a Snap-Layouts zone, or a new window's open-slide - see
    /// `WindowManager::toggle_maximize`/`apply_snap_zone`/
    /// `toggle_fullscreen` for where `anim_from` gets set) meant the
    /// decoration/resize-margin hit-test used the window's *final* position
    /// while the border was still visibly animating toward it - reported
    /// live as "the border isn't always truly on the edge of the window",
    /// i.e. hovering what you can see as the edge doesn't match what's
    /// actually clickable there for as long as `animation_duration_ms`
    /// (200ms by default) hasn't elapsed since the last toggle/snap/open.
    /// Content clicks never had this problem - `space.map_element` already
    /// maps the client's surface at the same interpolated rect the border
    /// draws at (`state/geometry.rs::sync_geometry`), so `space.element_
    /// under` and the border were already agreeing with each other; only
    /// this compositor's own decoration hit-test was reading a different
    /// number than what it was drawing on screen.
    pub fn hit_test_with(&self, x: i32, y: i32, geometry_for: impl Fn(WindowId, Rect) -> Rect) -> Option<(WindowId, TitlebarHit)> {
        for w in self.order.iter().rev().filter_map(|id| self.windows.get(id)) {
            if w.minimized || w.workspace != self.current_workspace {
                continue;
            }
            let margin = w.resize_margin.unwrap_or(self.resize_margin);
            let geometry = geometry_for(w.id, w.geometry);
            if let Some(hit) = ResizeEdge::hit_test(geometry, x, y, w.decorated, w.border_width, margin, self.theme.buttons_left, self.theme.button_order, w.is_dialog) {
                return Some((w.id, hit));
            }
            // Not a titlebar/border/resize-margin hit on `w` - but if the
            // point still falls inside `w`'s own plain content rect, `w`'s
            // real, opaque content is what's actually drawn there (this is
            // topmost-first order, so nothing checked so far is above it),
            // and continuing the loop into a *lower* window's own border/
            // resize zone at this same point would return a hit for
            // something the user cannot see or reach - `w`'s content is
            // in the way regardless of whether `w` itself claimed the
            // point as one of its own edges. Reported live as being able
            // to grab a resize edge, or trigger a titlebar-adjacent action,
            // on a window fully covered by another one on top of it.
            // Content clicks (the content-hit branch in `input.rs`) are
            // unaffected - they already resolve via `Space::element_under`,
            // smithay's own real Z-order, which never had this gap.
            if geometry.contains_point(x, y) {
                return None;
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
