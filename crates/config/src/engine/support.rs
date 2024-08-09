use super::*;

/// Shared between [`Engine::reload`] and `srd.reload()`'s Lua closure --
/// the closure can't capture `&Engine` itself (it isn't `Clone`/`Rc`, and
/// `mlua::Lua::create_function` needs a `'static` closure), so both go
/// through cloned `Lua`/state handles instead of one calling the other.
pub(super) fn do_reload(lua: &Lua, state: &Rc<RefCell<SharedState>>) -> Result<()> {
    let config_dir = {
        let mut s = state.borrow_mut();
        s.key_bindings.clear();
        s.event_handlers.clear();
        s.repeat_keys.clear();
        s.config_dir.clone()
    };
    let path = config_dir.join("init.lua");
    let src = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io { path: path.clone(), source })?;
    lua.load(&src).set_name(path.to_string_lossy().as_ref()).exec()?;
    Ok(())
}

/// Shared by `srd.window.focus` and `srd.window.move` so both accept
/// exactly the same direction names and report the same error.
pub(super) fn parse_direction(name: &str, caller: &str) -> mlua::Result<Direction> {
    match name {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        other => Err(mlua::Error::RuntimeError(format!("{caller}: unknown direction '{other}'"))),
    }
}

#[derive(Clone, Copy)]
pub(super) enum WindowAction {
    Close,
    Minimize,
    Maximize,
    Fullscreen,
    ToggleFloating,
    TogglePin,
    ScratchpadAdd,
}

/// Recursively flattens a Lua table into dotted config keys, e.g.
/// `{border = {width = 2}}` under prefix `"theme.decorations"` becomes
/// `theme.decorations.border.width = 2`. Matches how `docs/DEFAULTS.md`
/// documents nested `srd.theme.set_decorations{...}` / `srd.layout.configure`
/// tables.
pub(super) fn flatten_table_into(prefix: &str, table: &Table, out: &mut HashMap<String, ConfigValue>) -> mlua::Result<()> {
    for pair in table.clone().pairs::<Value, Value>() {
        let (k, v) = pair?;
        let Value::String(k) = k else { continue };
        let key = format!("{prefix}.{}", k.to_str()?);
        if let Value::Table(t) = &v {
            flatten_table_into(&key, t, out)?;
        } else if let Some(cv) = ConfigValue::from_lua(&v) {
            out.insert(key, cv);
        }
    }
    Ok(())
}

/// Checks the numeric ranges, layout-name references, and hex-color strings
/// documented in `docs/DEFAULTS.md`'s "Validation Rules" section against the
/// current config values. Returns a human-readable error per violation.
pub(super) fn validate(s: &SharedState) -> Vec<String> {
    let mut errors = Vec::new();

    let mut check_range = |key: &str, min: f64, max: f64| {
        if let Some(v) = s.values.get(key).and_then(ConfigValue::as_f64) {
            if v < min || v > max {
                errors.push(format!("{key} = {v} is out of range [{min}, {max}]"));
            }
        }
    };
    check_range("general.window_gap", 0.0, 100.0);
    check_range("layout.tiling.gaps.inner", 0.0, 100.0);
    check_range("layout.tiling.gaps.outer", 0.0, 100.0);
    check_range("layout.dynamic.gaps.inner", 0.0, 100.0);
    check_range("layout.dynamic.gaps.outer", 0.0, 100.0);
    check_range("layout.floating.gaps.inner", 0.0, 100.0);
    check_range("layout.floating.gaps.outer", 0.0, 100.0);
    check_range("general.border_width", 0.0, 20.0);
    check_range("theme.decorations.border.width", 0.0, 20.0);
    check_range("general.animation_duration", 0.0, 1000.0);
    check_range("general.resize_margin", 1.0, 50.0);
    check_range("performance.max_fps", 30.0, 240.0);
    check_range("performance.window_cache_size", 10.0, 10000.0);

    let layouts: Vec<String> = s.wm.borrow().available_layouts().iter().map(|l| l.to_string()).collect();
    for key in ["general.default_layout", "monitor.primary_layout", "monitor.secondary_layout"] {
        if let Some(name) = s.values.get(key).and_then(ConfigValue::as_str) {
            if !layouts.iter().any(|l| l == name) {
                errors.push(format!("{key} = '{name}' is not a registered layout {layouts:?}"));
            }
        }
    }

    let color_keys = [
        "theme.colors.background",
        "theme.colors.foreground",
        "theme.colors.primary",
        "theme.colors.secondary",
        "theme.colors.accent",
        "theme.colors.error",
        "theme.colors.warning",
        "theme.colors.success",
        "theme.decorations.border.active_color",
        "theme.decorations.border.inactive_color",
        "theme.decorations.title_bar.background",
        "theme.decorations.title_bar.foreground",
    ];
    for key in color_keys {
        if let Some(v) = s.values.get(key).and_then(ConfigValue::as_str) {
            if !is_valid_hex_color(v) {
                errors.push(format!("{key} = '{v}' is not a valid hex color (expected '#rrggbb')"));
            }
        }
    }

    errors
}

fn is_valid_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// The config surface documented in `docs/DEFAULTS.md`, seeded before
/// `init.lua` runs so `srd.get(...)` returns sensible values even for keys
/// the user's config never touches.
pub(super) fn default_config() -> HashMap<String, ConfigValue> {
    use ConfigValue::*;
    let mut m = HashMap::new();
    let mut set = |k: &str, v: ConfigValue| {
        m.insert(k.to_string(), v);
    };
    set("general.default_layout", String("dynamic".into()));
    set("general.smart_placement", Bool(true));
    set("general.window_gap", Number(8.0));
    set("general.border_width", Number(2.0));
    set("general.animations", Bool(true));
    set("general.animation_duration", Number(200.0));
    set("general.shadows", Bool(true));
    set("general.resize_margin", Number(6.0));
    // Deliberately *not* seeded here, unlike every other `general.*` key --
    // its actual default differs by backend (GLES/winit: on; udev/Pixman:
    // off, an untested-on-real-hardware CPU cost too real to default to on
    // - see `crates/wayland/src/rounded_corners.rs`), and neither backend
    // is known yet at the point `default_config` runs. Leaving the key
    // genuinely absent (rather than pre-seeded `true`/`false`) is what lets
    // `main.rs`'s `apply_general_settings` tell "user never touched this"
    // apart from "user explicitly chose a value" and hand the *unset* case
    // to whichever backend ends up connecting instead of deciding for it.
    set("general.focus_follows_mouse", Bool(false));
    set("general.mouse_follows_focus", Bool(true));
    set("general.auto_raise", Bool(false));
    set("general.auto_focus", Bool(true));

    set("monitor.primary_layout", String("dynamic".into()));
    set("monitor.secondary_layout", String("tiling".into()));
    set("monitor.auto_detect", Bool(true));
    // Deliberately *not* seeded, unlike everything else here: srdwm has one
    // flat workspace list shared by every monitor (see WindowManager's
    // `current_workspace` doc comment), not Hyprland-style independent
    // per-monitor workspace sets - "this monitor's primary workspace" and
    // "this monitor's workspace count" describe a design that doesn't
    // exist. `workspace.count` is the one knob that actually does anything.

    set("window.focus_follows_mouse", Bool(false));
    set("window.mouse_follows_focus", Bool(true));
    set("window.auto_raise", Bool(false));
    set("window.auto_focus", Bool(true));
    set("window.raise_on_focus", Bool(true));
    set("window.remember_position", Bool(true));
    set("window.remember_size", Bool(true));
    set("window.remember_state", Bool(true));

    set("workspace.count", Number(10.0));
    set("workspace.names", List(["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"].map(|s| s.to_string()).to_vec()));
    // `auto_switch` (jump to a new window's workspace when a rule places it
    // elsewhere) and `persistent` (workspace state surviving a restart) are
    // deliberately *not* seeded here: neither is implemented, and a key
    // that's accepted and silently does nothing is worse than one that
    // doesn't exist - see the same reasoning on `general.rounded_corners`'
    // absence from this function, though that one differs by backend
    // rather than being simply unbuilt.
    set("workspace.auto_back_and_forth", Bool(false));

    set("performance.vsync", Bool(true));
    set("performance.max_fps", Number(60.0));
    set("performance.window_cache_size", Number(100.0));
    set("performance.event_queue_size", Number(1000.0));
    set("performance.layout_timeout", Number(16.0));
    set("performance.enable_caching", Bool(true));

    set("debug.logging", Bool(true));
    set("debug.log_level", String("info".into()));
    set("debug.profile", Bool(false));
    set("debug.trace_events", Bool(false));
    set("debug.show_layout_bounds", Bool(false));
    set("debug.show_window_geometry", Bool(false));

    set("layout.tiling.split_ratio", Number(0.5));
    set("layout.tiling.master_ratio", Number(0.6));
    set("layout.tiling.auto_swap", Bool(true));
    set("layout.tiling.gaps.inner", Number(8.0));
    set("layout.tiling.gaps.outer", Number(16.0));
    set("layout.tiling.behavior.new_window_master", Bool(false));
    set("layout.tiling.behavior.auto_balance", Bool(true));
    set("layout.tiling.behavior.preserve_ratio", Bool(true));

    set("layout.dynamic.snap_threshold", Number(20.0));
    set("layout.dynamic.grid_size", Number(6.0));
    set("layout.dynamic.cascade_offset", Number(30.0));
    set("layout.dynamic.smart_placement", Bool(true));
    set("layout.dynamic.gaps.inner", Number(8.0));
    set("layout.dynamic.gaps.outer", Number(16.0));
    set("layout.dynamic.behavior.remember_positions", Bool(true));
    set("layout.dynamic.behavior.auto_arrange", Bool(true));
    set("layout.dynamic.behavior.overlap_prevention", Bool(true));

    set("layout.floating.default_position", String("center".into()));
    set("layout.floating.remember_position", Bool(true));
    set("layout.floating.always_on_top", Bool(false));
    set("layout.floating.gaps.inner", Number(0.0));
    set("layout.floating.gaps.outer", Number(16.0));
    set("layout.floating.behavior.allow_resize", Bool(true));
    set("layout.floating.behavior.allow_move", Bool(true));
    set("layout.floating.behavior.snap_to_edges", Bool(true));

    set("theme.colors.background", String("#2e3440".into()));
    set("theme.colors.foreground", String("#eceff4".into()));
    set("theme.colors.primary", String("#88c0d0".into()));
    set("theme.colors.secondary", String("#81a1c1".into()));
    set("theme.colors.accent", String("#5e81ac".into()));
    set("theme.colors.error", String("#bf616a".into()));
    set("theme.colors.warning", String("#ebcb8b".into()));
    set("theme.colors.success", String("#a3be8c".into()));

    set("theme.decorations.border.width", Number(2.0));
    set("theme.decorations.border.active_color", String("#88c0d0".into()));
    set("theme.decorations.border.inactive_color", String("#2e3440".into()));
    set("theme.decorations.border.focused_style", String("solid".into()));
    set("theme.decorations.border.unfocused_style", String("solid".into()));
    set("theme.decorations.title_bar.height", Number(24.0));
    set("theme.decorations.title_bar.show", Bool(true));
    set("theme.decorations.title_bar.font", String("JetBrains Mono 10".into()));
    set("theme.decorations.title_bar.background", String("#2e3440".into()));
    set("theme.decorations.title_bar.foreground", String("#eceff4".into()));

    set("platform.backend", String("auto".into()));
    set("platform.x11.use_ewmh", Bool(true));
    set("platform.x11.use_netwm", Bool(true));
    set("platform.wayland.use_xdg_shell", Bool(true));
    set("platform.wayland.use_layer_shell", Bool(true));
    set("platform.windows.use_dwm", Bool(true));
    set("platform.windows.use_win32", Bool(true));
    set("platform.windows.global_hooks", Bool(true));
    set("platform.macos.use_cocoa", Bool(true));
    set("platform.macos.use_core_graphics", Bool(true));
    set("platform.macos.accessibility_enabled", Bool(true));

    m
}
