use crate::geometry::Rect;

pub type MonitorId = u32;

#[derive(Debug, Clone)]
pub struct Monitor {
    pub id: MonitorId,
    pub name: String,
    pub geometry: Rect,
    pub refresh_rate_mhz: u32,
    pub primary: bool,
}

impl Monitor {
    pub fn new(id: MonitorId, name: impl Into<String>, geometry: Rect) -> Self {
        Self { id, name: name.into(), geometry, refresh_rate_mhz: 60_000, primary: false }
    }
}
