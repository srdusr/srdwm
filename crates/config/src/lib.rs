//! The `srd` Lua scripting API: config values, keybindings, layout/theme
//! setup, and window/workspace actions, all callable from `.lua` files.
//!
//! See [`engine`]'s module doc comment for the actual API surface and how
//! it compares to the legacy C++ engine.

mod engine;
mod value;

pub use engine::{ConfigError, Engine, Result};
pub use value::ConfigValue;
