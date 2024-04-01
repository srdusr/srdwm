use crate::geometry::Rect;
use crate::monitor::Monitor;
use crate::window::WindowId;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct TilingConfig {
    pub master_ratio: f32,
    pub master_count: usize,
    pub gap_inner: u32,
    pub gap_outer: u32,
}

impl Default for TilingConfig {
    fn default() -> Self {
        Self { master_ratio: 0.6, master_count: 1, gap_inner: 8, gap_outer: 16 }
    }
}

/// Arranges a set of windows on a monitor. Implementations are pure
/// functions of (window order, monitor, config) -> new geometries, which
/// keeps them trivially unit-testable and free of platform coupling.
pub trait Layout: Send + Sync {
    fn name(&self) -> &'static str;
    fn arrange(&self, windows: &[WindowId], monitor: &Monitor, cfg: &TilingConfig) -> HashMap<WindowId, Rect>;
}

/// Master-stack tiling (dwm/i3-style): the first `master_count` windows take
/// a resizable master column on the left; the rest are split evenly in a
/// stack column on the right. This replaces the legacy C++ placeholder,
/// which only ever split windows into equal-width columns.
pub struct MasterStackLayout;

impl Layout for MasterStackLayout {
    fn name(&self) -> &'static str {
        "tiling"
    }

    fn arrange(&self, windows: &[WindowId], monitor: &Monitor, cfg: &TilingConfig) -> HashMap<WindowId, Rect> {
        let mut out = HashMap::new();
        if windows.is_empty() {
            return out;
        }
        let area = monitor.geometry.inset(cfg.gap_outer);
        if windows.len() == 1 {
            out.insert(windows[0], area);
            return out;
        }

        let master_count = cfg.master_count.max(1).min(windows.len());
        let has_stack = windows.len() > master_count;
        let master_width = if has_stack {
            ((area.width as f32) * cfg.master_ratio) as u32
        } else {
            area.width
        };

        let half_gap = cfg.gap_inner / 2;
        let master_h = area.height / master_count as u32;
        for (i, &id) in windows[..master_count].iter().enumerate() {
            let r = Rect {
                x: area.x,
                y: area.y + (i as u32 * master_h) as i32,
                width: master_width.saturating_sub(half_gap),
                height: master_h.saturating_sub(cfg.gap_inner),
            };
            out.insert(id, r);
        }

        if has_stack {
            let stack = &windows[master_count..];
            let stack_x = area.x + master_width as i32 + half_gap as i32;
            let stack_width = area.width.saturating_sub(master_width + half_gap);
            let stack_h = area.height / stack.len() as u32;
            for (i, &id) in stack.iter().enumerate() {
                let r = Rect {
                    x: stack_x,
                    y: area.y + (i as u32 * stack_h) as i32,
                    width: stack_width,
                    height: stack_h.saturating_sub(cfg.gap_inner),
                };
                out.insert(id, r);
            }
        }
        out
    }
}

/// "Dynamic" layout: windows keep whatever geometry they already have.
/// Placement for *new* windows is handled separately by [`crate::placement::SmartPlacement`]
/// at creation time; this layout never repositions existing windows. This
/// mirrors the legacy design intent (see docs/PRIOR_ART.md) but, unlike the
/// C++ version, is a deliberate no-op rather than an accidental one.
pub struct NoOpLayout(pub &'static str);

impl Layout for NoOpLayout {
    fn name(&self) -> &'static str {
        self.0
    }

    fn arrange(&self, _windows: &[WindowId], _monitor: &Monitor, _cfg: &TilingConfig) -> HashMap<WindowId, Rect> {
        HashMap::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Monitor {
        Monitor::new(0, "test", Rect::new(0, 0, 1920, 1080))
    }

    #[test]
    fn single_window_fills_monitor_minus_outer_gap() {
        let layout = MasterStackLayout;
        let cfg = TilingConfig { gap_outer: 20, ..Default::default() };
        let result = layout.arrange(&[1], &monitor(), &cfg);
        assert_eq!(result[&1], Rect::new(20, 20, 1880, 1040));
    }

    #[test]
    fn two_windows_split_master_and_stack() {
        let layout = MasterStackLayout;
        let cfg = TilingConfig { gap_outer: 0, gap_inner: 0, master_ratio: 0.6, master_count: 1 };
        let result = layout.arrange(&[1, 2], &monitor(), &cfg);
        assert_eq!(result[&1].width, 1152); // 60% of 1920
        assert_eq!(result[&2].width, 768);
        assert_eq!(result[&1].height, 1080);
        assert_eq!(result[&2].height, 1080);
    }

    #[test]
    fn three_windows_stack_splits_remaining_height_evenly() {
        let layout = MasterStackLayout;
        let cfg = TilingConfig { gap_outer: 0, gap_inner: 0, master_ratio: 0.5, master_count: 1 };
        let result = layout.arrange(&[1, 2, 3], &monitor(), &cfg);
        assert_eq!(result[&2].height, 540);
        assert_eq!(result[&3].height, 540);
        assert_eq!(result[&2].y, 0);
        assert_eq!(result[&3].y, 540);
    }

    #[test]
    fn no_op_layout_never_moves_windows() {
        let layout = NoOpLayout("dynamic");
        let result = layout.arrange(&[1, 2, 3], &monitor(), &TilingConfig::default());
        assert!(result.is_empty());
    }
}
