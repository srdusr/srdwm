/// An axis-aligned rectangle in screen space, used for window and monitor bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn overlaps(&self, other: &Rect) -> bool {
        !(self.right() <= other.x
            || other.right() <= self.x
            || self.bottom() <= other.y
            || other.bottom() <= self.y)
    }

    /// Shrinks the rect on all sides by `margin`, saturating at zero size.
    pub fn inset(&self, margin: u32) -> Rect {
        let m = margin as i32;
        let width = self.width.saturating_sub(margin * 2);
        let height = self.height.saturating_sub(margin * 2);
        Rect { x: self.x + m, y: self.y + m, width, height }
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width as i32 / 2, self.y + self.height as i32 / 2)
    }

    /// Moves this rect so it lies inside `bounds`, shrinking it only if it
    /// is genuinely larger than `bounds`.
    ///
    /// Used when a monitor is unplugged and its windows have to be rehomed:
    /// a window at coordinates that no longer exist would otherwise be
    /// off-screen and unreachable. Position is adjusted in preference to
    /// size so a window keeps the dimensions the user gave it.
    pub fn clamped_into(&self, bounds: Rect) -> Rect {
        let width = self.width.min(bounds.width);
        let height = self.height.min(bounds.height);
        // `max(bounds.x)` after `min` so that a bounds smaller than the rect
        // still yields the bounds' own origin rather than a negative offset.
        let x = (self.x).min(bounds.right() - width as i32).max(bounds.x);
        let y = (self.y).min(bounds.bottom() - height as i32).max(bounds.y);
        Rect { x, y, width, height }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_detection_matches_aabb_semantics() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        let c = Rect::new(100, 100, 50, 50); // touches corner, should not overlap (half-open)
        let d = Rect::new(200, 200, 10, 10);

        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c));
        assert!(!a.overlaps(&d));
    }

    #[test]
    fn inset_shrinks_symmetrically() {
        let r = Rect::new(0, 0, 100, 60);
        let inset = r.inset(10);
        assert_eq!(inset, Rect::new(10, 10, 80, 40));
    }

    #[test]
    fn contains_point_is_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains_point(0, 0));
        assert!(!r.contains_point(10, 10));
        assert!(r.contains_point(9, 9));
    }
}
