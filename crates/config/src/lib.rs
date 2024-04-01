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
use srdwm_core::{Direction, WindowManager};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct SharedState {
    wm: Rc<RefCell<WindowManager>>,
    values: HashMap<String, ConfigValue>,
    key_bindings: HashMap<String, RegistryKey>,
    config_dir: PathBuf,
    log: Vec<String>,
    running: Rc<std::cell::Cell<bool>>,
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
            config_dir: config_dir.into(),
            log: Vec::new(),
            running: Rc::new(std::cell::Cell::new(true)),
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

    /// Runs the Lua function bound to `combo` (e.g. `"Mod4+Return"`), if any.
    /// Returns `true` if a binding existed and ran without erroring.
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

    fn register_srd_module(&self) -> Result<()> {
        let lua = &self.lua;
        let srd = lua.create_table()?;

        srd.set("set", self.fn_set()?)?;
        srd.set("get", self.fn_get()?)?;
        srd.set("reset", self.fn_reset()?)?;
        srd.set("reset_all", self.fn_reset_all()?)?;
        srd.set("reset_category", self.fn_reset_category()?)?;
        srd.set("bind", self.fn_bind()?)?;
        srd.set("load", self.fn_load()?)?;
        srd.set("spawn", self.fn_spawn()?)?;
        srd.set("notify", self.fn_notify()?)?;
        srd.set("quit", self.fn_quit()?)?;

        let window = lua.create_table()?;
        window.set("focused", self.fn_window_focused()?)?;
        window.set("close", self.fn_window_action(WindowAction::Close)?)?;
        window.set("minimize", self.fn_window_action(WindowAction::Minimize)?)?;
        window.set("maximize", self.fn_window_action(WindowAction::Maximize)?)?;
        window.set("focus", self.fn_window_focus_direction()?)?;
        window.set("set_decorations", self.fn_window_set_decorations()?)?;
        window.set("set_border_color", self.fn_window_set_border_color()?)?;
        window.set("set_border_width", self.fn_window_set_border_width()?)?;
        window.set("set_floating", self.fn_window_set_floating()?)?;
        window.set("toggle_floating", self.fn_window_action(WindowAction::ToggleFloating)?)?;
        window.set("is_floating", self.fn_window_is_floating()?)?;
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

    fn fn_bind(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |lua, (combo, f): (String, mlua::Function)| {
            let key = lua.create_registry_value(f)?;
            state.borrow_mut().key_bindings.insert(combo, key);
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
                    WindowAction::ToggleFloating => wm.toggle_floating(id),
                }
            }
            Ok(())
        })?)
    }

    fn fn_window_focus_direction(&self) -> Result<mlua::Function<'_>> {
        let state = self.state.clone();
        Ok(self.lua.create_function(move |_, direction: String| {
            let dir = match direction.as_str() {
                "left" => Direction::Left,
                "right" => Direction::Right,
                "up" => Direction::Up,
                "down" => Direction::Down,
                other => return Err(mlua::Error::RuntimeError(format!("srd.window.focus: unknown direction '{other}'"))),
            };
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

#[derive(Clone, Copy)]
enum WindowAction {
    Close,
    Minimize,
    Maximize,
    ToggleFloating,
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

    set("layout.dynamic.snap_threshold", Number(50.0));
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
    fn load_init_runs_the_users_init_lua() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("init.lua"), r#"srd.set("general.window_gap", 4)"#).unwrap();
        let engine = engine_in(dir.path());
        engine.load_init().unwrap();
        assert_eq!(engine.get("general.window_gap"), Some(ConfigValue::Number(4.0)));
    }
}
