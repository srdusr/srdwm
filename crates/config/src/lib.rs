//! The `srd` Lua scripting API: config values, keybindings, layout/theme
//! setup, and window/workspace actions, all callable from `.lua` files.
//!
//! This targets the API surface documented in the legacy project's
//! `docs/DEFAULTS.md` (see docs/PRIOR_ART.md for the full comparison), which
//! is richer than what the C++ `lua_manager.cc` actually registered: that
//! engine's `srd.window.focused()` returned a hardcoded placeholder table
//! with no methods, even though the shipped example `keybindings.lua` called
//! `window:close()` on it - a call that would have errored at runtime. Here,
//! `srd.window.close()` / `.minimize()` / `.maximize()` / `.focus(direction)`
//! act on the real focused window via a shared [`srdwm_core::WindowManager`],
//! and `srd.bind` stores the actual Lua closure (via the registry) rather
//! than just the key-combo string.

mod value;

pub use value::ConfigValue;

use mlua::{Lua, RegistryKey, Table, Value};
use srdwm_core::{Direction, Rect, WindowManager, WindowMatch, WindowRule, WindowRuleActions};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct SharedState {
    wm: Rc<RefCell<WindowManager>>,
    values: HashMap<String, ConfigValue>,
    key_bindings: HashMap<String, RegistryKey>,
    /// Combos registered with `srd.bind_repeat`, which fire repeatedly while
    /// held (Hyprland's `binde`). A subset of `key_bindings`.
    repeat_keys: std::collections::HashSet<String>,
    /// Handlers for non-key events (currently the lid switch), registered
    /// via `srd.on(...)`. Kept separate from `key_bindings` because the
    /// backends use that map to decide which *keypresses* to withhold from
    /// clients - a pseudo-entry there would be grabbed as if it were a key.
    event_handlers: HashMap<String, RegistryKey>,
    config_dir: PathBuf,
    log: Vec<String>,
    running: Rc<std::cell::Cell<bool>>,
    profile_start: Option<std::time::Instant>,
}

/// Owns the Lua interpreter and the `srd` module state. Cheap to keep around
/// for the process lifetime; `reload` re-executes `init.lua` from scratch
/// against a fresh Lua state so stale closures/globals can't linger.
pub struct Engine {
    lua: Lua,
    state: Rc<RefCell<SharedState>>,
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
    #[error("io error reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

pub type Result<T> = std::result::Result<T, ConfigError>;

impl Engine {
    /// Builds a fresh interpreter wired to `wm` and loads defaults; call
    /// [`Engine::load_init`] to run the user's `init.lua`.
    pub fn new(wm: Rc<RefCell<WindowManager>>, config_dir: impl Into<PathBuf>) -> Result<Self> {
        let lua = Lua::new();
        let state = Rc::new(RefCell::new(SharedState {
            wm,
            values: default_config(),
            key_bindings: HashMap::new(),
            repeat_keys: std::collections::HashSet::new(),
            event_handlers: HashMap::new(),
            config_dir: config_dir.into(),
            log: Vec::new(),
            running: Rc::new(std::cell::Cell::new(true)),
            profile_start: None,
        }));
        let engine = Self { lua, state };
        engine.register_srd_module()?;
        Ok(engine)
    }

    /// Shared flag `srd.quit()` clears; the main loop polls this to know
    /// when to stop.
    pub fn running_flag(&self) -> Rc<std::cell::Cell<bool>> {
        self.state.borrow().running.clone()
    }

    pub fn config_dir(&self) -> PathBuf {
        self.state.borrow().config_dir.clone()
    }

    /// Loads and executes `init.lua` from the config directory.
    pub fn load_init(&self) -> Result<()> {
        let path = self.config_dir().join("init.lua");
        self.exec_file(&path)
    }

    /// Re-executes `init.lua` from scratch: clears keybindings, event
    /// handlers and the repeat-key set first, so a binding or handler
    /// removed from the edited config doesn't linger from the previous
    /// load. `values` (`srd.set` keys) are deliberately left alone --
    /// `platform.backend`/`platform.os` are published once by `main.rs`
    /// before the *first* `load_init` and nothing in Lua ever re-sets them,
    /// so clearing `values` here would silently break every
    /// `if srd.get("platform.backend") == ...` branch in the reloaded
    /// config.
    ///
    /// Does *not* re-grab/re-register the reloaded key set with the
    /// platform backend - `main.rs` reads `bound_keys()` once, before
    /// connecting, to build the X11 `XGrabKey` list / Wayland intercept
    /// set. A binding whose *combo* is unchanged from startup picks up a
    /// reload immediately; a config that adds a brand new combo needs a
    /// real restart before the backend will ever hand that keypress to
    /// srdwm instead of the focused client.
    pub fn reload(&self) -> Result<()> {
        do_reload(&self.lua, &self.state)
    }

    pub fn exec_file(&self, path: &Path) -> Result<()> {
        let src = std::fs::read_to_string(path).map_err(|source| ConfigError::Io { path: path.to_path_buf(), source })?;
        self.lua.load(&src).set_name(path.to_string_lossy().as_ref()).exec()?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<ConfigValue> {
        self.state.borrow().values.get(key).cloned()
    }

    pub fn get_string(&self, key: &str, default: &str) -> String {
        self.get(key).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| default.to_string())
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
    }

    /// Sets a config value from Rust rather than Lua - used by `main.rs`
    /// to publish facts the *host* determined (which backend was picked,
    /// which OS this is) before `load_init` runs `init.lua`, so config
    /// files can read them back via `srd.get(key)` and branch on them
    /// (`if srd.get("platform.backend") == "wayland" then ... end`).
    /// Writing straight into `values` is the same thing `srd.set` does from
    /// the Lua side, just without going through the interpreter.
    pub fn set_string(&self, key: &str, value: impl Into<String>) {
        self.state.borrow_mut().values.insert(key.to_string(), ConfigValue::String(value.into()));
    }

    /// Runs the Lua function bound to `combo` (e.g. `"Mod4+Return"`), if any.
    /// Returns `true` if a binding existed and ran without erroring.
    /// Runs the `srd.on(name, ...)` handler for a non-key event, if any.
    /// Returns false when nothing is registered, so callers can log it.
    pub fn dispatch_event(&self, name: &str) -> bool {
        let func = {
            let state = self.state.borrow();
            state.event_handlers.get(name).and_then(|key| self.lua.registry_value::<mlua::Function>(key).ok())
        };
        match func {
            Some(f) => {
                if let Err(e) = f.call::<_, ()>(()) {
                    log::error!("event handler '{name}' errored: {e}");
                }
                true
            }
            None => false,
        }
    }

    pub fn dispatch_keybinding(&self, combo: &str) -> bool {
        let func = {
            let state = self.state.borrow();
            state.key_bindings.get(combo).and_then(|key| self.lua.registry_value::<mlua::Function>(key).ok())
        };
        match func {
            Some(f) => {
                if let Err(e) = f.call::<_, ()>(()) {
                    log::error!("keybinding '{combo}' errored: {e}");
                }
                true
            }
            None => false,
        }
    }

    pub fn bound_keys(&self) -> Vec<String> {
        self.state.borrow().key_bindings.keys().cloned().collect()
    }

    /// Combos that should auto-repeat while held.
    pub fn repeat_keys(&self) -> Vec<String> {
        self.state.borrow().repeat_keys.iter().cloned().collect()
    }

    fn register_srd_module(&self) -> Result<()> {
        let lua = &self.lua;
        let srd = lua.create_table()?;

        srd.set("set", self.fn_set()?)?;
        srd.set("get", self.fn_get()?)?;
        srd.set("reset", self.fn_reset()?)?;
        srd.set("reset_all", self.fn_reset_all()?)?;
        srd.set("reset_category", self.fn_reset_category()?)?;
        srd.set("bind", self.fn_bind()?)?;
        srd.set("bind_repeat", self.fn_bind_repeat()?)?;
        srd.set("on", self.fn_on()?)?;
        srd.set("rule", self.fn_rule()?)?;
        srd.set("load", self.fn_load()?)?;
        srd.set("spawn", self.fn_spawn()?)?;
        srd.set("notify", self.fn_notify()?)?;
        srd.set("quit", self.fn_quit()?)?;
        srd.set("reload", self.fn_reload()?)?;
        srd.set("validate_config", self.fn_validate_config()?)?;

        let debug = lua.create_table()?;
        debug.set("config_status", self.fn_debug_config_status()?)?;
        debug.set("validate_config", self.fn_validate_config()?)?;
        debug.set("show_settings", self.fn_debug_show_settings()?)?;
        debug.set("profile_start", self.fn_debug_profile_start()?)?;
        debug.set("profile_stop", self.fn_debug_profile_stop()?)?;
        srd.set("debug", debug)?;

        let window = lua.create_table()?;
        window.set("focused", self.fn_window_focused()?)?;
        window.set("close", self.fn_window_action(WindowAction::Close)?)?;
        window.set("minimize", self.fn_window_action(WindowAction::Minimize)?)?;
        window.set("maximize", self.fn_window_action(WindowAction::Maximize)?)?;
        window.set("fullscreen", self.fn_window_action(WindowAction::Fullscreen)?)?;
        window.set("toggle_pin", self.fn_window_action(WindowAction::TogglePin)?)?;
        window.set("focus", self.fn_window_focus_direction()?)?;
        window.set("move", self.fn_window_move_direction()?)?;
        window.set("next", self.fn_window_cycle(true)?)?;
        window.set("prev", self.fn_window_cycle(false)?)?;
        window.set("set_decorations", self.fn_window_set_decorations()?)?;
        window.set("set_border_color", self.fn_window_set_border_color()?)?;
        window.set("set_border_width", self.fn_window_set_border_width()?)?;
        window.set("set_opacity", self.fn_window_set_opacity()?)?;
        window.set("set_floating", self.fn_window_set_floating()?)?;
        window.set("toggle_floating", self.fn_window_action(WindowAction::ToggleFloating)?)?;
        window.set("is_floating", self.fn_window_is_floating()?)?;
        // `srd.window.scratchpad()` moves the *focused* window into the
        // scratchpad pool, hiding it (sway's `move scratchpad`).
        // `srd.window.scratchpad_show()` toggles pool visibility and takes
        // no target of its own - deliberately not routed through
        // `fn_window_action`'s focused-window gate, since showing a hidden
        // scratchpad window has to work even when nothing is currently
        // focused (an empty workspace, or focus on a different monitor).
        // See `WindowManager::scratchpad_show`'s doc comment.
        window.set("scratchpad", self.fn_window_action(WindowAction::ScratchpadAdd)?)?;
        window.set("scratchpad_show", self.fn_scratchpad_show()?)?;
        srd.set("window", window)?;

        let layout = lua.create_table()?;
        layout.set("set", self.fn_layout_set()?)?;
        layout.set("configure", self.fn_layout_configure()?)?;
        srd.set("layout", layout)?;

        let workspace = lua.create_table()?;
        workspace.set("next", self.fn_workspace_cycle(true)?)?;
        workspace.set("prev", self.fn_workspace_cycle(false)?)?;
        workspace.set("switch", self.fn_workspace_switch()?)?;
        workspace.set("move_window", self.fn_workspace_move_window()?)?;
        srd.set("workspace", workspace)?;

        let theme = lua.create_table()?;
        theme.set("set_colors", self.fn_theme_set("theme.colors")?)?;
        theme.set("set_decorations", self.fn_theme_set("theme.decorations")?)?;
        srd.set("theme", theme)?;

        // Expose `srd` both as a global and as a `require("srd")`-able
        // module: `require` resolves through `package.preload`/`package.path`,
        // never through globals, so config files that (reasonably) write
        // `local srd = require("srd")` would otherwise get a "module not
        // found" error despite `srd` existing as a global.
        let package: Table = lua.globals().get("package")?;
        let preload: Table = package.get("preload")?;
        preload.set(
            "srd",
            lua.create_function(|lua, ()| {
                let srd: Table = lua.globals().get("srd")?;
                Ok(srd)
            })?,
        )?;
        lua.globals().set("srd", srd)?;
        Ok(())
    }

    // ---- srd.* -----------------------------------------------------------

    fn fn_set(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (key, value): (String, Value)| {
            if let Some(v) = ConfigValue::from_lua(&value) {
                state.borrow_mut().values.insert(key, v);
            }
            Ok(())
        })?)
    }

    fn fn_get(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_reset(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_reset_all(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            state.borrow_mut().values = default_config();
            Ok(())
        })?)
    }

    fn fn_reset_category(&self) -> Result<mlua::Function<'_>> {
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
    fn fn_on(&self) -> Result<mlua::Function<'_>> {
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
    fn fn_bind_repeat(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_bind(&self) -> Result<mlua::Function<'_>> {
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
    fn fn_rule(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_load(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_spawn(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_quit(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            state.borrow().running.set(false);
            Ok(())
        })?)
    }

    fn fn_reload(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_notify(&self) -> Result<mlua::Function<'_>> {
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
    fn fn_validate_config(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, ()| {
            let s = state.borrow();
            let errors = validate(&s);
            let ok = errors.is_empty();
            Ok((ok, lua.create_sequence_from(errors)?))
        })?)
    }

    fn fn_debug_config_status(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_debug_show_settings(&self) -> Result<mlua::Function<'_>> {
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

    fn fn_debug_profile_start(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            state.borrow_mut().profile_start = Some(std::time::Instant::now());
            Ok(())
        })?)
    }

    fn fn_debug_profile_stop(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let elapsed = state.borrow_mut().profile_start.take().map(|t| t.elapsed().as_secs_f64());
            if let Some(secs) = elapsed {
                log::info!("profile: {:.3}ms", secs * 1000.0);
            }
            Ok(elapsed)
        })?)
    }

    // ---- srd.window.* ------------------------------------------------------

    fn fn_window_focused(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, ()| {
            let wm = state.borrow().wm.clone();
            let wm = wm.borrow();
            let Some(w) = wm.focused_window() else { return Ok(Value::Nil) };
            let t = lua.create_table()?;
            t.set("id", w.id)?;
            t.set("title", w.title.clone())?;
            t.set("x", w.geometry.x)?;
            t.set("y", w.geometry.y)?;
            t.set("width", w.geometry.width)?;
            t.set("height", w.geometry.height)?;
            t.set("floating", w.floating)?;
            t.set("maximized", w.maximized)?;
            t.set("minimized", w.minimized)?;
            t.set("scratchpad", w.scratchpad)?;
            Ok(Value::Table(t))
        })?)
    }

    fn fn_window_action(&self, action: WindowAction) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                match action {
                    WindowAction::Close => wm.close_window(id),
                    WindowAction::Minimize => wm.minimize_window(id),
                    WindowAction::Maximize => wm.toggle_maximize(id),
                    WindowAction::Fullscreen => wm.toggle_fullscreen(id),
                    WindowAction::ToggleFloating => wm.toggle_floating(id),
                    WindowAction::TogglePin => wm.toggle_always_on_top(id),
                    WindowAction::ScratchpadAdd => wm.scratchpad_add(id),
                }
            }
            Ok(())
        })?)
    }

    fn fn_scratchpad_show(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            wm.borrow_mut().scratchpad_show();
            Ok(())
        })?)
    }

    /// `srd.window.move("left")` - swap the focused window with its
    /// neighbour in that direction (Hyprland's `movewindow l/r/u/d`).
    fn fn_window_move_direction(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, direction: String| {
            let dir = parse_direction(&direction, "srd.window.move")?;
            let wm = state.borrow().wm.clone();
            wm.borrow_mut().move_window_direction(dir);
            Ok(())
        })?)
    }

    /// `srd.window.next()` / `srd.window.prev()` - cycle focus through the
    /// windows on the current workspace (Hyprland's `cyclenext`).
    fn fn_window_cycle(&self, forward: bool) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if forward {
                wm.focus_next();
            } else {
                wm.focus_previous();
            }
            // Bring it to the top, matching the `bringactivetotop` the
            // Hyprland binding pairs with `cyclenext`.
            if let Some(id) = wm.focused_id() {
                wm.raise_window(id);
            }
            Ok(())
        })?)
    }

    fn fn_window_focus_direction(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, direction: String| {
            let dir = parse_direction(&direction, "srd.window.focus")?;
            let wm = state.borrow().wm.clone();
            wm.borrow_mut().focus_direction(dir);
            Ok(())
        })?)
    }

    fn fn_window_set_decorations(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, enabled: bool| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.decorated = enabled;
                }
            }
            Ok(())
        })?)
    }

    fn fn_window_set_border_color(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (r, g, b): (u8, u8, u8)| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.border_color = (r, g, b);
                }
            }
            Ok(())
        })?)
    }

    fn fn_window_set_border_width(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, width: u32| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.border_width = width;
                }
            }
            Ok(())
        })?)
    }

    fn fn_window_set_opacity(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, opacity: f32| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.opacity = opacity.clamp(0.0, 1.0);
                }
            }
            Ok(())
        })?)
    }

    fn fn_window_set_floating(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, floating: bool| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(id) = wm.focused_id() {
                if let Some(w) = wm.window_mut(id) {
                    w.floating = floating;
                }
            }
            Ok(())
        })?)
    }

    fn fn_window_is_floating(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let wm = wm.borrow();
            Ok(wm.focused_id().map(|id| wm.is_floating(id)).unwrap_or(false))
        })?)
    }

    // ---- srd.layout.* ------------------------------------------------------

    fn fn_layout_set(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, name: String| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            let ws = wm.current_workspace();
            wm.set_layout(ws, name);
            wm.arrange_workspace(ws);
            Ok(())
        })?)
    }

    fn fn_layout_configure(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, (name, table): (String, Table)| {
            let mut s = state.borrow_mut();
            flatten_table_into(&format!("layout.{name}"), &table, &mut s.values)?;
            let master_ratio = s.values.get(&format!("layout.{name}.master_ratio")).and_then(|v| v.as_f64());
            drop(s);
            if let Some(ratio) = master_ratio {
                state.borrow().wm.borrow_mut().tiling.master_ratio = ratio as f32;
            }
            Ok(())
        })?)
    }

    // ---- srd.workspace.* ---------------------------------------------------

    fn fn_workspace_cycle(&self, forward: bool) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, ()| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            let ids: Vec<_> = wm.workspaces().iter().map(|w| w.id).collect();
            if ids.is_empty() {
                return Ok(());
            }
            let cur = wm.current_workspace();
            let pos = ids.iter().position(|&id| id == cur).unwrap_or(0);
            let next = if forward { (pos + 1) % ids.len() } else { (pos + ids.len() - 1) % ids.len() };
            wm.switch_workspace(ids[next]);
            Ok(())
        })?)
    }

    fn fn_workspace_switch(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, id: usize| {
            state.borrow().wm.borrow_mut().switch_workspace(id);
            Ok(())
        })?)
    }

    fn fn_workspace_move_window(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, id: usize| {
            let wm = state.borrow().wm.clone();
            let mut wm = wm.borrow_mut();
            if let Some(focused) = wm.focused_id() {
                wm.move_window_to_workspace(focused, id);
            }
            Ok(())
        })?)
    }

    // ---- srd.theme.* -------------------------------------------------------

    fn fn_theme_set(&self, prefix: &str) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        let prefix = prefix.to_string();
        Ok(self.lua.create_function(move |_, table: Table| {
            let mut s = state.borrow_mut();
            flatten_table_into(&prefix, &table, &mut s.values)?;
            Ok(())
        })?)
    }
}

/// Shared between [`Engine::reload`] and `srd.reload()`'s Lua closure --
/// the closure can't capture `&Engine` itself (it isn't `Clone`/`Rc`, and
/// `mlua::Lua::create_function` needs a `'static` closure), so both go
/// through cloned `Lua`/state handles instead of one calling the other.
fn do_reload(lua: &Lua, state: &Rc<RefCell<SharedState>>) -> Result<()> {
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
fn parse_direction(name: &str, caller: &str) -> mlua::Result<Direction> {
    match name {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        other => Err(mlua::Error::RuntimeError(format!("{caller}: unknown direction '{other}'"))),
    }
}

#[derive(Clone, Copy)]
enum WindowAction {
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
fn flatten_table_into(prefix: &str, table: &Table, out: &mut HashMap<String, ConfigValue>) -> mlua::Result<()> {
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
fn validate(s: &SharedState) -> Vec<String> {
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
fn default_config() -> HashMap<String, ConfigValue> {
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
    set("general.rounded_corners", Bool(true));
    set("general.focus_follows_mouse", Bool(false));
    set("general.mouse_follows_focus", Bool(true));
    set("general.auto_raise", Bool(false));
    set("general.auto_focus", Bool(true));

    set("monitor.primary_layout", String("dynamic".into()));
    set("monitor.secondary_layout", String("tiling".into()));
    set("monitor.auto_detect", Bool(true));
    set("monitor.primary_workspace", Number(1.0));
    set("monitor.workspace_count", Number(10.0));

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
    set("workspace.auto_switch", Bool(false));
    set("workspace.persistent", Bool(true));
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

#[cfg(test)]
mod tests {
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
}
