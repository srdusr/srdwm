use super::*;

/// Shared between [`Engine::reload`] and `srd.reload()`'s Lua closure --
/// the closure can't capture `&Engine` itself (it isn't `Clone`/`Rc`, and
/// `mlua::Lua::create_function` needs a `'static` closure), so both go
/// through cloned `Lua`/state handles instead of one calling the other.
/// Re-executes `init.lua`, and puts the previous config back if that fails.
///
/// The clear-then-execute order is required: a binding or handler deleted
/// from the edited file has to actually disappear, which only clearing
/// first achieves. The bug was that nothing ever undid the clear. A Lua
/// syntax error - the single most likely thing to go wrong with a
/// programmable config, and the thing a user is most likely to do by
/// accident - left the compositor with **no keybindings at all**: not the
/// old ones, not the new ones. The only key still working was the hardcoded
/// reload combo `main.rs` handles before consulting Lua, which is the one
/// key nobody thinks to press when their config has just stopped working,
/// because nothing tells them that is the situation.
///
/// Now the three maps are moved out rather than cleared, and moved back on
/// any failure, so a broken edit leaves the last *working* config running.
/// That is the behaviour every mainstream programmable config has (tmux,
/// Neovim, Hyprland): a bad reload is a no-op with an error, not a
/// half-applied state.
///
/// Answers "what happens when our config fails/user does something wrong
/// which can be expected since lua programmable config" - asked directly,
/// and previously answered by the code with "you lose every keybinding".
pub(super) fn do_reload(lua: &Lua, state: &Rc<RefCell<SharedState>>) -> Result<()> {
    let (config_dir, previous) = {
        let mut s = state.borrow_mut();
        let previous = (
            std::mem::take(&mut s.key_bindings),
            std::mem::take(&mut s.event_handlers),
            std::mem::take(&mut s.repeat_keys),
        );
        (s.config_dir.clone(), previous)
    };
    let restore = |state: &Rc<RefCell<SharedState>>, previous: (_, _, _)| {
        let mut s = state.borrow_mut();
        // Whatever the failed run managed to register before erroring is
        // discarded, not merged: half of a broken config is not a config.
        s.key_bindings = previous.0;
        s.event_handlers = previous.1;
        s.repeat_keys = previous.2;
    };
    let path = config_dir.join("init.lua");
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(source) => {
            restore(state, previous);
            return Err(ConfigError::Io { path, source });
        }
    };
    if let Err(e) = lua.load(&src).set_name(path.to_string_lossy().as_ref()).exec() {
        restore(state, previous);
        return Err(e.into());
    }
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
    check_range("theme.decorations.border.width", 0.0, 20.0);
    check_range("theme.decorations.border.radius", 0.0, 100.0);
    check_range("general.animation_duration", 0.0, 1000.0);
    check_range("general.resize_margin", 1.0, 50.0);
    check_range("performance.max_fps", 30.0, 240.0);
    check_range("performance.window_cache_size", 10.0, 10000.0);
    check_range("theme.decorations.border.inactive_dim", 0.0, 1.0);

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
        "theme.decorations.title_bar.foreground_focused",
        "theme.decorations.title_bar.foreground_unfocused",
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
    // Re-read `init.lua` when it changes on disk, no reload key needed.
    // See `main.rs`'s `config_mtime`/`CONFIG_POLL_INTERVAL`.
    set("general.config_reload_on_write", Bool(true));
    // Maximize runs to the bottom of the screen, under a dock, rather than
    // stopping above it. A top bar is still always honoured.
    set("general.maximize_covers_dock", Bool(true));
    set("general.smart_placement", Bool(true));
    set("general.window_gap", Number(8.0));
    set("general.animations", Bool(true));
    set("general.animation_duration", Number(200.0));
    set("general.shadows", Bool(true));
    set("general.resize_margin", Number(6.0));
    // `false`: real GBM+EGL+`DrmCompositor` GPU rendering on the udev
    // backend is opt-in and still missing real window content/decoration
    // support (see `crates/wayland/src/udev/gpu.rs`'s own module doc
    // comment for exactly what it does render) - unlike `general.
    // rounded_corners` just above, this has one unambiguous default
    // regardless of which backend ends up connecting (GPU rendering is
    // udev-only and experimental everywhere), so it's seeded here like
    // every other ordinary flag rather than left absent for a backend to
    // decide.
    set("general.gpu", Bool(false));
    // `false`: opt-in single-app-at-a-time placement, off by default so
    // an ordinary desktop session's floating/tiling behavior is
    // completely unaffected - see `WindowManager::phone_mode`'s own doc
    // comment.
    set("general.phone_mode", Bool(false));
    // `false`: an extra cursor sprite per other physical pointer device is
    // opt-in, not automatic - see `WindowManager::multi_cursor_enabled`'s
    // own doc comment for the real, reported reason (a phantom libinput
    // device from otherwise-ordinary hardware showing up as an
    // uncontrollable frozen ghost cursor).
    set("general.multi_cursor", Bool(false));
    // Real desktop icons (Home/Computer/Trash plus `~/Desktop`'s own
    // contents) on by default - see `WindowManager::desktop_icons_
    // enabled`'s own doc comment for why this, unlike `general.gpu` just
    // above, doesn't need an opt-in safety net.
    set("general.desktop_icons", Bool(true));
    // On by default - see `WindowManager::desktop_icons_all_monitors`'s
    // own doc comment.
    set("general.desktop_icons_all_monitors", Bool(true));
    // `0` (no static reservation) by default - see `WindowManager::
    // reserve_top`'s own doc comment for what this is and why.
    set("general.reserve_top", Number(0.0));
    set("general.reserve_bottom", Number(0.0));
    set("general.reserve_left", Number(0.0));
    set("general.reserve_right", Number(0.0));
    // Empty by default - see `WindowManager::file_manager`'s own doc
    // comment: empty means "dispatch via `xdg-open`", not "no file manager
    // configured, do nothing".
    set("general.file_manager", String(std::string::String::new()));
    // Double-click by default - see `WindowManager::desktop_icon_single_
    // click`'s own doc comment.
    set("general.desktop_icon_single_click", Bool(false));
    // Empty by default - see `WindowManager::terminal`'s own doc comment:
    // empty tries a short list of common terminals on `$PATH`.
    set("general.terminal", String(std::string::String::new()));
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
    set("general.auto_raise", Bool(false));
    // `mouse_follows_focus` (warp the pointer to match a keybinding-driven
    // focus change, the reverse of the two above) and `auto_focus` (no
    // clear distinct meaning found beyond what plain click-to-focus already
    // does) are deliberately not seeded - neither is implemented, same
    // reasoning as `workspace.auto_switch`'s own absence.

    set("monitor.primary_layout", String("dynamic".into()));
    set("monitor.secondary_layout", String("tiling".into()));
    set("monitor.auto_detect", Bool(true));
    // "This monitor's workspace *count*" specifically is still deliberately
    // not seeded/implemented - `workspace.count` is one flat number for
    // the whole desktop, not per-monitor. Independent per-monitor
    // workspace *sets* (which workspace each monitor is showing) is a
    // different, now-real knob: see `workspace.per_monitor` below.

    // The `window.*` namespace this codebase's own `docs/DEFAULTS.md`
    // documented (focus_follows_mouse/mouse_follows_focus/auto_raise/
    // auto_focus/raise_on_focus/remember_position/remember_size/
    // remember_state) was a full, entirely unimplemented duplicate of
    // `general.*`'s own focus keys plus three genuinely unbuilt
    // per-app-window-state-persistence features - removed rather than
    // seeded, same reasoning as everything else in this comment block.
    // `general.focus_follows_mouse`/`general.auto_raise` above are the
    // real, working versions of the one pair that *is* implemented.

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
    // `false`: srdwm's original single-shared-workspace design (switching
    // workspace changes what's visible on every monitor at once) - `true`
    // switches to Hyprland/niri-style independent per-monitor workspace
    // sets, each monitor tracking and displaying its own current
    // workspace. See `WindowManager::per_monitor_workspaces`'s own doc
    // comment.
    set("workspace.per_monitor", Bool(false));

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
    set("theme.decorations.border.radius", Number(6.0));
    set("theme.decorations.border.active_color", String("#88c0d0".into()));
    set("theme.decorations.border.inactive_color", String("#2e3440".into()));
    // The actually-wired unfocused-border knob (`apply_general_settings`,
    // `srdwm_core::ThemeConfig::border_inactive_dim`): a factor applied to
    // `border.active_color`, not the unused absolute `inactive_color`
    // above. `1.0` matches focused exactly; `0.0` fades to black.
    set("theme.decorations.border.inactive_dim", Number(0.35));
    set("theme.decorations.border.focused_style", String("solid".into()));
    set("theme.decorations.border.unfocused_style", String("solid".into()));
    set("theme.decorations.title_bar.height", Number(24.0));
    set("theme.decorations.title_bar.show", Bool(true));
    set("theme.decorations.title_bar.font", String("JetBrains Mono 10".into()));
    set("theme.decorations.title_bar.background", String("#2e3440".into()));
    set("theme.decorations.title_bar.foreground", String("#eceff4".into()));
    // The actually-wired pair (`apply_general_settings`): titlebar text has
    // always used two colours - brighter on the focused window, dimmer on
    // every other one - never the single `foreground` key above.
    set("theme.decorations.title_bar.foreground_focused", String("#88c0d0".into()));
    set("theme.decorations.title_bar.foreground_unfocused", String("#4c566a".into()));

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
