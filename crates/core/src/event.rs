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
///
/// Lowercases a single ASCII letter's name before building the string --
/// `Shift+r` and `Shift+R` are genuinely different X11 keysyms (`XK_r`/
/// `XK_R`), not case variants of one shared symbol, so a real Shift+<letter>
/// keypress's `modified_sym()` (see `input.rs::handle_keyboard_key_event`)
/// reports the uppercase name even though `Shift` is *also* tracked as its
/// own bit in `modifiers` - meaning without this, "Super+Shift+r" (how
/// every config writes it, lowercase) could never match a real Shift+R
/// keypress's actual combo string ("Shift+Mod4+R"). `canonicalize_key_
/// combo`'s own case-fixing (for named keys like "Space") explicitly
/// leaves plain letters alone, so it doesn't catch this either - this is
/// the one place both the config-registration path (`canonicalize_key_
/// combo` calls this) and the real-dispatch path converge, so normalizing
/// here is the single fix for both. Confirmed live: a real keypress
/// reached srdwm fine (a sibling Alt+Tab binding in the same config
/// fired), but every `Shift+<letter>` binding - including the reload key
/// this was found chasing - silently never did.
pub fn key_combo_string(modifiers: Modifiers, key_name: &str) -> String {
    let lowercased;
    let key_name = if key_name.chars().count() == 1 && key_name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        lowercased = key_name.to_ascii_lowercase();
        lowercased.as_str()
    } else {
        key_name
    };
    format!("{modifiers}{key_name}")
}

/// Parses a `"Mod4+Shift+Return"`-style combo string into modifiers plus the
/// bare key name, accepting the modifier tokens in *any* order.
///
/// This matters because [`key_combo_string`]/[`Modifiers`]'s `Display` only
/// ever produce one fixed order (Ctrl, Shift, Alt, Mod4) - but every
/// shipped keybinding is written the conventional "Mod4+Shift+x" way
/// (Super first, matching Hyprland's own `SUPER, SHIFT, x` convention).
/// `srd.bind` used to store the combo string exactly as the Lua config
/// wrote it, and dispatch always looked it up by the canonical
/// Ctrl/Shift/Alt/Mod4 order built from the real keypress - so any
/// binding combining more than one modifier in a different order than that
/// fixed one could never fire: X11 grabbed the physical key correctly
/// (`grab_keybindings` already parsed order-independently, duplicating
/// this logic) but dispatch found nothing to run, and on Wayland the combo
/// was not even recognized as bound at all, so the keypress was forwarded
/// straight to the focused client instead of reaching srdwm. Confirmed
/// against the shipped `keybindings.lua`: every multi-modifier binding
/// there (`Mod4+Shift+*`, `Mod4+Ctrl+k`, `Alt+Shift+Tab`, ...) is written
/// Super/Alt-first, which never matched the canonical order. Returns
/// `None` for an empty combo (no key name at all).
pub fn parse_key_combo(combo: &str) -> Option<(Modifiers, &str)> {
    let parts: Vec<&str> = combo.split('+').collect();
    let (key_name, mod_parts) = parts.split_last()?;
    let mut modifiers = Modifiers::empty();
    for m in mod_parts {
        modifiers |= match *m {
            "Ctrl" => Modifiers::CTRL,
            "Shift" => Modifiers::SHIFT,
            "Alt" => Modifiers::ALT,
            "Mod4" | "Super" => Modifiers::SUPER,
            _ => Modifiers::empty(),
        };
    }
    Some((modifiers, key_name))
}

/// The form of a combo to *show a person*, as distinct from the canonical
/// form everything internal is keyed by.
///
/// Two differences, both of which matter only on screen:
///
/// - `Mod4` is written `Super`. `Mod4` is the X11 modifier's name, not the
///   key's; nothing on a keyboard is labelled Mod4, and a launcher listing
///   "Mod4+Return" is showing an implementation detail.
/// - Modifiers come out in the order people write them - Super, Ctrl, Alt,
///   Shift - rather than the canonical Ctrl/Shift/Alt/Mod4 order, which
///   renders the owner's own `Super+Shift+h` binding as `Shift+Mod4+h`.
///
/// Round-trips: [`parse_key_combo`] accepts `Super` and takes modifiers in
/// any order, so a string from here can be pasted straight into a config.
/// Nothing dispatches on this form - [`canonicalize_key_combo`] still owns
/// what a binding is stored and looked up as.
pub fn display_key_combo(combo: &str) -> String {
    let Some((modifiers, key_name)) = parse_key_combo(combo) else { return combo.to_string() };
    let mut out = String::new();
    for (flag, name) in
        [(Modifiers::SUPER, "Super"), (Modifiers::CTRL, "Ctrl"), (Modifiers::ALT, "Alt"), (Modifiers::SHIFT, "Shift")]
    {
        if modifiers.contains(flag) {
            out.push_str(name);
            out.push('+');
        }
    }
    out.push_str(key_name);
    out
}

/// Re-orders a combo string into the canonical form [`key_combo_string`]
/// produces, regardless of what order its modifiers were written in, *and*
/// normalizes the key name to the exact casing [`crate::keysyms::
/// keysym_to_name`] produces at dispatch time.
///
/// That second part matters on its own, independent of modifier order:
/// `keysym_to_name` capitalizes every named key ("Space", "Return",
/// "Escape", "BackSpace", ...) while leaving letters/digits as-is, but
/// nothing constrains how a config author *writes* one - `srd.bind
/// ("Super+space", ...)` (lowercase, as `keybindings.lua` had it) parsed to
/// the literal key name "space" with no case change, so the string stored
/// here never matched what a real Space keypress builds at dispatch
/// ("Space", capitalized) even though the modifier-order fix above was
/// already in place. The bind was accepted at config-load time (no error,
/// nothing to notice) and then simply never fired - confirmed live: the
/// bind's own diagnostic (spawning a command that logs to a file) never
/// produced a log entry, meaning the keypress never even reached the
/// callback. Round-tripping through [`crate::keysyms::name_to_keysym`] (which
/// *is* already case-insensitive) and back fixes any such case mismatch for
/// every key the table recognizes; an unrecognized name is left as-is
/// (harmless - it wouldn't have matched at dispatch regardless of case).
///
/// Unparseable input (empty string) is returned unchanged, so a caller that
/// can't do anything better with it still has *something* to store/log.
pub fn canonicalize_key_combo(combo: &str) -> String {
    match parse_key_combo(combo) {
        Some((modifiers, key_name)) => {
            let canonical_name =
                crate::keysyms::name_to_keysym(key_name).and_then(crate::keysyms::keysym_to_name).unwrap_or_else(|| key_name.to_string());
            key_combo_string(modifiers, &canonical_name)
        }
        None => combo.to_string(),
    }
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
    /// `WindowManager::current_workspace` changed. Carries no id: every
    /// consumer that cares (`main.rs`'s `sync()`) re-reads whichever
    /// workspace is current now rather than trusting a stale snapshot from
    /// whenever this event was queued.
    ///
    /// Exists purely so `sync()` actually runs after a switch - without a
    /// `dirty`-setting event, `WindowManager::switch_workspace` alone only
    /// changes core's own bookkeeping; nothing shows or hides a single
    /// window for the new workspace until `sync()` runs, which only
    /// happens when a polled event sets `dirty`. Every switch path (a
    /// keybinding, `SUPER`+scroll, the `ext_workspace_v1` protocol's
    /// `activate` request) needs this pushed after it changes
    /// `current_workspace`, or the switch is invisible.
    WorkspaceChanged,
}

#[cfg(test)]
mod tests {
    /// A launcher shows this string to a person, so it has to read like the
    /// key on the keyboard and like what the config author wrote.
    #[test]
    fn display_form_uses_super_and_the_order_people_write() {
        assert_eq!(display_key_combo("Shift+Mod4+h"), "Super+Shift+h");
        assert_eq!(display_key_combo("Ctrl+Mod4+k"), "Super+Ctrl+k");
        assert_eq!(display_key_combo("Shift+Alt+Tab"), "Alt+Shift+Tab");
        assert_eq!(display_key_combo("Mod4+Return"), "Super+Return");
        assert_eq!(display_key_combo("Print"), "Print");
    }

    /// The display form must be pasteable back into a config, or telling
    /// someone their binding is "Super+Shift+h" sends them to a string that
    /// does not work.
    #[test]
    fn display_form_parses_back_to_the_same_binding() {
        for combo in ["Shift+Mod4+h", "Ctrl+Mod4+k", "Shift+Alt+Tab", "Mod4+Return", "Mod4+1"] {
            assert_eq!(canonicalize_key_combo(&display_key_combo(combo)), canonicalize_key_combo(combo));
        }
    }

    use super::*;

    #[test]
    fn super_first_combo_canonicalizes_to_dispatch_order() {
        // The shipped keybindings.lua writes every combo Super-first
        // ("Mod4+Shift+m"), matching Hyprland's own convention - but
        // `key_combo_string`'s Display order is fixed Ctrl/Shift/Alt/Mod4.
        // A binding registered under its literal Lua string could never be
        // found by a real keypress, which always dispatches through the
        // canonical order. This is exactly the bug `canonicalize_key_combo`
        // exists to close.
        assert_eq!(canonicalize_key_combo("Mod4+Shift+m"), "Shift+Mod4+m");
        assert_eq!(canonicalize_key_combo("Mod4+Ctrl+k"), "Ctrl+Mod4+k");
        assert_eq!(canonicalize_key_combo("Alt+Shift+Tab"), "Shift+Alt+Tab");
    }

    #[test]
    fn already_canonical_combo_is_unchanged() {
        assert_eq!(canonicalize_key_combo("Ctrl+Mod4+k"), "Ctrl+Mod4+k");
    }

    #[test]
    fn single_modifier_combo_is_unaffected() {
        // No ordering ambiguity with one modifier - this case always
        // worked, before and after the fix.
        assert_eq!(canonicalize_key_combo("Mod4+Return"), "Mod4+Return");
    }

    #[test]
    fn a_real_shifted_letter_keypress_matches_the_lowercase_written_binding() {
        // `Shift+r` and `Shift+R` are different X11 keysyms (`XK_r`/`XK_R`),
        // not case variants of one shared symbol - a real Shift+<letter>
        // keypress's dispatch-time key name is the *uppercase* one, even
        // though Shift is also tracked as its own bit in `modifiers`. Every
        // config writes bindings lowercase ("Super+Shift+r"), so the two
        // sides have to agree despite that: this is what silently broke
        // every `Shift+<letter>` binding (found chasing a dead reload key)
        // until `key_combo_string` started normalizing single letters.
        let config_side = canonicalize_key_combo("Mod4+Shift+r");
        let dispatch_side = key_combo_string(Modifiers::SHIFT | Modifiers::SUPER, "R");
        assert_eq!(config_side, dispatch_side);
        assert_eq!(config_side, "Shift+Mod4+r");
    }

    #[test]
    fn named_keys_are_unaffected_by_the_letter_lowercasing() {
        // Multi-character key names ("Space", "Return", ...) don't change
        // identity with Shift the way a single letter's keysym does, so
        // they must not be touched by the same normalization.
        assert_eq!(key_combo_string(Modifiers::SHIFT, "Space"), "Shift+Space");
    }

    #[test]
    fn lowercase_named_key_still_reaches_the_capitalized_dispatch_form() {
        // `srd.bind("Super+space", ...)` (lowercase, as a real config had
        // it) must resolve to the exact same string a live Space keypress
        // builds at dispatch time - `keysyms::keysym_to_name` always
        // capitalizes named keys ("Space"), so without this normalization
        // the bind is silently accepted at load time and then never fires.
        assert_eq!(canonicalize_key_combo("Super+space"), canonicalize_key_combo("Super+Space"));
        assert_eq!(canonicalize_key_combo("Super+space"), "Mod4+Space");
        assert_eq!(canonicalize_key_combo("Super+Shift+return"), canonicalize_key_combo("Super+Shift+Return"));
    }

    #[test]
    fn unrecognized_key_name_is_left_as_is() {
        // Not in `keysyms`' table at all - round-tripping through it fails,
        // so the original text passes through unchanged rather than being
        // silently dropped.
        assert_eq!(canonicalize_key_combo("Mod4+NotARealKey"), "Mod4+NotARealKey");
    }

    #[test]
    fn parse_key_combo_accepts_modifiers_in_any_order() {
        let (mods, key) = parse_key_combo("Mod4+Shift+m").unwrap();
        assert_eq!(key, "m");
        assert!(mods.contains(Modifiers::SUPER) && mods.contains(Modifiers::SHIFT));

        let (mods2, key2) = parse_key_combo("Shift+Mod4+m").unwrap();
        assert_eq!(key2, "m");
        assert_eq!(mods, mods2);
    }

    #[test]
    fn parse_key_combo_with_no_modifiers_is_bare_key() {
        let (mods, key) = parse_key_combo("Return").unwrap();
        assert_eq!(key, "Return");
        assert_eq!(mods, Modifiers::empty());
    }
}
