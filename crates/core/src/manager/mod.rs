use crate::geometry::Rect;
use crate::layout::{Layout, MasterStackLayout, NoOpLayout, TilingConfig};
use crate::monitor::{Monitor, MonitorId};
use crate::placement::{PlacementConfig, SmartPlacement, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};
use crate::rules::WindowRule;
use crate::theme::ThemeConfig;
use crate::window::{ResizeEdge, TitlebarHit, Window, WindowId, RESIZE_MARGIN};
use crate::workspace::{Workspace, WorkspaceId};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

struct DragState {
    window: WindowId,
    start_x: i32,
    start_y: i32,
    orig: Rect,
}

struct ResizeState {
    window: WindowId,
    edge: ResizeEdge,
    start_x: i32,
    start_y: i32,
    orig: Rect,
}

/// The platform-independent core of srdwm: owns window/workspace/monitor
/// state and layout policy. Backends (X11, Wayland, ...) drive this via
/// `add_window`/`remove_window`/input events, and apply the `Rect`s it
/// computes back onto real surfaces.
pub struct WindowManager {
    windows: HashMap<WindowId, Window>,
    order: Vec<WindowId>,
    focused: Option<WindowId>,
    monitors: Vec<Monitor>,
    workspaces: Vec<Workspace>,
    /// One flat value shared by every monitor - not per-output. Unlike
    /// Hyprland, srdwm has no notion of an independent workspace set per
    /// monitor; switching workspace changes what's visible on every screen
    /// at once. See `visible_windows`'s doc comment for the filter this
    /// actually drives.
    current_workspace: WorkspaceId,
    /// Whichever workspace was current immediately before the current one
    /// became current - see `switch_workspace`'s doc comment.
    previous_workspace: WorkspaceId,
    /// Read from `workspace.auto_back_and_forth`. When set, switching to
    /// the workspace that's already active switches to `previous_workspace`
    /// instead - sway's `workspace_auto_back_and_forth` behavior, a quick
    /// "jump back to whatever I was just on" toggle on a single keybinding.
    pub auto_back_and_forth: bool,
    next_workspace_id: WorkspaceId,
    next_window_id: WindowId,
    layouts: HashMap<String, Box<dyn Layout>>,
    pub tiling: TilingConfig,
    pub placement: PlacementConfig,
    /// Whether geometry changes made via `toggle_maximize`/`toggle_fullscreen`
    /// should be animated. Read from `general.animations`; a backend's open
    /// animation is gated on this too, since core has no notion of "open".
    pub animations_enabled: bool,
    /// Tween duration in milliseconds, read from `general.animation_duration`.
    pub animation_duration_ms: u32,
    /// Whether windows get a drop shadow. Read from `general.shadows`. A
    /// maximized or fullscreen window never gets one regardless of this --
    /// see the Wayland backend's shadow render call site - so this only
    /// ever turns it off entirely, not on for those.
    pub shadows_enabled: bool,
    /// Width, in pixels, of the resize grab band along a window's edges,
    /// read from `general.resize_margin`. See [`crate::window::RESIZE_MARGIN`]'s
    /// doc comment for the default and why it's what it is.
    pub resize_margin: i32,
    /// Whether a decorated window's content rounds its bottom two corners
    /// to match the titlebar's own curve (an undecorated/CSD window rounds
    /// all four). Read from `general.rounded_corners` - `None` when the
    /// user's config never touched that key at all (deliberately *not*
    /// defaulted in `crates/config`, unlike every other `general.*` key),
    /// so each backend can fall back to its own default rather than one
    /// baked in here: GLES/winit defaults on, udev/Pixman defaults off
    /// (an untested-on-real-hardware per-frame CPU cost for content that
    /// redraws constantly - see `crates/wayland/src/rounded_corners.rs`).
    /// `Some(_)` only when the user explicitly set it, and wins either way.
    pub rounded_corners_enabled: Option<bool>,
    /// Whether hovering a window (no click needed) focuses it, read from
    /// `general.focus_follows_mouse`. Off by default - matches
    /// `general.focus_follows_mouse`'s own documented default, and every
    /// desktop's convention of click-to-focus unless a user explicitly
    /// opts into the classic X11 sloppy-focus behaviour.
    pub focus_follows_mouse: bool,
    /// Whether hover-driven focus (above) also raises the window, not just
    /// focuses it - read from `general.auto_raise`. Meaningless (never
    /// consulted) while `focus_follows_mouse` is off, since a plain click
    /// already raises unconditionally regardless of this.
    pub auto_raise: bool,
    /// Default decoration colours and border width, read from `theme.colors.*`/
    /// `theme.decorations.*`. See `ThemeConfig`'s own doc comment.
    pub theme: ThemeConfig,
    drag: Option<DragState>,
    resize: Option<ResizeState>,
    rules: Vec<WindowRule>,
    /// Windows a client-close was requested for, drained once per tick by
    /// `main.rs`'s event loop and forwarded to `Platform::close`. Needed
    /// because `WindowManager` is platform-agnostic and has no way to send
    /// a client its close request directly - see `close_window`.
    close_requests: Vec<WindowId>,
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowManager {
    pub fn new() -> Self {
        let mut layouts: HashMap<String, Box<dyn Layout>> = HashMap::new();
        layouts.insert("tiling".into(), Box::new(MasterStackLayout));
        layouts.insert("dynamic".into(), Box::new(NoOpLayout("dynamic")));
        layouts.insert("floating".into(), Box::new(NoOpLayout("floating")));

        Self {
            windows: HashMap::new(),
            order: Vec::new(),
            focused: None,
            monitors: Vec::new(),
            workspaces: vec![Workspace::new(0, "1", "dynamic")],
            current_workspace: 0,
            previous_workspace: 0,
            auto_back_and_forth: false,
            next_workspace_id: 1,
            next_window_id: 1,
            layouts,
            tiling: TilingConfig::default(),
            placement: PlacementConfig::default(),
            animations_enabled: true,
            animation_duration_ms: 200,
            shadows_enabled: true,
            resize_margin: RESIZE_MARGIN,
            rounded_corners_enabled: None,
            focus_follows_mouse: false,
            auto_raise: false,
            theme: ThemeConfig::default(),
            drag: None,
            resize: None,
            rules: Vec::new(),
            close_requests: Vec::new(),
        }
    }

    /// Registers a window rule; on every subsequent `add_window`, the first
    /// rule whose matcher matches the new window has its actions applied.
    pub fn add_rule(&mut self, rule: WindowRule) {
        self.rules.push(rule);
    }

    pub fn register_layout(&mut self, name: impl Into<String>, layout: Box<dyn Layout>) {
        self.layouts.insert(name.into(), layout);
    }

    pub fn available_layouts(&self) -> Vec<&str> {
        self.layouts.keys().map(String::as_str).collect()
    }

}

mod dragresize;
mod focus;
mod hittest;
mod layout;
mod monitors;
mod windows;
mod winops;
mod workspaces;

#[cfg(test)]
mod tests;
