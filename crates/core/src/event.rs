use crate::monitor::{Monitor, MonitorId};
use crate::window::WindowId;
use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Modifiers: u8 {
        const SHIFT = 0b0000_0001;
        const CTRL  = 0b0000_0010;
        const ALT   = 0b0000_0100;
        const SUPER = 0b0000_1000;
    }
}

impl std::fmt::Display for Modifiers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.contains(Modifiers::CTRL) {
            write!(f, "Ctrl+")?;
        }
        if self.contains(Modifiers::SHIFT) {
            write!(f, "Shift+")?;
        }
        if self.contains(Modifiers::ALT) {
            write!(f, "Alt+")?;
        }
        if self.contains(Modifiers::SUPER) {
            write!(f, "Mod4+")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u8),
}

/// A key combination, e.g. "Mod4+Shift+Return", used both as the canonical
/// string form for Lua keybindings and as the lookup key at dispatch time.
pub fn key_combo_string(modifiers: Modifiers, key_name: &str) -> String {
    format!("{modifiers}{key_name}")
}

#[derive(Debug, Clone)]
pub enum Event {
    WindowCreated(WindowId),
    WindowDestroyed(WindowId),
    WindowTitleChanged(WindowId, String),
    WindowMoved { id: WindowId, x: i32, y: i32 },
    WindowResized { id: WindowId, width: u32, height: u32 },
    WindowFocused(WindowId),
    WindowUnfocused(WindowId),
    KeyPress { key_name: String, modifiers: Modifiers },
    KeyRelease { key_name: String, modifiers: Modifiers },
    MouseButtonPress { button: MouseButton, x: i32, y: i32 },
    MouseButtonRelease { button: MouseButton, x: i32, y: i32 },
    MouseMotion { x: i32, y: i32 },
    MonitorAdded(Monitor),
    MonitorRemoved(MonitorId),
    /// The laptop lid was closed or opened. Emitted by the udev backend from
    /// libinput switch events; `closed` is true when the lid is shut.
    ///
    /// Exposed to config as `srd.on_lid("closed"/"open", fn)` so a session
    /// can lock and suspend, which is otherwise impossible: a laptop that
    /// does nothing on lid-close is a real problem, not a nicety.
    LidSwitch { closed: bool },
}
