    use super::*;

    fn wm_with_monitor() -> WindowManager {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![{
            let mut m = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1080));
            m.primary = true;
            m
        }]);
        wm
    }

    #[test]
    fn new_window_on_dynamic_workspace_uses_smart_placement() {
        let mut wm = wm_with_monitor();
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "first");
        w.geometry = Rect::new(0, 0, 400, 300);
        wm.add_window(w);
        let placed = wm.window(id).unwrap().geometry;
        // The first window on an empty workspace cascades (see
        // `SmartPlacement::place`'s own doc comment on why grid is
        // skipped entirely when nothing else is open), starting at
        // `cascade_offset`, not (0,0).
        assert_eq!(placed.x, wm.placement.cascade_offset);
    }

    #[test]
    fn add_window_picks_up_the_configured_default_decoration_mode() {
        let mut wm = wm_with_monitor();
        wm.theme.default_decorated = false;
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        assert!(!wm.window(id).unwrap().decorated, "must pick up the live theme default, not Window::new's own hardcoded one");
    }

    #[test]
    fn phone_mode_maximizes_a_new_window_by_default() {
        let mut wm = wm_with_monitor();
        wm.phone_mode = true;
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        assert!(wm.window(id).unwrap().maximized);
    }

    #[test]
    fn phone_mode_does_not_maximize_a_window_a_rule_floats() {
        let mut wm = wm_with_monitor();
        wm.phone_mode = true;
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("popup".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { floating: Some(true), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        w.app_id = "popup".into();
        wm.add_window(w);
        assert!(!wm.window(id).unwrap().maximized, "a window a rule explicitly floats is meant to stay small, phone mode or not");
    }

    #[test]
    fn a_rules_explicit_maximized_false_still_wins_in_phone_mode() {
        let mut wm = wm_with_monitor();
        wm.phone_mode = true;
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("widget".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { maximized: Some(false), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        w.app_id = "widget".into();
        wm.add_window(w);
        assert!(!wm.window(id).unwrap().maximized, "an explicit rule action must win over phone mode's own default");
    }

    #[test]
    fn phone_mode_off_leaves_ordinary_placement_unaffected() {
        let mut wm = wm_with_monitor();
        assert!(!wm.phone_mode, "sanity: default is off");
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        assert!(!wm.window(id).unwrap().maximized);
    }

    #[test]
    fn a_rules_decorated_action_still_overrides_the_theme_default() {
        let mut wm = wm_with_monitor();
        wm.theme.default_decorated = false;
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("nemo".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { decorated: Some(true), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        w.app_id = "nemo".into();
        wm.add_window(w);
        assert!(wm.window(id).unwrap().decorated, "an explicit rule must still win over the theme-wide default");
    }

    #[test]
    fn a_decorated_false_rule_applies_once_app_id_becomes_known_after_creation() {
        // The real native-Wayland scenario `Window::rules_applied`'s own
        // doc comment describes: `add_window` sees an empty title/app_id
        // (xdg_toplevel's own set_app_id/set_title requests land on a
        // later commit, not at surface creation), so the real rule match
        // has to wait for `reapply_rules_if_pending` - this is the one
        // path `a_rules_decorated_action_still_overrides_the_theme_default`
        // above does NOT cover, since that test sets `app_id` before ever
        // calling `add_window` at all.
        let mut wm = wm_with_monitor();
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("firefox".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { decorated: Some(false), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        let w = Window::new(id, "");
        wm.add_window(w);
        assert!(wm.window(id).unwrap().decorated, "nothing could have matched yet with an empty app_id - still the theme default (true)");
        assert!(!wm.window(id).unwrap().rules_applied, "must stay pending, not falsely marked settled");

        if let Some(win) = wm.window_mut(id) {
            win.app_id = "firefox".into();
            win.title = "Mozilla Firefox".into();
        }
        let reapplied = wm.reapply_rules_if_pending(id);
        assert!(reapplied, "the now-real app_id should let the firefox rule match");
        assert!(!wm.window(id).unwrap().decorated, "the rule's decorated=false must actually take effect");
    }

    #[test]
    fn tiling_workspace_arranges_two_windows_side_by_side() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));

        wm.arrange_workspace(wm.current_workspace());
        let ra = wm.window(a).unwrap().geometry;
        let rb = wm.window(b).unwrap().geometry;
        assert!(!ra.overlaps(&rb));
        assert_eq!(ra.y, rb.y);
        assert!(ra.x < rb.x);
    }

    #[test]
    fn floating_window_is_skipped_by_tiling_arrange() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        wm.toggle_floating(a);
        let before = wm.window(a).unwrap().geometry;
        wm.arrange_workspace(wm.current_workspace());
        assert_eq!(wm.window(a).unwrap().geometry, before);
    }

    #[test]
    fn focus_cycles_forward_and_wraps() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        // `b` was added last, so it's focused.
        assert_eq!(wm.focused_id(), Some(b));
        wm.focus_next();
        assert_eq!(wm.focused_id(), Some(a));
        wm.focus_next();
        assert_eq!(wm.focused_id(), Some(b));
    }

    #[test]
    fn minimized_window_is_skipped_by_focus_cycling() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        wm.minimize_window(a);
        wm.focus_window(b);
        wm.focus_next();
        assert_eq!(wm.focused_id(), Some(b), "only unminimized window should ever be focused");
    }

    #[test]
    fn drag_moves_window_by_pointer_delta() {
        let mut wm = wm_with_monitor();
        // "tiling" layout leaves add_window's requested geometry alone;
        // "dynamic"/"floating" would override it via SmartPlacement, which
        // these tests aren't exercising.
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(300, 300, 400, 300);
        wm.add_window(w);
        wm.start_drag(a, 310, 310);
        wm.update_drag(360, 340);
        let g = wm.window(a).unwrap().geometry;
        assert_eq!((g.x, g.y), (350, 330));
        wm.end_drag();
        assert!(!wm.is_dragging());
    }

    #[test]
    fn dragging_across_a_monitor_boundary_updates_monitor_live_not_just_at_end() {
        // Reported live: a window dragged onto a second monitor with a
        // different scale (confirmed live: 1.0 and ~0.84) "looks very
        // messed up" - `state/geometry.rs::sync_geometry` reads `w.
        // monitor` on every drag motion tick to pick which scale converts
        // the client's physical size into the logical points `xdg_
        // toplevel::configure` sends it, and `w.monitor` used to only get
        // corrected once, at `end_drag`, leaving every mid-drag configure
        // computed against the wrong monitor's scale for the drag's whole
        // remaining duration.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(1000, 100, 200, 150); // fully on monitor 0
        wm.add_window(w);
        wm.window_mut(a).unwrap().monitor = 0;
        wm.start_drag(a, 1010, 110);
        assert_eq!(wm.window(a).unwrap().monitor, 0);
        // Dragged fully onto monitor 1 - checked immediately, before
        // `end_drag` runs at all.
        wm.update_drag(1600, 110);
        assert_eq!(wm.window(a).unwrap().monitor, 1, "monitor must update live during the drag, not only once it ends");
    }

    #[test]
    fn drag_ending_near_edge_snaps_to_half_screen() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(500, 500, 400, 300);
        wm.add_window(w);
        wm.start_drag(a, 510, 510);
        wm.update_drag(15, 510); // drag far left, landing within snap_threshold (8px) of edge 0
        wm.end_drag();
        let g = wm.window(a).unwrap().geometry;
        assert_eq!(g, Rect::new(0, 0, 960, 1080));
    }

    #[test]
    fn resize_from_bottom_right_grows_size_only() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.start_resize(a, ResizeEdge::BottomRight, 400, 300);
        wm.update_resize(450, 340);
        let g = wm.window(a).unwrap().geometry;
        assert_eq!(g, Rect::new(100, 100, 350, 240));
        wm.end_resize();
        assert!(!wm.is_resizing());
    }

    #[test]
    fn resizing_across_a_monitor_boundary_updates_monitor_live() {
        // Same fix as `dragging_across_a_monitor_boundary_updates_monitor_
        // live_not_just_at_end`'s own doc comment - a resize can carry the
        // edge being dragged onto a different monitor just as easily as a
        // move can carry the whole window, and `update_resize` never
        // corrected `w.monitor` at all before this fix, not even at the
        // end.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(1000, 100, 500, 150); // fully on monitor 0, right edge at 1500
        wm.add_window(w);
        wm.window_mut(a).unwrap().monitor = 0;
        wm.start_resize(a, ResizeEdge::Left, 1050, 100);
        // Drags the left edge from 1000 to 1290 (right edge anchored at
        // 1500, final width 210 - comfortably above MIN_WINDOW_WIDTH),
        // landing the whole window past the 1280 boundary on monitor 1.
        wm.update_resize(1340, 100);
        let g = wm.window(a).unwrap().geometry;
        assert_eq!(g, Rect::new(1290, 100, 210, 150), "sanity: resize must actually have cleared the boundary");
        assert_eq!(wm.window(a).unwrap().monitor, 1, "monitor must update live during the resize");
    }

    #[test]
    fn ending_a_resize_remembers_the_new_size_for_the_apps_next_window() {
        let mut wm = wm_with_monitor();
        // Tiling layout, so `add_window` skips `SmartPlacement`'s grid/
        // cascade sizing entirely and the asserted geometry below reflects
        // only the remembered-size lookup itself, not incidental grid math.
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.app_id = "alacritty".into();
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.start_resize(a, ResizeEdge::BottomRight, 400, 300);
        wm.update_resize(500, 400);
        wm.end_resize();

        let b = wm.alloc_window_id();
        let mut w2 = Window::new(b, "b");
        w2.app_id = "alacritty".into();
        // Whatever a backend would have hardcoded before calling add_window --
        // the remembered size must win over this, not just supplement it.
        w2.geometry = Rect::new(0, 0, 800, 600);
        wm.add_window(w2);
        let placed = wm.window(b).unwrap().geometry;
        assert_eq!((placed.width, placed.height), (400, 300), "the second alacritty window must open at the size the first was resized to");
    }

    #[test]
    fn remembered_size_is_keyed_by_app_id_not_shared_across_different_apps() {
        let mut wm = wm_with_monitor();
        // Tiling layout, so `add_window` skips `SmartPlacement`'s grid/
        // cascade sizing entirely and the asserted geometry below reflects
        // only the remembered-size lookup itself, not incidental grid math.
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.app_id = "alacritty".into();
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.start_resize(a, ResizeEdge::BottomRight, 400, 300);
        wm.update_resize(500, 400);
        wm.end_resize();

        let b = wm.alloc_window_id();
        let mut w2 = Window::new(b, "b");
        w2.app_id = "firefox".into();
        w2.geometry = Rect::new(0, 0, 800, 600);
        wm.add_window(w2);
        let placed = wm.window(b).unwrap().geometry;
        assert_eq!((placed.width, placed.height), (800, 600), "a different app's default size must be untouched by alacritty's remembered size");
    }

    #[test]
    fn a_dragged_window_remembers_its_new_position_for_the_next_same_app_window() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.app_id = "alacritty".into();
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.start_drag(a, 150, 150);
        wm.update_drag(650, 550);
        wm.end_drag();
        let dragged_to = wm.window(a).unwrap().geometry;

        let b = wm.alloc_window_id();
        let mut w2 = Window::new(b, "b");
        w2.app_id = "alacritty".into();
        w2.geometry = Rect::new(0, 0, 800, 600);
        wm.add_window(w2);
        let placed = wm.window(b).unwrap().geometry;
        assert_eq!((placed.x, placed.y), (dragged_to.x, dragged_to.y), "the second alacritty window must open where the first was dragged to");
    }

    #[test]
    fn closing_a_window_remembers_its_geometry_even_if_it_was_never_dragged_or_resized() {
        // Real report: "windows don't remember their placement/size" --
        // true for any window the user never manually touched, since only
        // `end_drag`/`end_resize` used to write `remembered_geometry` at
        // all. A window that was simply placed by SmartPlacement, looked
        // at, and closed had nothing recorded, so reopening it always fell
        // back to a fresh placement - indistinguishable from the memory
        // feature not existing at all for that (extremely common) case.
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.app_id = "alacritty".into();
        w.geometry = Rect::new(321, 111, 444, 222);
        wm.add_window(w);
        // Never dragged, never resized - closed exactly as SmartPlacement
        // left it.
        wm.remove_window(a);

        let b = wm.alloc_window_id();
        let mut w2 = Window::new(b, "b");
        w2.app_id = "alacritty".into();
        w2.geometry = Rect::new(0, 0, 800, 600);
        wm.add_window(w2);
        let placed = wm.window(b).unwrap().geometry;
        assert_eq!((placed.x, placed.y, placed.width, placed.height), (321, 111, 444, 222), "the next alacritty window must open where/how large the first one was when it closed");
    }

    #[test]
    fn a_remembered_position_on_a_monitor_that_no_longer_exists_falls_back_to_placement() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "dynamic");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.app_id = "alacritty".into();
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.start_drag(a, 150, 150);
        wm.update_drag(150, 150);
        wm.end_drag();
        // Simulate the monitor that position was remembered on being gone
        // (e.g. an external display unplugged since the last session) --
        // the only monitor left doesn't cover the remembered point at all.
        wm.set_monitors(vec![Monitor::new(1, "different", Rect::new(5000, 5000, 1920, 1080))]);

        let b = wm.alloc_window_id();
        let mut w2 = Window::new(b, "b");
        w2.app_id = "alacritty".into();
        w2.geometry = Rect::new(0, 0, 800, 600);
        wm.add_window(w2);
        let placed = wm.window(b).unwrap().geometry;
        assert!(placed.x >= 5000, "an invalid remembered position must fall back to placement on a real, currently-connected monitor, not be reused blindly");
    }

    #[test]
    fn maximizing_then_unmaximizing_does_not_change_the_remembered_size() {
        // Only an interactive drag-resize should update `remembered_sizes` --
        // maximize/fullscreen have their own separate `restore_geometry` and
        // are not "a size the user wants their next window to open at".
        let mut wm = wm_with_monitor();
        // Tiling layout, so `add_window` skips `SmartPlacement`'s grid/
        // cascade sizing entirely and the asserted geometry below reflects
        // only the remembered-size lookup itself, not incidental grid math.
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.app_id = "alacritty".into();
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.toggle_maximize(a);
        wm.toggle_maximize(a);

        let b = wm.alloc_window_id();
        let mut w2 = Window::new(b, "b");
        w2.app_id = "alacritty".into();
        w2.geometry = Rect::new(0, 0, 800, 600);
        wm.add_window(w2);
        let placed = wm.window(b).unwrap().geometry;
        assert_eq!((placed.width, placed.height), (800, 600), "maximize/unmaximize alone must not have remembered anything");
    }

    #[test]
    fn a_rules_explicit_geometry_still_wins_over_a_remembered_size() {
        let mut wm = wm_with_monitor();
        // Tiling layout, so `add_window` skips `SmartPlacement`'s grid/
        // cascade sizing entirely and the asserted geometry below reflects
        // only the remembered-size lookup itself, not incidental grid math.
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.app_id = "alacritty".into();
        w.geometry = Rect::new(100, 100, 300, 200);
        wm.add_window(w);
        wm.start_resize(a, ResizeEdge::BottomRight, 400, 300);
        wm.update_resize(500, 400);
        wm.end_resize();

        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("alacritty".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { geometry: Some(Rect::new(0, 0, 640, 480)), ..Default::default() },
        });
        let b = wm.alloc_window_id();
        let mut w2 = Window::new(b, "b");
        w2.app_id = "alacritty".into();
        w2.geometry = Rect::new(0, 0, 800, 600);
        wm.add_window(w2);
        let placed = wm.window(b).unwrap().geometry;
        assert_eq!((placed.width, placed.height), (640, 480), "a rule's explicit geometry is more specific and must win");
    }

    #[test]
    fn toggle_maximize_restores_original_geometry() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        let original = wm.window(a).unwrap().geometry;
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().geometry, Rect::new(0, 0, 1920, 1080));
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().geometry, original);
    }

    #[test]
    fn apply_snap_zone_resizes_to_the_named_zones_rect() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        wm.apply_snap_zone(a, SnapZoneKind::LeftHalf);
        assert_eq!(wm.window(a).unwrap().geometry, Rect::new(0, 0, 960, 1080));
    }

    #[test]
    fn apply_snap_zone_on_a_maximized_window_un_maximizes_it() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        wm.toggle_maximize(a);
        assert!(wm.window(a).unwrap().maximized);
        wm.apply_snap_zone(a, SnapZoneKind::TopRightQuarter);
        let w = wm.window(a).unwrap();
        assert!(!w.maximized, "snapping a maximized window must clear the maximized flag");
        assert_eq!(w.geometry, Rect::new(960, 0, 960, 540));
    }

    #[test]
    fn maximize_records_anim_from_when_animations_enabled() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        let placed = wm.window(a).unwrap().geometry;
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().anim_from, Some(placed));
    }

    #[test]
    fn maximize_does_not_record_anim_from_when_animations_disabled() {
        let mut wm = wm_with_monitor();
        wm.animations_enabled = false;
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        wm.toggle_maximize(a);
        assert_eq!(wm.window(a).unwrap().anim_from, None);
    }

    #[test]
    fn fullscreen_records_anim_from_covering_the_full_monitor() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut w = Window::new(a, "a");
        w.geometry = Rect::new(50, 50, 300, 200);
        wm.add_window(w);
        let placed = wm.window(a).unwrap().geometry;
        wm.toggle_fullscreen(a);
        assert_eq!(wm.window(a).unwrap().anim_from, Some(placed));
    }

    #[test]
    fn directional_focus_picks_nearest_window_in_that_direction() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let center = wm.alloc_window_id();
        let mut wc = Window::new(center, "center");
        wc.geometry = Rect::new(500, 500, 100, 100);
        wm.add_window(wc);
        let left = wm.alloc_window_id();
        let mut wl = Window::new(left, "left");
        wl.geometry = Rect::new(0, 500, 100, 100);
        wm.add_window(wl);
        let right = wm.alloc_window_id();
        let mut wr = Window::new(right, "right");
        wr.geometry = Rect::new(1000, 500, 100, 100);
        wm.add_window(wr);

        wm.focus_window(center);
        assert_eq!(wm.focus_direction(Direction::Left), Some(left));
        assert_eq!(wm.focused_id(), Some(left));

        wm.focus_window(center);
        assert_eq!(wm.focus_direction(Direction::Right), Some(right));
    }

    #[test]
    fn hit_test_prefers_topmost_window() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        let mut wa = Window::new(a, "a");
        wa.geometry = Rect::new(0, 0, 400, 300);
        wm.add_window(wa);
        let b = wm.alloc_window_id();
        let mut wb = Window::new(b, "b");
        wb.geometry = Rect::new(0, 0, 400, 300); // fully overlapping, added later -> on top
        wm.add_window(wb);

        let (hit_id, hit) = wm.hit_test(200, 10).unwrap();
        assert_eq!(hit_id, b);
        assert_eq!(hit, TitlebarHit::Drag);
    }

    #[test]
    fn hit_test_ignores_a_window_on_another_workspace_even_if_its_geometry_overlaps() {
        // Reported live: clicking a window sent the click to a different,
        // invisible window that merely happened to sit at the same screen
        // coordinates on a workspace that wasn't current. Rendering already
        // filtered by workspace (`visible_windows`); hit-testing didn't.
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut wa = Window::new(a, "a");
        wa.geometry = Rect::new(0, 0, 400, 300);
        wm.add_window(wa);
        // `a`'s own real, auto-placed geometry - read back rather than
        // assumed, since `SmartPlacement` (not the `Rect` set above,
        // which `add_window` overwrites) decides where it actually lands.
        let a_geom = wm.window(a).unwrap().geometry;
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        // Forced to genuinely identical geometry to `a` *after* placement
        // (`add_window`'s own `SmartPlacement` would otherwise place `b`
        // to avoid overlapping `a`, defeating this test's actual point --
        // the overlap here needs to be real, not incidental).
        wm.window_mut(b).unwrap().geometry = a_geom;
        let other_workspace = wm.add_workspace("2", "dynamic");
        wm.move_window_to_workspace(b, other_workspace); // b is now off-screen, not minimized

        // `hit_test` specifically means titlebar/border/resize-margin hits
        // (see its own doc comment) - a point in the window's plain
        // content area always resolves `None` there by design (`w`'s own
        // opaque content is in the way, `hit_test_with`'s own comment).
        // A few pixels below the top edge, horizontally centered, is
        // safely inside the titlebar band without landing in a corner
        // resize zone.
        let (px, py) = (a_geom.x + a_geom.width as i32 / 2, a_geom.y + 5);
        let (hit_id, _) = wm.hit_test(px, py).unwrap();
        assert_eq!(hit_id, a, "a click must land on the visible window, not one hidden on another workspace");
        // `window_at` (content-inclusive) is checked at the window's
        // actual center instead - unlike `hit_test`, it has no titlebar-
        // only restriction to work around.
        let center = (a_geom.x + a_geom.width as i32 / 2, a_geom.y + a_geom.height as i32 / 2);
        assert_eq!(wm.window_at(center.0, center.1), Some(a));
    }

    #[test]
    fn hit_test_does_not_see_through_a_covering_windows_content_to_a_lower_windows_edge() {
        // Reported live: a resize edge (or other titlebar/border zone)
        // could still be grabbed on a window that was fully covered by
        // another window on top of it, as long as the covering window's
        // own edges didn't happen to land on that exact point. `a`'s left
        // resize edge sits at x=0; `b` is stacked on top and covers that
        // point with its own real content, but `b`'s own edges are far
        // away (left at x=-100, nowhere near x=0), so `b` itself doesn't
        // register a hit there - the bug was falling through to `a`'s
        // edge underneath instead of stopping at `b`'s opaque content.
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        let mut wa = Window::new(a, "a");
        wa.geometry = Rect::new(0, 0, 400, 300);
        wm.add_window(wa);
        let b = wm.alloc_window_id();
        let mut wb = Window::new(b, "b");
        wb.geometry = Rect::new(-100, 0, 600, 300); // added later -> on top, fully covers a
        wm.add_window(wb);

        assert_eq!(wm.hit_test(0, 150), None, "a's edge must not be reachable through b's opaque content");
    }

    #[test]
    fn per_window_resize_margin_overrides_the_wm_wide_default() {
        // Hyprland's per-window `extend_border_grab_area` equivalent.
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling"); // keeps add_window from overriding geometry via SmartPlacement
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        // Tall enough that `CORNER_MARGIN * resize_margin`'s own widened
        // corner zone (150px at this override, see `CORNER_MARGIN`'s own
        // doc comment on why it's deliberately generous) doesn't reach
        // anywhere near the plain-edge point tested below - a shorter
        // window here used to work purely because the corner zone was
        // narrower, not because this test cared about corners at all.
        w.geometry = Rect::new(100, 100, 400, 800);
        w.resize_margin = Some(30);
        wm.add_window(w);

        // 15px in from the left edge: well past the WM-wide default (6px),
        // but still inside this window's own wider 30px override. Deep in
        // the window's own vertical middle, well clear of either corner
        // zone.
        let hit = wm.hit_test(115, 500);
        assert_eq!(hit.map(|(_, h)| h), Some(TitlebarHit::Resize(ResizeEdge::Left)));
    }

    #[test]
    fn moving_window_to_another_workspace_removes_it_from_current() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.move_window_to_workspace(a, ws2);
        assert_eq!(wm.visible_windows().count(), 0);
        wm.switch_workspace(ws2);
        assert_eq!(wm.visible_windows().count(), 1);
    }

    #[test]
    fn matching_rule_floats_new_window_on_add() {
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { title_contains: Some("calculator".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { floating: Some(true), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "Calculator"));
        assert!(wm.is_floating(id));
    }

    #[test]
    fn non_matching_rule_leaves_window_untouched() {
        let mut wm = wm_with_monitor();
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { title_contains: Some("calculator".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { floating: Some(true), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "Terminal"));
        assert!(!wm.is_floating(id));
    }

    #[test]
    fn rule_assigns_window_to_target_workspace() {
        let mut wm = wm_with_monitor();
        let target = wm.add_workspace("scratch", "dynamic");
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("scratchpad".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { workspace: Some(target), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "notes");
        w.app_id = "scratchpad".into();
        wm.add_window(w);
        assert_eq!(wm.window(id).unwrap().workspace, target);
    }

    #[test]
    fn removing_a_workspace_reassigns_its_windows() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        wm.remove_workspace(ws2);
        assert_ne!(wm.window(a).unwrap().workspace, ws2);
        assert!(wm.workspace(ws2).is_none());
    }

    #[test]
    fn rename_workspace_changes_the_display_name() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.rename_workspace(ws2, "code");
        assert_eq!(wm.workspace(ws2).unwrap().name, "code");
    }

    #[test]
    fn auto_back_and_forth_jumps_to_the_previous_workspace_when_reselecting_the_active_one() {
        let mut wm = wm_with_monitor();
        wm.auto_back_and_forth = true;
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), ws2);
        // Re-selecting the already-active workspace jumps back to 1, the
        // one that was active right before.
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), 1);
    }

    #[test]
    fn without_auto_back_and_forth_reselecting_the_active_workspace_is_a_plain_no_op() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), ws2);
    }

    #[test]
    fn switching_to_a_workspace_with_a_window_focuses_it() {
        // Regression test: `switch_workspace` used to only ever touch
        // `current_workspace`, never `self.focused` - reported live as
        // switching to a workspace with an open window leaving that window
        // unfocused while whatever was focused *before* the switch (now
        // invisible, off on the old workspace) kept receiving real
        // keyboard input.
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let ws2 = wm.add_workspace("2", "dynamic");
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        wm.move_window_to_workspace(b, ws2);
        wm.focus_window(a);
        assert_eq!(wm.focused_id(), Some(a), "sanity: a is focused on the original workspace");

        wm.switch_workspace(ws2);
        assert_eq!(wm.focused_id(), Some(b), "switching to a workspace with a window must focus it, not leave the old workspace's window focused");
    }

    #[test]
    fn switching_to_an_empty_workspace_clears_focus() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        wm.focus_window(a);
        let empty_ws = wm.add_workspace("2", "dynamic");

        wm.switch_workspace(empty_ws);
        assert_eq!(wm.focused_id(), None, "no window on the new workspace to focus, and the old one is no longer visible");
    }

    #[test]
    fn per_monitor_workspaces_off_by_default_switch_workspace_still_moves_every_monitor() {
        // Sanity: the new `per_monitor_workspaces` field must default to
        // `false` and leave shared-mode behaviour completely unchanged --
        // every existing workspace test above this one relies on that.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        assert!(!wm.per_monitor_workspaces, "shared mode must be the default");
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        assert_eq!(wm.workspace_for_monitor(0), ws2);
        assert_eq!(wm.workspace_for_monitor(1), ws2, "shared mode: every monitor must agree");
    }

    #[test]
    fn per_monitor_workspaces_on_switching_one_monitor_leaves_the_other_alone() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.per_monitor_workspaces = true;
        let ws2 = wm.add_workspace("2", "dynamic");

        wm.switch_workspace_on_monitor(ws2, 1);

        assert_eq!(wm.workspace_for_monitor(1), ws2, "monitor 1 switched");
        assert_eq!(wm.workspace_for_monitor(0), 1, "monitor 0 must still fall back to current_workspace, untouched");
    }

    #[test]
    fn per_monitor_workspaces_on_visible_windows_respects_each_monitors_own_workspace() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.per_monitor_workspaces = true;
        let ws2 = wm.add_workspace("2", "dynamic");

        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "on-monitor-0-workspace-1"));
        wm.window_mut(a).unwrap().monitor = 0;

        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "on-monitor-1-workspace-2"));
        wm.window_mut(b).unwrap().monitor = 1;
        wm.move_window_to_workspace(b, ws2);

        // Before switching monitor 1 to workspace 2, b isn't visible yet
        // (monitor 1 still falls back to workspace 1).
        assert!(!wm.visible_windows().any(|w| w.id == b));

        wm.switch_workspace_on_monitor(ws2, 1);

        let visible: Vec<_> = wm.visible_windows().map(|w| w.id).collect();
        assert!(visible.contains(&a), "monitor 0's own window must still be visible");
        assert!(visible.contains(&b), "monitor 1's window must become visible once its monitor switches to workspace 2");
    }

    #[test]
    fn per_monitor_workspaces_on_multiple_workspaces_can_be_active_at_once() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.per_monitor_workspaces = true;
        let ws2 = wm.add_workspace("2", "dynamic");

        wm.switch_workspace_on_monitor(ws2, 1);

        assert!(wm.is_workspace_visible(1), "monitor 0 is still showing workspace 1");
        assert!(wm.is_workspace_visible(ws2), "monitor 1 is showing workspace 2");
    }

    #[test]
    fn switching_to_a_workspace_where_the_already_focused_window_lives_is_a_no_op_for_focus() {
        // The auto-focus-on-switch behavior above must not fight
        // `focus_window`'s own workspace-follow call into `switch_workspace`
        // (see that function's doc comment): when a window on another
        // workspace is focused directly, that window - not merely "the
        // topmost window on its workspace" - must end up focused, even if
        // it isn't the topmost one.
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        wm.move_window_to_workspace(a, ws2);
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        wm.move_window_to_workspace(b, ws2);
        // b was added after a, so it's topmost - focusing a directly must
        // still result in a being focused, not b.
        wm.focus_window(a);
        assert_eq!(wm.focused_id(), Some(a));
    }

    #[test]
    fn focusing_a_window_on_another_workspace_switches_to_it() {
        // Regression test: `focus_window` used to mark the target focused
        // without ever touching `current_workspace` - reported live
        // (relayed from the AGS peer session, measured directly over IPC):
        // `srd dispatch focus <id>` on a window from a different workspace
        // left the active workspace unchanged and the newly-"focused"
        // window `visible: false`, so keyboard input had nowhere visible
        // to go while whatever was actually on screen kept looking
        // focused. Reachable by ordinary Alt-Tab, a dock icon, or anything
        // else that ends up calling `focus_window` on a window that isn't
        // on the current workspace.
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        wm.move_window_to_workspace(id, ws2);
        assert_eq!(wm.current_workspace(), 1, "sanity: still on the default workspace");

        wm.focus_window(id);
        assert_eq!(wm.current_workspace(), ws2, "focusing a window must bring its workspace along");
        assert_eq!(wm.focused_id(), Some(id));
    }

    #[test]
    fn focusing_a_minimized_window_also_restores_it() {
        // Regression: `focus_window` marked a window focused without
        // clearing `minimized` - a dock icon's click (foreign-toplevel
        // `Activate`, or the plain `"focus"` IPC command) both route
        // through here, so clicking a minimized app's dock icon left it
        // `focused: true` but still `minimized: true`, still excluded from
        // `visible_windows`/rendering. Reads exactly like the click did
        // nothing, since the window never actually reappears.
        let mut wm = wm_with_monitor();
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        wm.minimize_window(id);
        assert!(wm.window(id).unwrap().minimized, "sanity: actually minimized first");

        wm.focus_window(id);
        assert!(!wm.window(id).unwrap().minimized, "focusing a minimized window must restore it");
        assert_eq!(wm.focused_id(), Some(id));
        assert!(wm.visible_windows().any(|w| w.id == id));
    }

    #[test]
    fn refocusing_an_already_visible_window_does_not_trigger_auto_back_and_forth() {
        // The fix above must not call `switch_workspace` unconditionally --
        // `switch_workspace`'s own `auto_back_and_forth` handling treats
        // being asked to "switch" to the *already*-current workspace as a
        // deliberate toggle-to-previous gesture. An ordinary redundant
        // `focus_window` call (re-focusing something already focused and
        // already visible - ordinary mouse click traffic, not a workspace
        // switch request) must not be misread as that gesture and jump the
        // user to `previous_workspace` as a surprise side effect.
        let mut wm = wm_with_monitor();
        wm.auto_back_and_forth = true;
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.focus_window(id);
        assert_eq!(wm.current_workspace(), ws2, "must stay put - this is not a workspace-switch request");
    }

    #[test]
    fn switching_to_a_nonexistent_workspace_does_not_move_or_touch_previous() {
        let mut wm = wm_with_monitor();
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        wm.switch_workspace(9999);
        assert_eq!(wm.current_workspace(), ws2);
        // The failed switch must not have overwritten `previous_workspace`
        // either - auto_back_and_forth would otherwise jump to a
        // workspace id that was never really visited.
        wm.auto_back_and_forth = true;
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), 1);
    }

    #[test]
    fn output_position_requests_drain_in_arrival_order() {
        let mut wm = wm_with_monitor();
        wm.request_output_position(0, 100, 0);
        wm.request_output_position(1, 0, 0);
        assert_eq!(wm.drain_output_position_requests(), vec![(0, 100, 0), (1, 0, 0)]);
        // Draining empties the queue - a second drain with nothing new
        // queued in between must come back empty, not repeat the same
        // requests the backend already applied.
        assert!(wm.drain_output_position_requests().is_empty());
    }

    #[test]
    fn a_second_output_position_request_for_the_same_output_replaces_the_first() {
        // Only the latest requested position for a given output should
        // survive to the next drain - e.g. a display-settings panel
        // dragging a monitor preview around fires many requests for the
        // same output before the user lets go; the backend only needs to
        // apply where it ended up, not replay the whole drag.
        let mut wm = wm_with_monitor();
        wm.request_output_position(0, 100, 0);
        wm.request_output_position(0, 200, 50);
        assert_eq!(wm.drain_output_position_requests(), vec![(0, 200, 50)]);
    }

    #[test]
    fn rename_workspace_is_a_no_op_for_an_id_that_does_not_exist() {
        let mut wm = wm_with_monitor();
        wm.rename_workspace(9999, "ghost");
        assert!(wm.workspaces().iter().all(|w| w.name != "ghost"));
    }

    // ---- Scratchpad --------------------------------------------------------

    #[test]
    fn scratchpad_add_hides_the_window_and_marks_pool_membership() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        let w = wm.window(a).unwrap();
        assert!(w.scratchpad);
        assert!(w.minimized);
        assert!(w.floating);
        assert!(!wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_brings_back_the_hidden_window_and_focuses_it() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        wm.scratchpad_show();
        let w = wm.window(a).unwrap();
        assert!(!w.minimized);
        assert_eq!(wm.focused_id(), Some(a));
        assert!(wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_hides_again_when_the_shown_scratchpad_window_is_focused() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        wm.scratchpad_show(); // shows + focuses
        wm.scratchpad_show(); // toggles back off
        assert!(wm.window(a).unwrap().minimized);
        assert!(!wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_brings_it_back_even_when_minimized_through_a_different_path() {
        // A scratchpad window can be minimized several ways besides the
        // `scratchpad_show` toggle-off branch itself - a titlebar minimize
        // button, a client's own `minimize_request` (both ultimately call
        // this same `minimize_window`). `scratchpad` is pool membership,
        // tracked independently of *how* the window ended up minimized, so
        // pressing the scratchpad binding afterward must still find and
        // show it - not treat it as "already handled" just because
        // something other than `scratchpad_show` did the hiding.
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        wm.scratchpad_show(); // shown + focused
        wm.minimize_window(a); // hidden via the generic path, not the toggle
        assert!(wm.window(a).unwrap().scratchpad, "must still be pool-managed after an ordinary minimize");
        wm.scratchpad_show();
        let w = wm.window(a).unwrap();
        assert!(!w.minimized, "the binding must show it again, not treat it as already visible");
        assert_eq!(wm.focused_id(), Some(a));
    }

    #[test]
    fn scratchpad_show_moves_the_window_onto_the_current_workspace() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace(ws2);
        wm.scratchpad_show();
        assert_eq!(wm.window(a).unwrap().workspace, ws2);
        assert!(wm.visible_windows().any(|w| w.id == a));
    }

    #[test]
    fn scratchpad_show_with_no_scratchpad_windows_is_a_no_op() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "normal"));
        wm.scratchpad_show();
        assert_eq!(wm.focused_id(), Some(a));
        assert!(!wm.window(a).unwrap().minimized);
    }

    #[test]
    fn scratchpad_show_picks_the_most_recently_added_hidden_window() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "old"));
        wm.scratchpad_add(a);
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "new"));
        wm.scratchpad_add(b);
        wm.scratchpad_show();
        assert_eq!(wm.focused_id(), Some(b));
        assert!(wm.window(a).unwrap().minimized);
    }

    #[test]
    fn scratchpad_remove_leaves_current_visibility_untouched_but_drops_pool_membership() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "term"));
        wm.scratchpad_add(a);
        wm.scratchpad_remove(a);
        assert!(!wm.window(a).unwrap().scratchpad);
        assert!(wm.window(a).unwrap().minimized);
        // No longer scratchpad-managed, so a later `scratchpad_show` must
        // not touch it.
        wm.scratchpad_show();
        assert!(wm.window(a).unwrap().minimized);
    }

    // ---- Monitor hotplug -------------------------------------------------

    fn two_monitors() -> Vec<Monitor> {
        let mut a = Monitor::new(0, "primary", Rect::new(0, 0, 1280, 800));
        a.primary = true;
        let b = Monitor::new(1, "secondary", Rect::new(1280, 0, 1920, 1080));
        vec![a, b]
    }

    #[test]
    fn disabled_monitor_is_reported_but_never_shows_up_in_monitors() {
        // The whole point of keeping this separate from `set_monitors`:
        // real placement (`monitors()`) must never see a disabled output,
        // even though `srd monitors`/AGS's panel now needs to list it.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.set_disabled_monitor("HDMI-A-1".to_string(), Rect::new(1920, 0, 1920, 1080), Rect::new(1920, 0, 1920, 1080), false);

        assert_eq!(wm.monitors().len(), 2, "disabled_monitors must not leak into real placement's monitor list");
        let disabled: Vec<_> = wm.disabled_monitors().collect();
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled[0].0, "HDMI-A-1");
    }

    #[test]
    fn re_enabling_clears_the_disabled_monitor_record() {
        let mut wm = WindowManager::new();
        wm.set_disabled_monitor("HDMI-A-1".to_string(), Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1080), false);
        assert_eq!(wm.disabled_monitors().count(), 1);

        wm.clear_disabled_monitor("HDMI-A-1");
        assert_eq!(wm.disabled_monitors().count(), 0);
    }

    #[test]
    fn primary_secondary_layout_is_a_no_op_outside_per_monitor_workspaces_mode() {
        // Shared mode: every monitor shows the same one workspace, so a
        // primary/secondary split has nothing distinct to apply to.
        let mut wm = WindowManager::new();
        wm.primary_layout = "dynamic".to_string();
        wm.secondary_layout = "tiling".to_string();
        wm.set_monitors(two_monitors());
        assert_eq!(wm.workspace(1).unwrap().layout, "dynamic", "must not touch the shared workspace's layout");
    }

    #[test]
    fn primary_secondary_layout_applies_once_workspaces_are_split_per_monitor() {
        let mut wm = WindowManager::new();
        wm.per_monitor_workspaces = true;
        wm.primary_layout = "dynamic".to_string();
        wm.secondary_layout = "tiling".to_string();
        wm.set_monitors(two_monitors());
        // Give the secondary monitor its own workspace, same as a real
        // independent per-monitor switch would.
        let ws2 = wm.add_workspace("2", "dynamic");
        wm.switch_workspace_on_monitor(ws2, 1);
        // Re-applied on the next monitor-list refresh (a hotplug or
        // restart), not continuously - see `apply_monitor_layouts`'s own
        // doc comment for why it doesn't hook every workspace switch.
        wm.set_monitors(two_monitors());

        assert_eq!(wm.workspace(wm.workspace_for_monitor(0)).unwrap().layout, "dynamic");
        assert_eq!(wm.workspace(ws2).unwrap().layout, "tiling");
    }

    #[test]
    fn primary_secondary_layout_does_not_clobber_the_still_shared_workspace() {
        // Neither monitor has been independently switched yet - both
        // still resolve to the same fallback workspace. secondary_layout
        // must not stomp what primary_layout just set on it.
        let mut wm = WindowManager::new();
        wm.per_monitor_workspaces = true;
        wm.primary_layout = "dynamic".to_string();
        wm.secondary_layout = "tiling".to_string();
        wm.set_monitors(two_monitors());
        assert_eq!(wm.workspace(1).unwrap().layout, "dynamic");
    }

    #[test]
    fn unplugging_a_monitor_rehomes_its_windows_to_the_primary() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "on-second-monitor");
        w.geometry = Rect::new(1500, 200, 600, 400); // inside monitor 1 only
        wm.add_window(w);
        wm.window_mut(id).unwrap().monitor = 1;

        // Monitor 1 goes away.
        wm.set_monitors(vec![two_monitors().remove(0)]);

        let w = wm.window(id).unwrap();
        assert_eq!(w.monitor, 0, "window should be rehomed to the primary monitor");
        assert!(
            Rect::new(0, 0, 1280, 800).overlaps(&w.geometry),
            "rehomed window should be on-screen, got {:?}",
            w.geometry
        );
    }

    #[test]
    fn windows_already_on_a_surviving_monitor_are_left_alone() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "on-primary");
        w.geometry = Rect::new(10, 20, 300, 200);
        wm.add_window(w);
        wm.window_mut(id).unwrap().monitor = 0;
        wm.window_mut(id).unwrap().geometry = Rect::new(10, 20, 300, 200);

        wm.set_monitors(vec![two_monitors().remove(0)]);

        let w = wm.window(id).unwrap();
        assert_eq!(w.monitor, 0);
        assert_eq!(w.geometry, Rect::new(10, 20, 300, 200), "untouched window must not move");
    }

    #[test]
    fn a_window_still_overlapping_the_primary_keeps_its_geometry() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "straddling");
        wm.add_window(w.clone());
        // Straddles the boundary, so it still overlaps the primary.
        w.geometry = Rect::new(1200, 100, 400, 300);
        wm.window_mut(id).unwrap().monitor = 1;
        wm.window_mut(id).unwrap().geometry = w.geometry;

        wm.set_monitors(vec![two_monitors().remove(0)]);

        let got = wm.window(id).unwrap();
        assert_eq!(got.monitor, 0, "monitor id must still be remapped");
        assert_eq!(got.geometry, Rect::new(1200, 100, 400, 300), "already-visible geometry should be kept");
    }

    #[test]
    fn losing_every_monitor_leaves_windows_intact_for_when_one_returns() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "orphan");
        w.geometry = Rect::new(1500, 200, 600, 400);
        wm.add_window(w);
        wm.window_mut(id).unwrap().monitor = 1;
        wm.window_mut(id).unwrap().geometry = Rect::new(1500, 200, 600, 400);

        wm.set_monitors(Vec::new());

        let got = wm.window(id).unwrap();
        assert_eq!(got.geometry, Rect::new(1500, 200, 600, 400));
        assert_eq!(got.monitor, 1);
    }

    #[test]
    fn a_window_whose_monitor_field_is_stale_is_still_rescued() {
        // Regression: `add_window` assigns `monitor` from the *primary*
        // monitor, so a window placed on the second monitor by a rule (or
        // dragged there) keeps `monitor == 0`. Rehoming that keyed off the
        // field alone skipped this window entirely and left it off-screen.
        // Reproduced live by unplugging a monitor out from under an xterm.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let id = wm.alloc_window_id();
        let w = Window::new(id, "placed-by-rule");
        wm.add_window(w);
        // Geometry on monitor 1, but `monitor` still says 0 - exactly what
        // add_window + a geometry rule produce.
        wm.window_mut(id).unwrap().geometry = Rect::new(1500, 200, 600, 400);
        assert_eq!(wm.window(id).unwrap().monitor, 0, "precondition: stale field");

        wm.set_monitors(vec![two_monitors().remove(0)]);

        let got = wm.window(id).unwrap();
        assert!(
            Rect::new(0, 0, 1280, 800).overlaps(&got.geometry),
            "window must be pulled back on-screen, got {:?}",
            got.geometry
        );
    }

    #[test]
    fn a_new_window_lands_on_the_focused_windows_monitor_not_always_primary() {
        // Real bug, reported live: "why do all windows only open on the
        // first monitor" - `add_window` used to resolve its target
        // monitor via `primary_monitor()` unconditionally, so a second
        // monitor being the one the user was actually working on never
        // mattered at all.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let first = wm.alloc_window_id();
        wm.add_window(Window::new(first, "on-primary"));
        assert_eq!(wm.window(first).unwrap().monitor, 0, "sanity: nothing focused yet falls back to primary");

        // `add_window` itself focuses whatever it just added, so moving
        // this window onto the secondary monitor and leaving it focused is
        // enough to make it "the window the user is currently on" for the
        // next one.
        wm.window_mut(first).unwrap().monitor = 1;

        let second = wm.alloc_window_id();
        wm.add_window(Window::new(second, "should-follow-focus"));
        assert_eq!(wm.window(second).unwrap().monitor, 1, "a new window must land on the focused window's monitor, not primary");
    }

    #[test]
    fn a_new_window_lands_on_the_pointers_monitor_when_nothing_is_focused_there() {
        // Real bug, reported live: with nothing focused (a fresh session,
        // or the last-focused window sitting on a *different* monitor than
        // the one just clicked/hovered), a new window still fell all the
        // way back to primary - even though the user was demonstrably at
        // the second monitor when they launched it. `set_pointer_monitor`
        // is what a real backend's pointer-motion handler calls to tell
        // core this.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.set_pointer_monitor(Some(1));

        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "should-follow-pointer"));
        assert_eq!(wm.window(id).unwrap().monitor, 1, "a new window must land on the pointer's monitor when nothing is focused, not primary");
    }

    #[test]
    fn the_pointers_monitor_wins_over_a_stale_focused_window() {
        // Real bug, reported live: with a window focused on the first
        // monitor but the pointer now over the *second* monitor's bare
        // desktop (an empty workspace, or hovering a panel/dock that isn't
        // a core-tracked window - neither ever changes `self.focused`), a
        // freshly launched app still landed on the first monitor, where
        // the stale focus pointed, not the second monitor the user was
        // demonstrably at. `self.focused` only updates when a real window
        // is actually focused, so it can't tell "still working over there"
        // apart from "attention moved elsewhere, nothing there has been
        // focused yet" - `pointer_monitor` can, since it updates on every
        // motion event, so it wins first. See `add_window`'s own doc
        // comment for the full reasoning and the comparable-compositor
        // precedent (Hyprland, Mutter, sway's `focus_follows_mouse`).
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());

        let first = wm.alloc_window_id();
        wm.add_window(Window::new(first, "focused-on-primary"));
        wm.window_mut(first).unwrap().monitor = 0;
        wm.set_pointer_monitor(Some(1));

        let second = wm.alloc_window_id();
        wm.add_window(Window::new(second, "should-follow-the-pointer"));
        assert_eq!(wm.window(second).unwrap().monitor, 1, "the pointer's monitor must win over a stale focused window's");
    }

    // ---- Fullscreen ------------------------------------------------------

    #[test]
    fn fullscreen_covers_the_monitor_and_restores_the_original_geometry() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "app");
        w.geometry = Rect::new(100, 100, 400, 300);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(100, 100, 400, 300);

        wm.toggle_fullscreen(id);
        let got = wm.window(id).unwrap();
        assert!(got.fullscreen);
        assert_eq!(got.geometry, Rect::new(0, 0, 1280, 800), "should cover the whole monitor");
        assert!(!got.decorated, "fullscreen must drop the titlebar");

        wm.toggle_fullscreen(id);
        let got = wm.window(id).unwrap();
        assert!(!got.fullscreen);
        assert_eq!(got.geometry, Rect::new(100, 100, 400, 300));
        assert!(got.decorated);
    }

    #[test]
    fn fullscreen_round_trip_restores_a_client_side_decorated_window_to_undecorated() {
        // Regression test: exiting fullscreen used to hardcode
        // `decorated = true` unconditionally, which is only correct for a
        // window that was decorated to begin with. A window a rule sets
        // `decorated = false` for (client-side-decorated apps like
        // Firefox) that goes fullscreen and back used to come back
        // permanently `decorated = true` - with nothing to ever set it
        // back, since the client only negotiates its decoration mode once.
        // Since border/titlebar hit-testing is keyed off `Window.decorated`
        // directly, this made srdwm swallow every click near the top of
        // the window as a fake titlebar hit instead of forwarding it to
        // the client.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "firefox");
        w.geometry = Rect::new(100, 100, 400, 300);
        wm.add_window(w);
        // Set after `add_window`, not before - `add_window` now applies
        // `theme.default_decorated` unconditionally (same as `corner_radius`/
        // `border_color` already did), matching how a real client's
        // negotiated CSD mode actually lands in production too:
        // `set_decorated_from_mode` runs against an already-added window,
        // never folded into the `Window` passed into `add_window` itself.
        wm.window_mut(id).unwrap().decorated = false;

        wm.toggle_fullscreen(id);
        assert!(!wm.window(id).unwrap().decorated, "fullscreen itself must still drop the titlebar");

        wm.toggle_fullscreen(id);
        assert!(!wm.window(id).unwrap().decorated, "must restore the pre-fullscreen decorated=false, not default to true");
    }

    /// A monitor whose usable `geometry` is shrunk by a bottom dock's
    /// exclusive zone, distinct from its true `full_geometry` - the shape
    /// every real backend reports once a bar/dock has claimed space (see
    /// `Monitor::full_geometry`'s doc comment).
    fn monitor_with_dock() -> Monitor {
        let mut m = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1020));
        m.full_geometry = Rect::new(0, 0, 1920, 1080);
        // No top bar in this fixture - maximize ignores the dock the same
        // way fullscreen does, so it's the same rect as `full_geometry`.
        m.maximize_geometry = Rect::new(0, 0, 1920, 1080);
        m.primary = true;
        m
    }

    /// A monitor with *both* a bottom dock's exclusive zone and a top bar's,
    /// distinguishing `maximize_geometry` (stops at the bar, ignores the
    /// dock) from `full_geometry` (ignores both) and `geometry` (stops at
    /// both) - `monitor_with_dock` alone can't tell these apart since it
    /// has no bar to stop at.
    fn monitor_with_dock_and_bar() -> Monitor {
        let mut m = Monitor::new(0, "primary", Rect::new(0, 34, 1920, 986));
        m.full_geometry = Rect::new(0, 0, 1920, 1080);
        m.maximize_geometry = Rect::new(0, 34, 1920, 1046);
        m.primary = true;
        m
    }

    #[test]
    fn fullscreen_covers_the_full_monitor_ignoring_a_dock_reservation() {
        // Regression test: fullscreen used to target `Monitor::geometry`
        // (the usable, exclusive-zone-shrunk area) - so a fullscreened
        // window stopped short of a dock's reserved strip instead of
        // covering (or going under) it like fullscreen does everywhere
        // else. `full_geometry` is what fixes that. `toggle_maximize` now
        // targets the same rect (see `maximize_also_covers_the_full_monitor_
        // ignoring_a_dock_reservation` below) - on the user's own request,
        // not a bug fix - so this is no longer the one place `full_geometry`
        // matters, just the first.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.toggle_fullscreen(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080), "fullscreen must reach the true monitor edge, past the dock");
    }

    #[test]
    fn maximize_also_covers_the_full_monitor_ignoring_a_dock_reservation() {
        // `toggle_maximize` used to target `Monitor::geometry` (the usable,
        // exclusive-zone-shrunk area), deliberately different from
        // fullscreen's `full_geometry` - several desktops' convention of a
        // maximized window stopping short of a persistent dock. Changed on
        // the user's own request ("maximize should still go past dock
        // area/no dock in that mode"): maximize now covers the same full
        // rect fullscreen does, the only remaining difference being
        // `decorated`. A layer-shell client with its own overlap-based
        // auto-hide (AGS's dock) can react to the window now genuinely
        // overlapping its band - nothing here forces the dock/bar to hide.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.toggle_maximize(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080), "maximize must reach the true monitor edge, past the dock, same as fullscreen");
    }

    #[test]
    fn maximize_covers_a_dock_but_still_stops_at_a_top_bar() {
        // Live-tested regression: making maximize target `full_geometry`
        // (the test above) fixed "maximize stops at the dock" but as a side
        // effect also let it extend behind a top bar's reserved strip,
        // which was never asked for and was reported back once the user
        // actually tried it. `maximize_geometry` is the fix - distinct
        // from both `geometry` (stops at everything) and `full_geometry`
        // (stops at nothing).
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock_and_bar()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.toggle_maximize(id);
        assert_eq!(
            wm.window(id).unwrap().geometry,
            Rect::new(0, 34, 1920, 1046),
            "maximize must cover the dock's strip but still stop at the top bar's"
        );
    }

    #[test]
    fn maximized_window_live_tracks_a_monitor_geometry_change() {
        // Regression test: `set_monitors` updated `Monitor::geometry`/
        // `full_geometry` correctly but never touched already-maximized/
        // fullscreen windows' own `geometry`, so an already-maximized
        // window stayed stuck at its stale size until manually
        // un-maximized and re-maximized - reported live as "maximize does
        // not extend past the dock" even after the dock's own zone change
        // (or, now, monitor resize/reconnect) had already taken effect in
        // every other respect.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        wm.toggle_maximize(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080));

        // The monitor's real geometry changes (a resize, a reconnect at a
        // different resolution - the same code path a dock dropping its
        // exclusive zone used to exercise before maximize stopped
        // respecting that zone at all).
        let mut resized = Monitor::new(0, "primary", Rect::new(0, 0, 2560, 1420));
        resized.full_geometry = Rect::new(0, 0, 2560, 1440);
        resized.maximize_geometry = Rect::new(0, 0, 2560, 1440);
        resized.primary = true;
        wm.set_monitors(vec![resized]);

        assert_eq!(
            wm.window(id).unwrap().geometry,
            Rect::new(0, 0, 2560, 1440),
            "an already-maximized window must live-track a monitor geometry change, not just windows placed afterward"
        );
    }

    #[test]
    fn fullscreen_window_also_live_tracks_a_monitor_geometry_change() {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        wm.toggle_fullscreen(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080));

        let mut resized = Monitor::new(0, "primary", Rect::new(0, 0, 2560, 1420));
        resized.full_geometry = Rect::new(0, 0, 2560, 1440);
        resized.primary = true;
        wm.set_monitors(vec![resized]);

        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 2560, 1440), "fullscreen must live-track the true full rect, not the usable one");
    }

    #[test]
    fn a_non_maximized_window_is_left_alone_by_a_monitor_geometry_change() {
        // set_monitors' new re-sync pass is gated on maximized/fullscreen --
        // must not clobber an ordinary floating/tiled window's geometry just
        // because the monitor rect changed underneath it.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        w.geometry = Rect::new(100, 100, 400, 300);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(100, 100, 400, 300);

        let mut freed = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1080));
        freed.full_geometry = Rect::new(0, 0, 1920, 1080);
        freed.primary = true;
        wm.set_monitors(vec![freed]);

        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(100, 100, 400, 300));
    }

    #[test]
    fn dragging_a_window_can_cross_into_the_dock_reserved_strip() {
        // Regression test: `update_drag`'s clamp used to also use
        // `Monitor::geometry` (the shrunk usable area), which made it
        // physically impossible to ever drag a floating window into the
        // strip a dock reserves - not just discouraged, genuinely
        // unreachable at any drag speed or angle. `full_geometry` is what
        // makes that space reachable again; the dock still renders on top
        // as an overlay, same as it does everywhere else.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "a");
        w.geometry = Rect::new(500, 500, 200, 200);
        wm.add_window(w);

        wm.start_drag(id, 600, 600);
        // Drag far down - past the old usable-area bottom (1020) and
        // toward the true monitor bottom (1080).
        wm.update_drag(600, 5000);
        let g = wm.window(id).unwrap().geometry;
        // Old behavior (clamped to `geometry`, bottom 1020) would stop at
        // y=980; clamped to `full_geometry` (bottom 1080), it reaches 1040.
        assert_eq!(g.y, 1040, "must clamp against the true monitor bottom, not the dock-shrunk usable area");
    }

    #[test]
    fn class_rule_applies_once_app_id_is_known_after_creation() {
        // Regression test: `add_window` matches rules against whatever
        // `app_id`/`title` the window already has - for a native Wayland
        // client those are still empty at that moment (the real values
        // only arrive on a later commit, well after `new_toplevel`), so
        // every class-based rule - including `srd.rule({ class =
        // "firefox" }, { decorated = false })`, meant to stop srdwm
        // drawing a second titlebar over Firefox's own - silently never
        // matched. `reapply_rules_if_pending` is the retry a backend calls
        // once the real app_id is known.
        let mut wm = wm_with_monitor();
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("firefox".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { decorated: Some(false), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        // Empty app_id, exactly as a fresh native Wayland toplevel has it.
        wm.add_window(Window::new(id, ""));
        assert!(wm.window(id).unwrap().decorated, "no app_id yet, so no match - must not have flipped early");

        let w = wm.window_mut(id).unwrap();
        w.app_id = "firefox".into();
        wm.reapply_rules_if_pending(id);
        assert!(!wm.window(id).unwrap().decorated, "app_id now known - the rule must apply on retry");

        // A later, unrelated title change (e.g. a browser tab switching)
        // must not re-match and re-apply - rule actions apply once.
        let w = wm.window_mut(id).unwrap();
        w.decorated = true;
        w.title = "a new tab title".into();
        wm.reapply_rules_if_pending(id);
        assert!(wm.window(id).unwrap().decorated, "rules_applied is already true - must not re-run the match");
    }

    #[test]
    fn opacity_rule_applies_on_the_deferred_retry_same_as_other_actions() {
        // Regression test: `opacity` was added to `add_window`'s own rule
        // application but missed here, in the deferred retry
        // `reapply_rules_if_pending` - confirmed live: a rule like
        // `srd.rule({ class = "Alacritty" }, { opacity = 0.4 })` never took
        // effect for any real native Wayland client, since (per the test
        // above) that's the *only* path a class-based rule actually
        // matches through for one of those - `add_window`'s own match
        // attempt always fails first, against an as-yet-empty `app_id`.
        let mut wm = wm_with_monitor();
        wm.add_rule(WindowRule {
            matcher: crate::rules::WindowMatch { class: Some("alacritty".into()), ..Default::default() },
            actions: crate::rules::WindowRuleActions { opacity: Some(0.4), ..Default::default() },
        });
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, ""));
        assert_eq!(wm.window(id).unwrap().opacity, 1.0, "no app_id yet, so no match - must not have applied early");

        let w = wm.window_mut(id).unwrap();
        w.app_id = "Alacritty".into();
        wm.reapply_rules_if_pending(id);
        assert_eq!(wm.window(id).unwrap().opacity, 0.4, "app_id now known - the rule must apply on retry");
    }

    #[test]
    fn fullscreen_from_maximized_still_restores_the_pre_maximize_size() {
        // Both share `restore_geometry`; entering fullscreen from a
        // maximised window must not overwrite it with the monitor rect, or
        // the window could never get its real size back.
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "app");
        w.geometry = Rect::new(50, 60, 300, 200);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(50, 60, 300, 200);

        wm.toggle_maximize(id);
        wm.toggle_fullscreen(id);
        assert!(wm.is_fullscreen(id));
        assert!(!wm.window(id).unwrap().maximized, "the two states are mutually exclusive");

        wm.toggle_fullscreen(id);
        assert_eq!(
            wm.window(id).unwrap().geometry,
            Rect::new(50, 60, 300, 200),
            "must restore the size from before maximise, not the monitor rect"
        );
    }

    #[test]
    fn tiling_leaves_fullscreen_windows_alone() {
        let mut wm = WindowManager::new();
        wm.set_monitors(two_monitors());
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "tiled"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "full"));
        wm.toggle_fullscreen(b);

        let changes = wm.arrange_workspace(wm.current_workspace());
        assert!(
            !changes.iter().any(|(id, _)| *id == b),
            "a fullscreen window must not be re-tiled"
        );
        assert_eq!(wm.window(b).unwrap().geometry, Rect::new(0, 0, 1280, 800));
    }

    // ---- Directional move ------------------------------------------------

    #[test]
    fn moving_a_window_swaps_it_with_its_neighbour() {
        let mut wm = wm_with_monitor();
        let left = wm.alloc_window_id();
        let mut a = Window::new(left, "left");
        a.geometry = Rect::new(0, 0, 400, 400);
        wm.add_window(a);
        wm.window_mut(left).unwrap().geometry = Rect::new(0, 0, 400, 400);

        let right = wm.alloc_window_id();
        let mut b = Window::new(right, "right");
        b.geometry = Rect::new(600, 0, 400, 400);
        wm.add_window(b);
        wm.window_mut(right).unwrap().geometry = Rect::new(600, 0, 400, 400);

        wm.focus_window(left);
        let swapped = wm.move_window_direction(Direction::Right);

        assert_eq!(swapped, Some(right));
        assert_eq!(wm.window(left).unwrap().geometry, Rect::new(600, 0, 400, 400));
        assert_eq!(wm.window(right).unwrap().geometry, Rect::new(0, 0, 400, 400));
    }

    #[test]
    fn moving_with_no_neighbour_pushes_to_the_monitor_edge() {
        let mut wm = wm_with_monitor();
        let id = wm.alloc_window_id();
        let mut w = Window::new(id, "only");
        w.geometry = Rect::new(500, 300, 200, 150);
        wm.add_window(w);
        wm.window_mut(id).unwrap().geometry = Rect::new(500, 300, 200, 150);
        wm.focus_window(id);

        assert_eq!(wm.move_window_direction(Direction::Left), None);
        assert_eq!(wm.window(id).unwrap().geometry.x, 0, "should hug the left edge");

        wm.move_window_direction(Direction::Down);
        let g = wm.window(id).unwrap().geometry;
        let mon = wm.primary_monitor().unwrap().geometry;
        assert_eq!(g.bottom(), mon.bottom(), "should hug the bottom edge");
    }

    #[test]
    fn swapping_also_reorders_the_stack_so_tiling_follows() {
        // Under tiling the layout assigns slots from `order`, so a swap that
        // only exchanged geometry would be undone by the next arrange.
        let mut wm = wm_with_monitor();
        wm.set_layout(wm.current_workspace(), "tiling");
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        wm.arrange_workspace(wm.current_workspace());

        // Snapshot *after* focusing: `focus_window` raises, which reorders
        // on its own and would otherwise mask what the move did.
        wm.focus_window(a);
        let order_before: Vec<_> = wm.stacking_order().map(|w| w.id).collect();
        wm.move_window_direction(Direction::Right);
        let order_after: Vec<_> = wm.stacking_order().map(|w| w.id).collect();

        assert_ne!(order_before, order_after, "stacking order must reflect the swap");
        assert_eq!(
            order_after,
            order_before.iter().rev().copied().collect::<Vec<_>>(),
            "the two windows should have traded places in the stack"
        );
    }

    // ---- Always on top ---------------------------------------------------

    #[test]
    fn pinned_windows_stay_above_newly_raised_ones() {
        let mut wm = wm_with_monitor();
        let pinned = wm.alloc_window_id();
        wm.add_window(Window::new(pinned, "pip"));
        let other = wm.alloc_window_id();
        wm.add_window(Window::new(other, "normal"));

        wm.toggle_always_on_top(pinned);
        assert!(wm.is_always_on_top(pinned));
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned));

        // Raising a normal window must not bury the pinned one.
        wm.raise_window(other);
        assert_eq!(
            wm.stacking_order().last().map(|w| w.id),
            Some(pinned),
            "pinned window must remain topmost after another is raised"
        );
    }

    #[test]
    fn a_new_window_does_not_cover_a_pinned_one() {
        let mut wm = wm_with_monitor();
        let pinned = wm.alloc_window_id();
        wm.add_window(Window::new(pinned, "pip"));
        wm.toggle_always_on_top(pinned);

        let fresh = wm.alloc_window_id();
        wm.add_window(Window::new(fresh, "just opened"));

        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned));
    }

    #[test]
    fn unpinning_lets_a_window_fall_back_into_the_normal_stack() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));

        wm.toggle_always_on_top(a);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(a));
        wm.toggle_always_on_top(a);
        wm.raise_window(b);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(b));
    }

    #[test]
    fn lower_window_sends_it_to_the_back_of_the_stack() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let b = wm.alloc_window_id();
        wm.add_window(Window::new(b, "b"));
        let c = wm.alloc_window_id();
        wm.add_window(Window::new(c, "c"));
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(c), "precondition: c is on top after being added last");

        wm.lower_window(c);
        let order: Vec<_> = wm.stacking_order().map(|w| w.id).collect();
        assert_eq!(order, vec![c, a, b], "c must be at the very back, a/b unchanged relative to each other");
    }

    #[test]
    fn lower_window_never_buries_a_pinned_window() {
        let mut wm = wm_with_monitor();
        let a = wm.alloc_window_id();
        wm.add_window(Window::new(a, "a"));
        let pinned = wm.alloc_window_id();
        wm.add_window(Window::new(pinned, "pinned"));
        wm.toggle_always_on_top(pinned);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned));

        wm.lower_window(a);
        assert_eq!(wm.stacking_order().last().map(|w| w.id), Some(pinned), "a pinned window must stay on top even after an unrelated lower_window call");
    }
