    use super::*;
    use srdwm_core::Window;

    fn engine_in(dir: &std::path::Path) -> Engine {
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        Engine::new(wm, dir).unwrap()
    }

    #[test]
    fn srd_set_and_get_roundtrip_scalars() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine.lua.load(r#"srd.set("general.window_gap", 12)"#).exec().unwrap();
        assert_eq!(engine.get("general.window_gap"), Some(ConfigValue::Number(12.0)));
    }

    #[test]
    fn defaults_are_seeded_before_any_script_runs() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        assert_eq!(engine.get_string("general.default_layout", ""), "dynamic");
    }

    #[test]
    fn reset_restores_default_value() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine.lua.load(r#"srd.set("general.window_gap", 99)"#).exec().unwrap();
        engine.lua.load(r#"srd.reset("general.window_gap")"#).exec().unwrap();
        assert_eq!(engine.get("general.window_gap"), Some(ConfigValue::Number(8.0)));
    }

    #[test]
    fn bind_stores_real_closure_and_dispatch_runs_it() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(r#"srd.bind("Mod4+q", function() srd.set("test.marker", true) end)"#)
            .exec()
            .unwrap();
        assert!(engine.dispatch_keybinding("Mod4+q"));
        assert_eq!(engine.get("test.marker"), Some(ConfigValue::Bool(true)));
        assert!(!engine.dispatch_keybinding("Mod4+nonexistent"));
    }

    #[test]
    fn srd_is_requireable_not_just_a_global() {
        // Every shipped example config opens with `local srd = require("srd")`;
        // that must resolve through package.preload, not just exist as a global.
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(r#"local srd = require("srd"); srd.set("test.via_require", true)"#)
            .exec()
            .unwrap();
        assert_eq!(engine.get("test.via_require"), Some(ConfigValue::Bool(true)));
    }

    #[test]
    fn window_close_style_call_from_legacy_example_config_now_works() {
        // The legacy C++ engine's `srd.window.focused()` returned a
        // placeholder table with no methods, so `window:close()` in the
        // shipped example config would have errored at runtime. Here
        // `srd.window.close()` acts directly on the focused window.
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(Window::new(id, "test"));
        }
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine
            .lua
            .load(
                r#"
                local w = srd.window.focused()
                assert(w ~= nil, "expected a focused window")
                srd.window.set_floating(true)
                assert(srd.window.is_floating() == true)
                "#,
            )
            .exec()
            .unwrap();
    }

    #[test]
    fn layout_configure_updates_master_ratio_live() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine
            .lua
            .load(r#"srd.layout.configure("tiling", { master_ratio = 0.75 })"#)
            .exec()
            .unwrap();
        assert_eq!(engine.get("layout.tiling.master_ratio"), Some(ConfigValue::Number(0.75)));
        assert!((wm.borrow().tiling.master_ratio - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn a_reload_that_fails_leaves_the_previous_config_running() {
        // The single most likely thing to go wrong with a programmable
        // config: the user saves a syntax error. Before this, the clear
        // that precedes re-execution was never undone, so a bad edit left
        // the compositor with no keybindings at all.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("init.lua"), r#"srd.bind("Mod4+t", function() end)"#).unwrap();
        let engine = engine_in(dir.path());
        engine.load_init().unwrap();
        assert_eq!(engine.bound_keys().len(), 1, "the good config bound one key");

        std::fs::write(dir.path().join("init.lua"), "this is not lua ((").unwrap();
        assert!(engine.reload().is_err(), "a syntax error must be reported");
        assert_eq!(engine.bound_keys().len(), 1, "the working config must survive a failed reload");
        assert!(engine.dispatch_keybinding(&srdwm_core::canonicalize_key_combo("Mod4+t")), "and must still actually run");
    }

    #[test]
    fn a_reload_whose_file_has_vanished_also_keeps_the_previous_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("init.lua"), r#"srd.bind("Mod4+t", function() end)"#).unwrap();
        let engine = engine_in(dir.path());
        engine.load_init().unwrap();
        std::fs::remove_file(dir.path().join("init.lua")).unwrap();
        assert!(engine.reload().is_err());
        assert_eq!(engine.bound_keys().len(), 1);
    }

    #[test]
    fn a_reload_that_succeeds_still_drops_bindings_removed_from_the_file() {
        // The restore must not turn into "bindings are never cleared".
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("init.lua"), r#"srd.bind("Mod4+t", function() end) srd.bind("Mod4+y", function() end)"#).unwrap();
        let engine = engine_in(dir.path());
        engine.load_init().unwrap();
        assert_eq!(engine.bound_keys().len(), 2);

        std::fs::write(dir.path().join("init.lua"), r#"srd.bind("Mod4+t", function() end)"#).unwrap();
        engine.reload().unwrap();
        assert_eq!(engine.bound_keys().len(), 1, "a deleted binding must really go away");
    }

    #[test]
    fn a_half_applied_broken_config_is_discarded_not_merged() {
        // The failing file registers a binding and *then* errors. Keeping
        // it would leave a config that is neither the old one nor the new.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("init.lua"), r#"srd.bind("Mod4+t", function() end)"#).unwrap();
        let engine = engine_in(dir.path());
        engine.load_init().unwrap();

        std::fs::write(dir.path().join("init.lua"), "srd.bind(\"Mod4+z\", function() end)\nerror(\"boom\")").unwrap();
        assert!(engine.reload().is_err());
        let keys = engine.bound_keys();
        assert_eq!(keys.len(), 1);
        assert!(!keys.iter().any(|k| k.contains('z')), "the broken run's own binding must not survive: {keys:?}");
    }

    #[test]
    fn refresh_is_a_known_event_name() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine.lua.load(r#"srd.on("refresh", function() srd.set("refreshed", true) end)"#).exec().unwrap();
        assert!(engine.dispatch_event("refresh"), "a registered refresh handler must run");
        assert_eq!(engine.get("refreshed"), Some(ConfigValue::Bool(true)));
    }

    #[test]
    fn srd_lock_queues_a_lock_request_on_the_window_manager() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        assert!(!wm.borrow_mut().drain_lock_request(), "nothing queued yet");
        engine.lua.load("srd.lock()").exec().unwrap();
        assert!(wm.borrow_mut().drain_lock_request(), "srd.lock() must queue a real request");
    }

    #[test]
    fn srd_load_executes_module_relative_to_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("extra.lua"), r#"srd.set("from.extra", "yes")"#).unwrap();
        let engine = engine_in(dir.path());
        engine.lua.load(r#"srd.load("extra")"#).exec().unwrap();
        assert_eq!(engine.get("from.extra"), Some(ConfigValue::String("yes".into())));
    }

    #[test]
    fn validate_config_passes_on_untouched_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(r#"local ok, errs = srd.validate_config(); assert(ok, table.concat(errs, "; "))"#)
            .exec()
            .unwrap();
    }

    #[test]
    fn validate_config_flags_out_of_range_gap_and_bad_color() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(
                r#"
                srd.set("general.window_gap", 500)
                srd.set("theme.colors.background", "not-a-color")
                local ok, errs = srd.validate_config()
                assert(ok == false)
                assert(#errs == 2, "expected 2 errors, got " .. #errs)
                "#,
            )
            .exec()
            .unwrap();
    }

    #[test]
    fn validate_config_flags_unregistered_layout_name() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(
                r#"
                srd.set("general.default_layout", "nonexistent")
                local ok, errs = srd.validate_config()
                assert(ok == false)
                "#,
            )
            .exec()
            .unwrap();
    }

    #[test]
    fn debug_namespace_reports_status_and_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(
                r#"
                local status = srd.debug.config_status()
                assert(status.keys > 0)
                srd.debug.profile_start()
                local elapsed = srd.debug.profile_stop()
                assert(type(elapsed) == "number")
                local settings = srd.debug.show_settings()
                assert(settings["general.window_gap"] == 8)
                "#,
            )
            .exec()
            .unwrap();
    }

    #[test]
    fn srd_rule_floats_matching_window_on_creation() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine
            .lua
            .load(r#"srd.rule({ title = "calculator" }, { floating = true })"#)
            .exec()
            .unwrap();
        let id = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "Calculator"));
            id
        };
        assert!(wm.borrow().is_floating(id));
    }

    #[test]
    fn srd_rule_aspect_ratio_applies_a_parsed_w_h_pair_on_creation() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.rule({ class = "scrcpy" }, { aspect_ratio = "9:16" })"#).exec().unwrap();
        let id = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            let mut w = srdwm_core::Window::new(id, "phone");
            w.app_id = "scrcpy".into();
            wm.add_window(w);
            id
        };
        assert_eq!(wm.borrow().window(id).unwrap().aspect_ratio, Some((9, 16)));
    }

    #[test]
    fn srd_rule_rejects_a_malformed_aspect_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        let err = engine.lua.load(r#"srd.rule({ class = "x" }, { aspect_ratio = "sixteen by nine" })"#).exec();
        assert!(err.is_err());
    }

    #[test]
    fn srd_monitor_split_stores_a_split_request_by_connector_name() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.monitor.split("eDP-1", 2, "rows")"#).exec().unwrap();
        let split = wm.borrow().monitor_split("eDP-1").unwrap();
        assert_eq!(split.parts, 2);
        assert!(split.rows);
    }

    #[test]
    fn srd_monitor_split_direction_defaults_to_columns() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.monitor.split("HDMI-A-1", 3)"#).exec().unwrap();
        let split = wm.borrow().monitor_split("HDMI-A-1").unwrap();
        assert_eq!(split.parts, 3);
        assert!(!split.rows);
    }

    #[test]
    fn srd_monitor_scale_stores_a_factor_by_connector_name() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.monitor.scale("HDMI-A-1", 0.75)"#).exec().unwrap();
        assert_eq!(wm.borrow().monitor_scale("HDMI-A-1"), Some(0.75));
    }

    #[test]
    fn srd_monitor_scale_with_a_non_positive_factor_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.monitor.scale("HDMI-A-1", 0.75)"#).exec().unwrap();
        engine.lua.load(r#"srd.monitor.scale("HDMI-A-1", 0)"#).exec().unwrap();
        assert_eq!(wm.borrow().monitor_scale("HDMI-A-1"), None);
    }

    #[test]
    fn srd_monitor_split_with_one_part_clears_an_existing_split() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.monitor.split("eDP-1", 2)"#).exec().unwrap();
        assert!(wm.borrow().monitor_split("eDP-1").is_some());
        engine.lua.load(r#"srd.monitor.split("eDP-1", 1)"#).exec().unwrap();
        assert!(wm.borrow().monitor_split("eDP-1").is_none());
    }

    #[test]
    fn srd_window_scratchpad_hides_the_focused_window_and_show_brings_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let id = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(Window::new(id, "term"));
            id
        };
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.window.scratchpad()"#).exec().unwrap();
        assert!(wm.borrow().window(id).unwrap().minimized);
        assert!(wm.borrow().window(id).unwrap().scratchpad);
        engine.lua.load(r#"srd.window.scratchpad_show()"#).exec().unwrap();
        assert!(!wm.borrow().window(id).unwrap().minimized);
        assert_eq!(wm.borrow().focused_id(), Some(id));
    }

    #[test]
    fn srd_rule_title_regex_matches_a_specific_dialog_not_the_main_window() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.rule({ title_regex = "^Save File$" }, { floating = true })"#).exec().unwrap();
        let dialog = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "Save File"));
            id
        };
        let main = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            wm.add_window(srdwm_core::Window::new(id, "Save File - GIMP"));
            id
        };
        assert!(wm.borrow().is_floating(dialog));
        assert!(!wm.borrow().is_floating(main));
    }

    #[test]
    fn srd_rule_instance_matches_independently_of_class() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        engine.lua.load(r#"srd.rule({ instance = "firefox" }, { pinned = true })"#).exec().unwrap();
        let id = {
            let mut wm = wm.borrow_mut();
            let id = wm.alloc_window_id();
            let mut w = srdwm_core::Window::new(id, "Mozilla Firefox");
            w.app_id = "Navigator".into();
            w.instance = "firefox".into();
            wm.add_window(w);
            id
        };
        assert!(wm.borrow().window(id).unwrap().always_on_top);
    }

    #[test]
    fn srd_rule_rejects_an_invalid_regex_with_a_lua_error() {
        let dir = tempfile::tempdir().unwrap();
        let wm = Rc::new(RefCell::new(WindowManager::new()));
        let engine = Engine::new(wm.clone(), dir.path()).unwrap();
        let result = engine.lua.load(r#"srd.rule({ title_regex = "(unclosed" }, { floating = true })"#).exec();
        assert!(result.is_err());
    }

    #[test]
    fn load_init_runs_the_users_init_lua() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("init.lua"), r#"srd.set("general.window_gap", 4)"#).unwrap();
        let engine = engine_in(dir.path());
        engine.load_init().unwrap();
        assert_eq!(engine.get("general.window_gap"), Some(ConfigValue::Number(4.0)));
    }

    #[test]
    fn bind_repeat_registers_the_binding_and_marks_it_repeating() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(r#"
                srd.bind("Mod4+a", function() end)
                srd.bind_repeat("XF86AudioRaiseVolume", function() end)
            "#)
            .exec()
            .unwrap();

        let bound = engine.bound_keys();
        // A repeating bind is still a normal binding - it must be grabbed
        // and dispatched like any other, or it would never fire at all.
        assert!(bound.contains(&"Mod4+a".to_string()));
        assert!(bound.contains(&"XF86AudioRaiseVolume".to_string()));

        let repeat = engine.repeat_keys();
        assert_eq!(repeat, vec!["XF86AudioRaiseVolume".to_string()]);
        assert!(!repeat.contains(&"Mod4+a".to_string()), "a plain bind must not repeat");
    }

    #[test]
    fn bind_repeat_dispatches_like_a_normal_binding() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_in(dir.path());
        engine
            .lua
            .load(r#"
                fired = 0
                srd.bind_repeat("Mod4+z", function() fired = fired + 1 end)
            "#)
            .exec()
            .unwrap();
        assert!(engine.dispatch_keybinding("Mod4+z"));
        assert!(engine.dispatch_keybinding("Mod4+z"));
        let fired: i64 = engine.lua.globals().get("fired").unwrap();
        assert_eq!(fired, 2);
    }
