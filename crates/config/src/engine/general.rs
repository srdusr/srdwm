use super::*;
use super::support::{default_config, do_reload, validate};

impl Engine {
    // ---- srd.* -----------------------------------------------------------

    pub(super) fn fn_set(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (key, value): (String, Value)| {
            if let Some(v) = ConfigValue::from_lua(&value) {
                state.borrow_mut().values.insert(key, v);
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_get(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, key: String| {
            let v = state.borrow().values.get(&key).cloned();
            Ok(match v {
                Some(ConfigValue::String(s)) => Value::String(lua.create_string(&s)?),
                Some(ConfigValue::Number(n)) => Value::Number(n),
                Some(ConfigValue::Bool(b)) => Value::Boolean(b),
                Some(ConfigValue::List(items)) => Value::Table(lua.create_sequence_from(items)?),
                None => Value::Nil,
            })
        })?)
    }

    pub(super) fn fn_reset(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, key: String| {
            let defaults = default_config();
            let mut s = state.borrow_mut();
            match defaults.get(&key) {
                Some(v) => {
                    s.values.insert(key, v.clone());
                }
                None => {
                    s.values.remove(&key);
                }
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_reset_all(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            state.borrow_mut().values = default_config();
            Ok(())
        })?)
    }

    pub(super) fn fn_reset_category(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, category: String| {
            let defaults = default_config();
            let mut s = state.borrow_mut();
            let prefix = format!("{category}.");
            s.values.retain(|k, _| !k.starts_with(&prefix));
            for (k, v) in defaults.into_iter().filter(|(k, _)| k.starts_with(&prefix)) {
                s.values.insert(k, v);
            }
            Ok(())
        })?)
    }

    /// `srd.on("lid_closed", function() ... end)` - registers a handler for
    /// a non-key event. `"ready"` fires once, after the platform backend has
    /// connected (real Wayland/X11 display available, `WAYLAND_DISPLAY`/
    /// `DISPLAY` set for anything `srd.spawn`ed from the handler to inherit)
    /// - see `main.rs`. Config that starts background processes (a bar,
    /// wallpaper daemon, clipboard watcher) belongs in a `"ready"` handler,
    /// not at a config file's top level: top-level code runs during
    /// `load_init`, which is *before* the platform connects, so anything
    /// spawned there inherits no display socket to connect to at all.
    pub(super) fn fn_on(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, (name, f): (String, mlua::Function)| {
            const KNOWN: [&str; 3] = ["lid_closed", "lid_open", "ready"];
            if !KNOWN.contains(&name.as_str()) {
                return Err(mlua::Error::RuntimeError(format!(
                    "srd.on: unknown event '{name}' (known: {})",
                    KNOWN.join(", ")
                )));
            }
            let key = lua.create_registry_value(f)?;
            state.borrow_mut().event_handlers.insert(name, key);
            Ok(())
        })?)
    }

    /// `srd.bind_repeat(combo, fn)` - like `srd.bind`, but keeps firing
    /// while the key is held (Hyprland's `binde`). For volume, brightness
    /// and window-switcher cycling, where one step per press is unusable.
    pub(super) fn fn_bind_repeat(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, (combo, f): (String, mlua::Function)| {
            let combo = srdwm_core::canonicalize_key_combo(&combo);
            let key = lua.create_registry_value(f)?;
            let mut s = state.borrow_mut();
            s.repeat_keys.insert(combo.clone());
            s.key_bindings.insert(combo, key);
            Ok(())
        })?)
    }

    pub(super) fn fn_bind(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, (combo, f): (String, mlua::Function)| {
            // `key_bindings` is keyed by whatever string dispatch builds
            // from a real keypress (`srdwm_core::key_combo_string`, fixed
            // Ctrl/Shift/Alt/Mod4 order) - storing the config's own
            // literal string here (usually written Super-first,
            // "Mod4+Shift+x") meant multi-modifier bindings could never be
            // found at dispatch time even though the raw key was correctly
            // grabbed/intercepted. See `parse_key_combo`'s doc comment.
            let combo = srdwm_core::canonicalize_key_combo(&combo);
            let key = lua.create_registry_value(f)?;
            state.borrow_mut().key_bindings.insert(combo, key);
            Ok(())
        })?)
    }

    /// `srd.rule({ title = "...", class = "...", title_regex = "...",
    /// class_regex = "...", instance = "..." }, { floating = true,
    /// workspace = 2, x = .., y = .., width = .., height = ..,
    /// decorated = false, border_color = {r,g,b}, border_width = 2,
    /// corner_radius = 10, maximized = true, opacity = 0.9,
    /// aspect_ratio = "9:16" })`. At least one matcher field is
    /// required; unmatched rules apply nothing.
    ///
    /// `title`/`class` are plain substring/exact match, cheap and cover
    /// most rules with no regex syntax to get right. `title_regex`/
    /// `class_regex` (Rust `regex` crate syntax, case-sensitive unless the
    /// pattern starts with `(?i)`) and `instance` (X11 `WM_CLASS`'s
    /// instance half, matched exactly - see `srdwm_core::WindowMatch`'s
    /// doc comment) exist for the cases that need more precision, e.g.
    /// disambiguating a specific dialog by title while leaving an app's
    /// main window alone. Every field given is ANDed together.
    pub(super) fn fn_rule(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (matcher, actions): (Table, Table)| {
            let title_contains: Option<String> = matcher.get("title")?;
            let class: Option<String> = match matcher.get("class")? {
                Some(c) => Some(c),
                None => matcher.get("app_id")?,
            };
            let instance: Option<String> = matcher.get("instance")?;
            let title_regex = match matcher.get::<_, Option<String>>("title_regex")? {
                Some(pat) => Some(srdwm_core::Regex::new(&pat).map_err(|e| mlua::Error::RuntimeError(format!("srd.rule: invalid title_regex '{pat}': {e}")))?),
                None => None,
            };
            let class_regex = match matcher.get::<_, Option<String>>("class_regex")? {
                Some(pat) => Some(srdwm_core::Regex::new(&pat).map_err(|e| mlua::Error::RuntimeError(format!("srd.rule: invalid class_regex '{pat}': {e}")))?),
                None => None,
            };

            let border_color: Option<(u8, u8, u8)> = match actions.get::<_, Option<Table>>("border_color")? {
                Some(t) => Some((t.get(1)?, t.get(2)?, t.get(3)?)),
                None => None,
            };
            let geometry: Option<Rect> = {
                let x: Option<i32> = actions.get("x")?;
                let y: Option<i32> = actions.get("y")?;
                let width: Option<u32> = actions.get("width")?;
                let height: Option<u32> = actions.get("height")?;
                match (x, y, width, height) {
                    (Some(x), Some(y), Some(width), Some(height)) => Some(Rect::new(x, y, width, height)),
                    _ => None,
                }
            };
            // `aspect_ratio = "9:16"` - the "phone monitor / special
            // workspace" ask's own real, scoped answer (see `Window::
            // aspect_ratio`'s own doc comment in `crates/core`): a rule
            // matching any VM/emulator/`scrcpy` window by `app_id` keeps
            // it phone-shaped through a resize, with no Android-specific
            // (or even VM-specific) code anywhere in this compositor.
            // `"W:H"` (a plain string, not a table) matches this
            // project's own `border_color = {r,g,b}` precedent for "a
            // structured value needs its own small parse", just with a
            // string instead of a table since a ratio is conventionally
            // written that way everywhere (`16:9`, `9:16`, `4:3`).
            let aspect_ratio: Option<(u32, u32)> = match actions.get::<_, Option<String>>("aspect_ratio")? {
                Some(spec) => {
                    let (w, h) = spec
                        .split_once(':')
                        .and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)))
                        .filter(|(w, h)| *w > 0 && *h > 0)
                        .ok_or_else(|| mlua::Error::RuntimeError(format!("srd.rule: aspect_ratio must be \"W:H\" with positive integers, got {spec:?}")))?;
                    Some((w, h))
                }
                None => None,
            };

            let rule = WindowRule {
                matcher: WindowMatch { title_contains, class, title_regex, class_regex, instance },
                actions: WindowRuleActions {
                    floating: actions.get("floating")?,
                    maximized: actions.get("maximized")?,
                    workspace: actions.get("workspace")?,
                    geometry,
                    decorated: actions.get("decorated")?,
                    border_color,
                    border_width: actions.get("border_width")?,
                    corner_radius: actions.get("corner_radius")?,
                    pinned: actions.get("pinned")?,
                    opacity: actions.get("opacity")?,
                    resize_margin: actions.get("resize_margin")?,
                    aspect_ratio,
                },
            };
            state.borrow().wm.borrow_mut().add_rule(rule);
            Ok(())
        })?)
    }

    /// `srd.monitor.split(name, parts[, direction])` - divides connector
    /// `name`'s real output into `parts` equal logical monitors for
    /// placement/tiling purposes ("monitors inside monitors"), no new
    /// `wl_output` involved - see `srdwm_core::monitor::MonitorSplit`'s
    /// own doc comment for exactly what that does and doesn't give a
    /// client. `direction` is `"columns"` (default, side-by-side) or
    /// `"rows"` (stacked); any other value is treated as `"columns"`
    /// rather than erroring, same "malformed value falls back to a
    /// sensible default" stance other config setters already take.
    /// `parts <= 1` clears an existing split for `name`.
    pub(super) fn fn_monitor_split(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (name, parts, direction): (String, u32, Option<String>)| {
            let rows = matches!(direction.as_deref(), Some("rows"));
            state.borrow().wm.borrow_mut().set_monitor_split(name, parts, rows);
            Ok(())
        })?)
    }

    /// `srd.monitor.scale(name, factor)` - sets connector `name`'s
    /// output scale, applied the next time a backend brings that head up
    /// (startup, hotplug, or re-enable). A physically large monitor with
    /// the same pixel count as a smaller one (a big low-DPI external
    /// display next to a small high-DPI laptop panel, say) can run below
    /// `1.0` to show more logical desktop space rather than just larger
    /// text at the same resolution. `factor <= 0` clears an existing
    /// override.
    pub(super) fn fn_monitor_scale(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (name, factor): (String, f64)| {
            state.borrow().wm.borrow_mut().set_monitor_scale(name, factor);
            Ok(())
        })?)
    }

    /// `srd.load("keybindings")`/`"themes"`/`"rules"`/`"startup"`: each is
    /// its own file, its own logical concern, and - deliberately, since
    /// this function catches its own execution error rather than letting
    /// `?` propagate one - its own failure domain. A `mlua::Error` from
    /// one module used to unwind straight out through this function and
    /// back into whatever was running `init.lua` itself, aborting every
    /// statement after that `srd.load` call, including every *other*
    /// `srd.load` - a typo in `rules.lua` silently took `startup.lua`
    /// (autostart) down with it, and there was no way from the config
    /// author's side to prevent that, short of never making a mistake.
    /// Reproduced live in the worse but related case (the error was in
    /// `init.lua` itself, above every `srd.load` call, which this
    /// function alone can't isolate against): a session that started with
    /// nothing but a bare cursor, zero autostart, zero keybindings, no
    /// error visible anywhere except one `WARN` line in a multi-hundred-
    /// megabyte log file. A module failing now still leaves the user
    /// without whatever that module would have set up, logged clearly at
    /// `error` level with the module name and the file path - but
    /// everything *else* `init.lua` goes on to load still does.
    pub(super) fn fn_load(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, module: String| {
            let dir = state.borrow().config_dir.clone();
            let path = dir.join(format!("{module}.lua"));
            let src = match std::fs::read_to_string(&path) {
                Ok(src) => src,
                Err(e) => {
                    log::error!("srd.load('{module}'): {e} ({}) - this module did not load, but the rest of init.lua still will", path.display());
                    return Ok(());
                }
            };
            if let Err(e) = lua.load(&src).set_name(path.to_string_lossy().as_ref()).exec() {
                log::error!("srd.load('{module}'): {e} - this module did not finish loading, but the rest of init.lua still will");
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_spawn(&self) -> Result<mlua::Function<'_>> {
        Ok(self.lua.create_function(move |_, command: String| {
            #[cfg(unix)]
            let result = std::process::Command::new("sh").arg("-c").arg(&command).spawn();
            #[cfg(windows)]
            let result = std::process::Command::new("cmd").arg("/C").arg(&command).spawn();
            if let Err(e) = result {
                log::warn!("srd.spawn('{command}') failed: {e}");
            }
            Ok(())
        })?)
    }

    /// Sets an environment variable in srdwm's own process - `std::process::
    /// Command` inherits the parent's environment by default (`fn_spawn`
    /// above never overrides that), so anything set here is visible to
    /// every process `srd.spawn` starts from this point on, and to their own
    /// children in turn. Hyprland's `env = NAME,VALUE` works the same way
    /// (sets it in its own process before forking anything), which is the
    /// mechanism a ported `env.conf` needs - there is no per-spawn
    /// equivalent that would let one client see a variable no other client
    /// does, so this is deliberately global and process-wide, not scoped.
    pub(super) fn fn_setenv(&self) -> Result<mlua::Function<'_>> {
        Ok(self.lua.create_function(move |_, (name, value): (String, String)| {
            std::env::set_var(&name, &value);
            Ok(())
        })?)
    }

    pub(super) fn fn_quit(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            state.borrow().running.set(false);
            Ok(())
        })?)
    }

    pub(super) fn fn_reload(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        // `create_function`'s closure is handed the `&Lua` it's being
        // called from as its first argument - used directly here instead
        // of capturing a cloned handle, since `mlua::Lua` isn't `Clone` in
        // this version.
        Ok(self.lua.create_function(move |lua, ()| {
            match do_reload(lua, &state) {
                Ok(()) => log::info!("srd.reload: config reloaded"),
                Err(e) => log::error!("srd.reload: {e}"),
            }
            Ok(())
        })?)
    }

    pub(super) fn fn_notify(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (message, level): (String, Option<String>)| {
            let level = level.unwrap_or_else(|| "info".to_string());
            #[cfg(unix)]
            {
                let sent = std::process::Command::new("notify-send").arg("srdwm").arg(&message).status();
                if sent.map(|s| !s.success()).unwrap_or(true) {
                    log::info!("[{level}] {message}");
                }
            }
            #[cfg(not(unix))]
            {
                log::info!("[{level}] {message}");
            }
            state.borrow_mut().log.push(format!("[{level}] {message}"));
            Ok(())
        })?)
    }

    /// Checks the numeric/string ranges documented in `docs/DEFAULTS.md`'s
    /// "Validation Rules" section. Returns `(ok, errors)`; `errors` is an
    /// empty table when `ok` is true.
    pub(super) fn fn_validate_config(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, ()| {
            let s = state.borrow();
            let errors = validate(&s);
            let ok = errors.is_empty();
            Ok((ok, lua.create_sequence_from(errors)?))
        })?)
    }

    pub(super) fn fn_debug_config_status(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, ()| {
            let s = state.borrow();
            let t = lua.create_table()?;
            t.set("keys", s.values.len())?;
            t.set("bound_keys", s.key_bindings.len())?;
            t.set("log_entries", s.log.len())?;
            t.set("config_dir", s.config_dir.to_string_lossy().into_owned())?;
            Ok(t)
        })?)
    }

    pub(super) fn fn_debug_show_settings(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, ()| {
            let s = state.borrow();
            let mut keys: Vec<&String> = s.values.keys().collect();
            keys.sort();
            let t = lua.create_table()?;
            for key in keys {
                let v = &s.values[key];
                log::info!("{key} = {v:?}");
                let lua_v = match v {
                    ConfigValue::String(s) => Value::String(lua.create_string(s)?),
                    ConfigValue::Number(n) => Value::Number(*n),
                    ConfigValue::Bool(b) => Value::Boolean(*b),
                    ConfigValue::List(items) => Value::Table(lua.create_sequence_from(items.clone())?),
                };
                t.set(key.as_str(), lua_v)?;
            }
            Ok(t)
        })?)
    }

    pub(super) fn fn_debug_profile_start(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            state.borrow_mut().profile_start = Some(std::time::Instant::now());
            Ok(())
        })?)
    }

    pub(super) fn fn_debug_profile_stop(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let elapsed = state.borrow_mut().profile_start.take().map(|t| t.elapsed().as_secs_f64());
            if let Some(secs) = elapsed {
                log::info!("profile: {:.3}ms", secs * 1000.0);
            }
            Ok(elapsed)
        })?)
    }
}
