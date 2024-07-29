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
    /// maximized = true, opacity = 0.9 })`. At least one matcher field is
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
                    pinned: actions.get("pinned")?,
                    opacity: actions.get("opacity")?,
                },
            };
            state.borrow().wm.borrow_mut().add_rule(rule);
            Ok(())
        })?)
    }

    pub(super) fn fn_load(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, module: String| {
            let dir = state.borrow().config_dir.clone();
            let path = dir.join(format!("{module}.lua"));
            let src = std::fs::read_to_string(&path)
                .map_err(|e| mlua::Error::RuntimeError(format!("srd.load('{module}'): {e} ({})", path.display())))?;
            lua.load(&src).set_name(path.to_string_lossy().as_ref()).exec()?;
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
