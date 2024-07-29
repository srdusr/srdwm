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

use crate::value::ConfigValue;
use support::{default_config, do_reload};

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
}

mod general;
mod layout;
mod register;
mod support;
mod theme;
mod window;
mod workspace;

#[cfg(test)]
mod tests;
