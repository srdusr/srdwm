//! Keyboard key events: precise keybinding matching against
//! `srdwm_core::keysyms`, VT switching, and the locked-session/native-lock
//! password-entry path.

use smithay::backend::input::{KeyState as BackendKeyState, KeyboardKeyEvent};
use smithay::backend::session::Session as _;
use smithay::input::keyboard::FilterResult;
use smithay::utils::SERIAL_COUNTER;

use srdwm_core::{Event as CoreEvent, Modifiers};

use crate::state::CompState;

/// Shared between the winit (nested) and udev (bare-TTY) backends: both
/// deliver keyboard events through smithay's generic `KeyboardKeyEvent`
/// trait, so the precise-keybinding-matching logic (see the module docs)
/// only needs to exist once.
pub(crate) fn handle_keyboard_key_event<B: smithay::backend::input::InputBackend, E: KeyboardKeyEvent<B>>(state: &mut CompState, event: &E) {
    super::notify_idle_activity(state);
    let keycode = event.key_code();
    let key_state = event.state();
    let time = event.time_msec();
    let serial = SERIAL_COUNTER.next_serial();
    let Some(keyboard) = state.seat.get_keyboard() else { return };

    // While the session is locked, every key goes to the lock surface and
    // *nothing* is treated as a WM keybinding. Skipping this would leave the
    // lock trivially bypassable - the config binds spawn commands
    // (`Mod4+Return` opens a terminal), so honouring bindings here would let
    // anyone at a locked screen run arbitrary programs.
    if state.lock.locked {
        // A native lock (`crate::native_lock`) has no external client
        // surface to forward to at all - srdwm is its own locker, so
        // every keystroke feeds the password buffer directly instead.
        // Only on press: a character is typed on key-down, matching
        // ordinary text input, and password/BackSpace/Return/Escape
        // handling only make sense once per physical keystroke, not once
        // per press *and* release.
        if state.lock.native.is_some() {
            if key_state == BackendKeyState::Pressed {
                keyboard.input::<(), _>(state, keycode, key_state, serial, time, |data, mods, handle| {
                    // `keysym_to_utf8` on the already-resolved keysym
                    // (rather than the state-aware `xkb_state_key_get_
                    // utf8` xkbcommon's own docs recommend) is a
                    // deliberate simplification: correct for plain
                    // ASCII/shifted-symbol passwords, which is the
                    // overwhelming common case; the gap is dead-key/
                    // compose sequences spanning more than one keypress,
                    // which would just make that one character not match
                    // rather than ever falsely succeed - a usability
                    // rough edge, not a security one. Computed before
                    // `keysym_name_for` below, which takes `handle` by
                    // value.
                    let utf8 = xkbcommon::xkb::keysym_to_utf8(handle.modified_sym());
                    let name = keysym_name_for(handle).unwrap_or_default();
                    data.native_lock_key(&name, &utf8, mods.caps_lock);
                    FilterResult::Intercept(())
                });
            } else {
                keyboard.input::<(), _>(state, keycode, key_state, serial, time, |_, _, _| FilterResult::Intercept(()));
            }
            return;
        }
        keyboard.input::<(), _>(state, keycode, key_state, serial, time, |_, _, _| FilterResult::Forward);
        return;
    }

    let bound_keys = state.bound_keys.clone();
    let matched: Option<(String, Modifiers)> =
        keyboard.input(state, keycode, key_state, serial, time, move |data, mods, handle| {
            let modifiers = core_modifiers_from_xkb(mods);
            // `Ctrl+Alt+F1`..`F12` (xkb emits these as the `XF86Switch_VT_1`..
            // `_12` keysyms, not a plain function-key + modifier combo) --
            // handled here, by raw keysym *value* rather than name, since
            // matching a name string wrong fails silently and looks
            // identical to this never having been implemented at all (it
            // wasn't, until now: reported live, the user had to leave the
            // graphical session entirely and log in on a different TTY to
            // get a shell back after srdwm went down, because nothing ever
            // told the session to switch away). Values are contiguous
            // (0x1008FE01..=0x1008FE0C, xkbcommon's `keysyms.rs`), so `raw -
            // KEY_XF86SWITCH_VT_1 + 1` is the target VT. Udev/bare-TTY
            // backend only - `data.udev` is `None` under the nested winit
            // backend, where VT switching is meaningless, so this is a
            // no-op there rather than an error, same as every other
            // udev-only feature in this module.
            const KEY_XF86SWITCH_VT_1: u32 = 0x1008_FE01;
            const KEY_XF86SWITCH_VT_12: u32 = 0x1008_FE0C;
            let raw = handle.modified_sym().raw();
            if (KEY_XF86SWITCH_VT_1..=KEY_XF86SWITCH_VT_12).contains(&raw) {
                if key_state == BackendKeyState::Pressed {
                    if let Some(udev) = data.udev.as_mut() {
                        let vt = (raw - KEY_XF86SWITCH_VT_1 + 1) as i32;
                        if let Err(e) = udev.session.change_vt(vt) {
                            log::warn!("udev: change_vt({vt}) failed: {e}");
                        }
                    }
                }
                return FilterResult::Intercept((String::new(), modifiers));
            }
            match keysym_name_for(handle) {
                Some(name) if bound_keys.contains(&srdwm_core::key_combo_string(modifiers, &name)) => {
                    FilterResult::Intercept((name, modifiers))
                }
                _ => FilterResult::Forward,
            }
        });

    match key_state {
        BackendKeyState::Pressed => {
            // An empty `key_name` is the VT-switch case above, already
            // fully handled inside the closure - it isn't a real
            // keybinding and must not start a repeat timer or fire a
            // `CoreEvent::KeyPress` (`Lua` config has nothing bound to `""`,
            // so this would be harmless either way, but skipping it is both
            // cheaper and clearer than relying on that).
            if let Some((key_name, modifiers)) = matched {
                if !key_name.is_empty() {
                    state.begin_repeat(keycode, &key_name, modifiers);
                    state.pending.borrow_mut().push(CoreEvent::KeyPress { key_name, modifiers });
                }
            }
        }
        // Any release ends a repeat of *that* key; releasing an unrelated
        // key must not stop it.
        BackendKeyState::Released => state.end_repeat(keycode),
    }
    // Unmatched keys were already forwarded to the focused client by
    // `FilterResult::Forward` inside the closure above.
}

/// Translates the effective xkb keysym for this keypress into the same
/// `"Return"`/`"a"`/`"F5"`-style name `srdwm_core::keysyms` uses, so a
/// binding written once in Lua resolves identically on X11 and Wayland.
pub(crate) fn keysym_name_for(handle: smithay::input::keyboard::KeysymHandle<'_>) -> Option<String> {
    // `raw_syms()` - the keycode's level-0 (unshifted) symbol for the
    // *current* layout, not `modified_sym()` (what Shift actually turns it
    // into). For a keybinding like `Super+Shift+2`, matching against
    // `modified_sym()` looked up whatever Shift+2 really produces on the
    // active layout - `@` on US, and something else again on most other
    // layouts - which never equals the literal name `"2"` the Lua config
    // binds against. Every `Super+Shift+<number>` binding
    // (`keybindings.lua`'s `workspace.move_window`) silently never matched
    // anything, indistinguishable from not being bound at all. Shift is
    // still fully honored as a *modifier* - `core_modifiers_from_xkb`
    // reads it independently of which symbol this function returns - this
    // only changes which symbol *name* represents "the 2 key", the same
    // physical-key-plus-modifier-flags model every other keybinding system
    // (Hyprland, i3, sway) uses. The other caller of this function (the
    // native lock's password entry) only ever compares the result against
    // non-shift-sensitive names (`BackSpace`/`Return`/`Escape`), so this
    // doesn't change that path's behavior at all - real character input
    // there already goes through `keysym_to_utf8(handle.modified_sym())`
    // separately, untouched by this.
    let sym = handle.raw_syms().first().copied().unwrap_or_else(|| handle.modified_sym());
    srdwm_core::keysyms::keysym_to_name(sym.raw())
}

pub(crate) fn core_modifiers_from_xkb(mods: &smithay::input::keyboard::ModifiersState) -> Modifiers {
    let mut m = Modifiers::empty();
    if mods.shift {
        m |= Modifiers::SHIFT;
    }
    if mods.ctrl {
        m |= Modifiers::CTRL;
    }
    if mods.alt {
        m |= Modifiers::ALT;
    }
    if mods.logo {
        m |= Modifiers::SUPER;
    }
    m
}
