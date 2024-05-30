use regex::Regex;

use crate::geometry::Rect;
use crate::window::Window;
use crate::workspace::WorkspaceId;

/// Match criteria for a [`WindowRule`]. A matcher with every field `None`
/// matches nothing (an accidental `srd.rule({}, {...})` in config should be a
/// silent no-op, not "apply to every window"). Every field that is `Some`
/// must match (AND semantics) - matching the convention i3's multi-criteria
/// rules and bspwm's `class:instance:title` rules both already use.
///
/// `title_contains`/`class` (plain substring/exact match) are kept
/// alongside the regex fields below rather than folded into them: they
/// cover the large majority of real rules (`srd.rule({ class = "firefox" },
/// ...)`) with no regex syntax to get right, and are cheaper to evaluate.
/// `title_regex`/`class_regex`/`instance` exist for the cases that need
/// more precision - disambiguating a specific dialog by title while
/// leaving an app's main window alone, the concrete example that motivated
/// adding these - without forcing every simple rule to write one.
#[derive(Debug, Clone, Default)]
pub struct WindowMatch {
    /// Case-insensitive substring match against `Window::title`.
    pub title_contains: Option<String>,
    /// Case-insensitive exact match against `Window::app_id` (X11 `WM_CLASS`'s
    /// *class* half / Wayland `app_id`).
    pub class: Option<String>,
    /// Regex match against `Window::title`. Case-sensitive by default --
    /// write `(?i)` at the start of the pattern for case-insensitive,
    /// the same convention i3's own criteria use.
    pub title_regex: Option<Regex>,
    /// Regex match against `Window::app_id`.
    pub class_regex: Option<Regex>,
    /// Case-insensitive exact match against `Window::instance` (X11
    /// `WM_CLASS`'s *instance* half). Always fails to match on Wayland-native
    /// windows, which have no equivalent (`Window::instance` is always
    /// empty there) - an X11/XWayland-only criterion, same as bspwm's.
    pub instance: Option<String>,
}

impl WindowMatch {
    pub fn is_empty(&self) -> bool {
        self.title_contains.is_none() && self.class.is_none() && self.title_regex.is_none() && self.class_regex.is_none() && self.instance.is_none()
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
        if let Some(re) = &self.title_regex {
            if !re.is_match(&window.title) {
                return false;
            }
        }
        if let Some(re) = &self.class_regex {
            if !re.is_match(&window.app_id) {
                return false;
            }
        }
        if let Some(i) = &self.instance {
            if !window.instance.eq_ignore_ascii_case(i) {
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
    /// Always-on-top (Hyprland's `pin`).
    pub pinned: Option<bool>,
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
        let m = WindowMatch { title_contains: Some("firefox".into()), ..Default::default() };
        assert!(m.matches(&w));
    }

    #[test]
    fn class_match_is_case_insensitive_exact() {
        let mut w = Window::new(1, "");
        w.app_id = "Firefox".into();
        let m = WindowMatch { class: Some("firefox".into()), ..Default::default() };
        assert!(m.matches(&w));
        let mut w2 = Window::new(2, "");
        w2.app_id = "firefoxx".into();
        assert!(!m.matches(&w2));
    }

    #[test]
    fn title_regex_matches_a_specific_dialog_without_matching_the_main_window() {
        // The concrete case that motivated adding regex support at all:
        // disambiguating a specific dialog by title while leaving an app's
        // main window alone - not reliably possible with substring-only
        // matching if the dialog's title is a substring-superset situation
        // (or vice versa) that plain `contains` can't express.
        let m = WindowMatch { title_regex: Some(Regex::new(r"^Save File$").unwrap()), ..Default::default() };
        let dialog = Window::new(1, "Save File");
        let main = Window::new(2, "Save File - GNU Image Manipulation Program");
        assert!(m.matches(&dialog));
        assert!(!m.matches(&main));
    }

    #[test]
    fn class_regex_is_case_sensitive_unless_the_pattern_opts_in() {
        let mut w = Window::new(1, "");
        w.app_id = "firefox".into();
        let sensitive = WindowMatch { class_regex: Some(Regex::new(r"^Firefox$").unwrap()), ..Default::default() };
        assert!(!sensitive.matches(&w));
        let insensitive = WindowMatch { class_regex: Some(Regex::new(r"(?i)^Firefox$").unwrap()), ..Default::default() };
        assert!(insensitive.matches(&w));
    }

    #[test]
    fn instance_match_is_case_insensitive_exact_and_independent_of_class() {
        let mut w = Window::new(1, "");
        w.app_id = "Navigator".into();
        w.instance = "firefox".into();
        let m = WindowMatch { instance: Some("Firefox".into()), ..Default::default() };
        assert!(m.matches(&w));
        let mut w2 = Window::new(2, "");
        w2.instance = "firefoxdeveloperedition".into();
        assert!(!m.matches(&w2));
    }

    #[test]
    fn multiple_criteria_are_combined_with_and() {
        let mut w = Window::new(1, "Preferences");
        w.app_id = "firefox".into();
        let m = WindowMatch { class: Some("firefox".into()), title_contains: Some("preferences".into()), ..Default::default() };
        assert!(m.matches(&w));
        // Same class, different title - must not match once title is
        // also a criterion.
        let mut w2 = Window::new(2, "Mozilla Firefox");
        w2.app_id = "firefox".into();
        assert!(!m.matches(&w2));
    }

    #[test]
    fn a_nonempty_instance_criterion_does_not_match_a_wayland_native_window() {
        // `Window::instance` is always empty on Wayland (no equivalent
        // concept), so a real `instance` rule (never an empty-string one --
        // nobody writes `instance = ""`) must not match there.
        let w = Window::new(1, "");
        let m = WindowMatch { instance: Some("firefox".into()), ..Default::default() };
        assert!(!m.matches(&w));
    }
}
