use crate::geometry::Rect;

pub type MonitorId = u32;

#[derive(Debug, Clone)]
pub struct Monitor {
    pub id: MonitorId,
    /// Usable area: the output rect shrunk by any layer-shell exclusive
    /// zone (a bar/dock). What placement, tiling and maximize target --
    /// see `full_geometry`'s doc comment for the one thing that
    /// deliberately does *not* use this field.
    pub geometry: Rect,
    /// The output's true full rect, ignoring any exclusive zone.
    ///
    /// Kept separate from `geometry` because "respects the dock" and
    /// "doesn't" are two genuinely different behaviors a window needs,
    /// not one setting: fullscreen (and a window being interactively
    /// dragged) should be able to cover or cross the strip a bar/dock
    /// reserves - the bar just renders on top, as an overlay, the same
    /// way it does everywhere else - while a *new* window's placement,
    /// tiling and maximize should keep avoiding that strip, same as
    /// before. Defaults to `geometry` (no reservation) for any backend
    /// that hasn't been taught the distinction yet.
    pub full_geometry: Rect,
    /// The rect `toggle_maximize` targets: `full_geometry` with only a
    /// top-anchored bar's exclusive zone (a menu bar, always expected to
    /// stay visible/reachable) subtracted back out again - a dock anchored
    /// to any other edge is deliberately left alone, same as
    /// `full_geometry`. Two behaviors maximize needs that neither
    /// `geometry` (shrunk by *every* zone) nor `full_geometry` (shrunk by
    /// none) can express on its own: "go past the dock" and "still stop at
    /// the top bar" are both true at once, on the user's own explicit
    /// call when the two pulled in opposite directions (`full_geometry` had
    /// briefly covered both, which un-did "stop at the top bar" as a side
    /// effect of fixing "go past the dock").
    ///
    /// Defaults to `geometry` (no reservation ignored at all) for any
    /// backend or test that hasn't been taught the per-edge distinction --
    /// same conservative-default reasoning as `full_geometry`'s own doc
    /// comment.
    pub maximize_geometry: Rect,
    pub name: String,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
}

impl Monitor {
    pub fn new(id: MonitorId, name: impl Into<String>, geometry: Rect) -> Self {
        Self { id, name: name.into(), geometry, full_geometry: geometry, maximize_geometry: geometry, refresh_rate_mhz: 60_000, primary: false }
    }
}
