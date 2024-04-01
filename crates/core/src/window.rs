use crate::geometry::Rect;

pub type WindowId = u64;

/// State of a single managed window. This is platform-independent: backends
/// (X11, Wayland, ...) own the real surface/client handle and keep a `Window`
/// in sync with it via `srdwm_core::WindowManager`.
#[derive(Debug, Clone)]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub app_id: String,
    pub geometry: Rect,
    /// Geometry to restore to when un-maximizing.
    pub restore_geometry: Option<Rect>,
    pub decorated: bool,
    pub floating: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub always_on_top: bool,
    pub border_color: (u8, u8, u8),
    pub border_width: u32,
    pub workspace: usize,
    pub monitor: u32,
}

impl Window {
    pub fn new(id: WindowId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            app_id: String::new(),
            geometry: Rect::new(0, 0, 640, 480),
            restore_geometry: None,
            decorated: true,
            floating: false,
            minimized: false,
            maximized: false,
            fullscreen: false,
            always_on_top: false,
            border_color: (136, 192, 208), // Nord accent, matches legacy theme default
            border_width: 2,
            workspace: 0,
            monitor: 0,
        }
    }
}

/// The height, in pixels, of the drawn title bar. Shared between backends so
/// hit-testing and rendering agree on the same band.
pub const TITLEBAR_HEIGHT: u32 = 30;
/// Width of a resize grab margin along each window edge.
pub const RESIZE_MARGIN: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeEdge {
    /// Determine which titlebar button (if any) a point within the titlebar
    /// band falls on. Buttons are laid out right-aligned: close, maximize, minimize.
    pub fn hit_test(frame: Rect, x: i32, y: i32) -> Option<TitlebarHit> {
        if !frame.contains_point(x, y) {
            return None;
        }
        if y < frame.y + TITLEBAR_HEIGHT as i32 {
            const BUTTON: i32 = TITLEBAR_HEIGHT as i32;
            let right = frame.right();
            if x >= right - BUTTON {
                return Some(TitlebarHit::Close);
            }
            if x >= right - BUTTON * 2 {
                return Some(TitlebarHit::Maximize);
            }
            if x >= right - BUTTON * 3 {
                return Some(TitlebarHit::Minimize);
            }
            return Some(TitlebarHit::Drag);
        }
        let edge = Self::resize_edge_at(frame, x, y)?;
        Some(TitlebarHit::Resize(edge))
    }

    fn resize_edge_at(frame: Rect, x: i32, y: i32) -> Option<ResizeEdge> {
        let m = RESIZE_MARGIN;
        let near_left = x <= frame.x + m;
        let near_right = x >= frame.right() - m;
        let near_top = y <= frame.y + m;
        let near_bottom = y >= frame.bottom() - m;
        Some(match (near_left, near_right, near_top, near_bottom) {
            (true, _, true, _) => ResizeEdge::TopLeft,
            (_, true, true, _) => ResizeEdge::TopRight,
            (true, _, _, true) => ResizeEdge::BottomLeft,
            (_, true, _, true) => ResizeEdge::BottomRight,
            (true, false, false, false) => ResizeEdge::Left,
            (false, true, false, false) => ResizeEdge::Right,
            (false, false, false, true) => ResizeEdge::Bottom,
            _ => return None,
        })
    }

    /// Apply a pointer delta to `original` geometry along this edge, honoring
    /// the given minimum size.
    pub fn apply_delta(self, original: Rect, dx: i32, dy: i32, min_w: u32, min_h: u32) -> Rect {
        let mut r = original;
        let min_w = min_w as i32;
        let min_h = min_h as i32;
        match self {
            ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
                let new_w = (original.width as i32 - dx).max(min_w);
                r.x = original.right() - new_w;
                r.width = new_w as u32;
            }
            _ => {}
        }
        match self {
            ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => {
                r.width = (original.width as i32 + dx).max(min_w) as u32;
            }
            _ => {}
        }
        match self {
            ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
                let new_h = (original.height as i32 - dy).max(min_h);
                r.y = original.bottom() - new_h;
                r.height = new_h as u32;
            }
            _ => {}
        }
        match self {
            ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight => {
                r.height = (original.height as i32 + dy).max(min_h) as u32;
            }
            _ => {}
        }
        r
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarHit {
    Drag,
    Close,
    Maximize,
    Minimize,
    Resize(ResizeEdge),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Rect {
        Rect::new(100, 100, 400, 300)
    }

    #[test]
    fn close_button_is_top_right_corner_of_titlebar() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 5, f.y + 5);
        assert_eq!(hit, Some(TitlebarHit::Close));
    }

    #[test]
    fn maximize_is_left_of_close() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - TITLEBAR_HEIGHT as i32 - 5, f.y + 5);
        assert_eq!(hit, Some(TitlebarHit::Maximize));
    }

    #[test]
    fn middle_of_titlebar_is_drag() {
        let f = frame();
        let (cx, _) = f.center();
        let hit = ResizeEdge::hit_test(f, cx, f.y + 5);
        assert_eq!(hit, Some(TitlebarHit::Drag));
    }

    #[test]
    fn bottom_right_corner_is_resize() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 1, f.bottom() - 1);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::BottomRight)));
    }

    #[test]
    fn outside_frame_is_none() {
        let f = frame();
        assert_eq!(ResizeEdge::hit_test(f, 0, 0), None);
    }

    #[test]
    fn resize_right_edge_grows_width_only() {
        let r = Rect::new(0, 0, 200, 100);
        let out = ResizeEdge::Right.apply_delta(r, 50, 999, 50, 50);
        assert_eq!(out, Rect::new(0, 0, 250, 100));
    }

    #[test]
    fn resize_left_edge_moves_x_and_shrinks_width() {
        let r = Rect::new(100, 0, 200, 100);
        let out = ResizeEdge::Left.apply_delta(r, 30, 0, 50, 50);
        assert_eq!(out, Rect::new(130, 0, 170, 100));
    }

    #[test]
    fn resize_respects_minimum_size() {
        let r = Rect::new(0, 0, 100, 100);
        let out = ResizeEdge::Right.apply_delta(r, -500, 0, 50, 50);
        assert_eq!(out.width, 50);
    }
}
