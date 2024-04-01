pub mod event;
pub mod geometry;
pub mod layout;
pub mod manager;
pub mod monitor;
pub mod placement;
pub mod window;
pub mod workspace;

pub use event::{Event, MouseButton, Modifiers};
pub use geometry::Rect;
pub use layout::{Layout, MasterStackLayout, NoOpLayout, TilingConfig};
pub use manager::{Direction, WindowManager};
pub use monitor::{Monitor, MonitorId};
pub use placement::{PlacementConfig, SmartPlacement};
pub use window::{ResizeEdge, TitlebarHit, Window, WindowId, RESIZE_MARGIN, TITLEBAR_HEIGHT};
pub use workspace::{Workspace, WorkspaceId};
