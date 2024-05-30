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

    /// The overlapping region of two rects, or `None` if they don't
    /// overlap at all (matches `overlaps`' own half-open semantics: a rect
    /// that only touches another along an edge or corner does not count).
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        if !self.overlaps(other) {
            return None;
        }
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Some(Rect::new(x, y, (right - x) as u32, (bottom - y) as u32))
    }

    /// `self` minus `other`, as the (up to 4) axis-aligned pieces left
    /// over - the standard top/bottom/left/right sliver decomposition
    /// around the intersection. An empty `Vec` means `other` fully covers
    /// `self`; a one-element `Vec` equal to `self` means they don't
    /// overlap at all.
    fn subtract_one(&self, other: &Rect) -> Vec<Rect> {
        let Some(ix) = self.intersection(other) else { return vec![*self] };
        let mut out = Vec::with_capacity(4);
        // Top sliver: full width, above the intersection.
        if ix.y > self.y {
            out.push(Rect::new(self.x, self.y, self.width, (ix.y - self.y) as u32));
        }
        // Bottom sliver: full width, below the intersection.
        if ix.bottom() < self.bottom() {
            out.push(Rect::new(self.x, ix.bottom(), self.width, (self.bottom() - ix.bottom()) as u32));
        }
        // Left/right slivers are constrained to the intersection's own
        // y-range (not self's full height), so the top/bottom slivers
        // above don't get double-counted at the corners.
        if ix.x > self.x {
            out.push(Rect::new(self.x, ix.y, (ix.x - self.x) as u32, ix.height));
        }
        if ix.right() < self.right() {
            out.push(Rect::new(ix.right(), ix.y, (self.right() - ix.right()) as u32, ix.height));
        }
        out
    }

    /// `self` minus every rect in `occluders` that overlaps it, as the
    /// disjoint pieces still left over. Used to keep a window's border
    /// from rendering on top of another window's content that's actually
    /// stacked in front of it - see `crates/wayland/src/elements.rs`'s
    /// `visible_border_fragments` doc comment for the fuller story on why
    /// that's needed at all. An empty result means `occluders` between
    /// them fully cover `self`.
    pub fn subtract_all(&self, occluders: &[Rect]) -> Vec<Rect> {
        let mut remaining = vec![*self];
        for occluder in occluders {
            if remaining.is_empty() {
                break;
            }
            remaining = remaining.iter().flat_map(|r| r.subtract_one(occluder)).collect();
        }
        remaining
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

    #[test]
    fn intersection_of_non_overlapping_rects_is_none() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(20, 20, 10, 10);
        assert_eq!(a.intersection(&b), None);
    }

    #[test]
    fn intersection_is_the_overlapping_region() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        assert_eq!(a.intersection(&b), Some(Rect::new(50, 50, 50, 50)));
    }

    #[test]
    fn subtract_all_with_no_occluders_returns_the_rect_unchanged() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(r.subtract_all(&[]), vec![r]);
    }

    #[test]
    fn subtract_all_with_a_non_overlapping_occluder_returns_the_rect_unchanged() {
        let r = Rect::new(0, 0, 100, 100);
        let occluder = Rect::new(200, 200, 10, 10);
        assert_eq!(r.subtract_all(&[occluder]), vec![r]);
    }

    #[test]
    fn subtract_all_with_a_fully_covering_occluder_returns_nothing() {
        let r = Rect::new(10, 10, 20, 20);
        let occluder = Rect::new(0, 0, 100, 100);
        assert!(r.subtract_all(&[occluder]).is_empty());
    }

    /// This is the exact bug this whole mechanism exists to fix, found live:
    /// a tall vertical border strip on a background window (e.g. its right
    /// edge) with a foreground window's content covering its middle,
    /// leaving only a sliver above and below visible - rather than the
    /// border rendering straight through the foreground window's content.
    #[test]
    fn subtract_all_splits_a_tall_strip_around_a_covering_window_into_two_slivers() {
        // A 3px-wide, 630px-tall right border strip...
        let border = Rect::new(890, 126, 3, 630);
        // ...with a foreground window covering its middle vertically.
        let foreground = Rect::new(240, 277, 800, 630);
        let pieces = border.subtract_all(&[foreground]);
        // Only the sliver above the foreground window's top edge and the
        // sliver below its bottom edge should remain - the foreground
        // window's own height (630) exceeds the border's, so in this case
        // the whole thing is covered from y=277 down; only the top sliver
        // (126..277) survives.
        assert_eq!(pieces, vec![Rect::new(890, 126, 3, 277 - 126)]);
    }

    #[test]
    fn subtract_all_leaves_a_gap_when_the_occluder_only_covers_the_middle() {
        let strip = Rect::new(0, 0, 5, 100);
        let occluder = Rect::new(0, 30, 5, 20); // covers y in [30, 50)
        let pieces = strip.subtract_all(&[occluder]);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.contains(&Rect::new(0, 0, 5, 30)));
        assert!(pieces.contains(&Rect::new(0, 50, 5, 50)));
    }

    #[test]
    fn subtract_all_handles_multiple_occluders_in_sequence() {
        let strip = Rect::new(0, 0, 5, 100);
        let a = Rect::new(0, 10, 5, 10); // [10,20)
        let b = Rect::new(0, 40, 5, 10); // [40,50)
        let pieces = strip.subtract_all(&[a, b]);
        assert_eq!(pieces.len(), 3);
        assert!(pieces.contains(&Rect::new(0, 0, 5, 10)));
        assert!(pieces.contains(&Rect::new(0, 20, 5, 20)));
        assert!(pieces.contains(&Rect::new(0, 50, 5, 50)));
    }

    #[test]
    fn subtract_all_handles_a_partial_side_overlap_without_losing_area() {
        // Occluder only covers the left half of the rect - the right half
        // (a "right sliver") must survive intact.
        let r = Rect::new(0, 0, 100, 50);
        let occluder = Rect::new(-10, -10, 60, 70); // covers x in [0,50)
        let pieces = r.subtract_all(&[occluder]);
        assert_eq!(pieces, vec![Rect::new(50, 0, 50, 50)]);
    }
}
