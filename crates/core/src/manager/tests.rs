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
        // Grid placement starts at grid_margin, not (0,0).
        assert_eq!(placed.x, wm.placement.grid_margin as i32);
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
        let b = wm.alloc_window_id();
        let mut wb = Window::new(b, "b");
        wb.geometry = Rect::new(0, 0, 400, 300); // identical geometry to `a`
        wm.add_window(wb);
        let other_workspace = wm.add_workspace("2", "dynamic");
        wm.move_window_to_workspace(b, other_workspace); // b is now off-screen, not minimized

        let (hit_id, _) = wm.hit_test(200, 10).unwrap();
        assert_eq!(hit_id, a, "a click must land on the visible window, not one hidden on another workspace");
        assert_eq!(wm.window_at(200, 10), Some(a));
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
        // Re-selecting the already-active workspace jumps back to 0, the
        // one that was active right before.
        wm.switch_workspace(ws2);
        assert_eq!(wm.current_workspace(), 0);
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
        assert_eq!(wm.current_workspace(), 0);
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
        w.decorated = false;
        wm.add_window(w);

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
        m.primary = true;
        m
    }

    #[test]
    fn fullscreen_covers_the_full_monitor_ignoring_a_dock_reservation() {
        // Regression test: fullscreen used to target `Monitor::geometry`
        // (the usable, exclusive-zone-shrunk area), the same field maximize
        // correctly uses - so a fullscreened window stopped short of a
        // dock's reserved strip instead of covering (or going under) it
        // like fullscreen does everywhere else. `full_geometry` is what
        // fixes that; `geometry` must stay untouched so maximize keeps
        // respecting the dock.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.toggle_fullscreen(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080), "fullscreen must reach the true monitor edge, past the dock");
    }

    #[test]
    fn maximize_still_respects_the_dock_reservation() {
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));

        wm.toggle_maximize(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1020), "maximize must still stop at the dock, unlike fullscreen");
    }

    #[test]
    fn maximized_window_grows_when_the_dock_drops_its_reservation_live() {
        // Regression test: a dock that hides/reduces its exclusive zone
        // while a window is already maximized (an auto-hide dock reacting
        // to monocle/maximize, exactly the scenario an AGS peer session hit
        // live) used to leave that window stuck at its stale, dock-shrunk
        // size - `set_monitors` updated `Monitor::geometry` correctly but
        // never touched already-maximized/fullscreen windows' `geometry`,
        // so nothing re-grew until the window was manually un-maximized and
        // re-maximized.
        let mut wm = WindowManager::new();
        wm.set_monitors(vec![monitor_with_dock()]);
        let id = wm.alloc_window_id();
        wm.add_window(Window::new(id, "a"));
        wm.toggle_maximize(id);
        assert_eq!(wm.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1020));

        // The dock drops its exclusive zone to 0.
        let mut freed = Monitor::new(0, "primary", Rect::new(0, 0, 1920, 1080));
        freed.full_geometry = Rect::new(0, 0, 1920, 1080);
        freed.primary = true;
        wm.set_monitors(vec![freed]);

        assert_eq!(
            wm.window(id).unwrap().geometry,
            Rect::new(0, 0, 1920, 1080),
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
