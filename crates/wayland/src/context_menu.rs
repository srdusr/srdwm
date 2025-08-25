//! Right-click titlebar window menu - the row set and hit-testing now
//! live in `srdwm_core::context_menu` (shared with the X11 backend, which
//! needs the exact same rows). This re-export keeps every existing
//! `crate::context_menu::...` call site in this crate unchanged.
//!
//! `decoration::render_context_menu` still owns the actual pixels; this
//! crate has no rendering-specific state of its own to add on top of the
//! shared struct.

pub(crate) use srdwm_core::context_menu::{ContextMenu, MenuAction};
