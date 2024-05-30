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
    pub name: String,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
}

impl Monitor {
    pub fn new(id: MonitorId, name: impl Into<String>, geometry: Rect) -> Self {
        Self { id, name: name.into(), geometry, full_geometry: geometry, refresh_rate_mhz: 60_000, primary: false }
    }
}
