use crate::geometry::Rect;
use crate::window::Window;
use crate::workspace::WorkspaceId;

/// Match criteria for a [`WindowRule`]. A matcher with every field `None`
/// matches nothing (an accidental `srd.rule({}, {...})` in config should be a
/// silent no-op, not "apply to every window").
#[derive(Debug, Clone, Default)]
pub struct WindowMatch {
    /// Case-insensitive substring match against `Window::title`.
    pub title_contains: Option<String>,
    /// Case-insensitive exact match against `Window::app_id` (X11 `WM_CLASS`
    /// / Wayland `app_id`).
    pub class: Option<String>,
}

impl WindowMatch {
    pub fn is_empty(&self) -> bool {
        self.title_contains.is_none() && self.class.is_none()
    }

    pub fn matches(&self, window: &Window) -> bool {
        if self.is_empty() {
            return false;
        }
        if let Some(t) = &self.title_contains {
            if !window.title.to_lowercase().contains(&t.to_lowercase()) {
                return false;
            }
        }
        if let Some(c) = &self.class {
            if !window.app_id.eq_ignore_ascii_case(c) {
                return false;
            }
        }
        true
    }
}

/// Actions applied once, when a matching window is first added.
#[derive(Debug, Clone, Default)]
pub struct WindowRuleActions {
    pub floating: Option<bool>,
    pub maximized: Option<bool>,
    pub workspace: Option<WorkspaceId>,
    pub geometry: Option<Rect>,
    pub decorated: Option<bool>,
    pub border_color: Option<(u8, u8, u8)>,
    pub border_width: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct WindowRule {
    pub matcher: WindowMatch,
    pub actions: WindowRuleActions,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_matcher_matches_nothing() {
        let w = Window::new(1, "anything");
        assert!(!WindowMatch::default().matches(&w));
    }

    #[test]
    fn title_match_is_case_insensitive_substring() {
        let w = Window::new(1, "Mozilla Firefox");
        let m = WindowMatch { title_contains: Some("firefox".into()), class: None };
        assert!(m.matches(&w));
    }

    #[test]
    fn class_match_is_case_insensitive_exact() {
        let mut w = Window::new(1, "");
        w.app_id = "Firefox".into();
        let m = WindowMatch { title_contains: None, class: Some("firefox".into()) };
        assert!(m.matches(&w));
        let mut w2 = Window::new(2, "");
        w2.app_id = "firefoxx".into();
        assert!(!m.matches(&w2));
    }
}
