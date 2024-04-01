//! A small, hand-maintained keysym <-> name table covering the keys used in
//! practice by window-manager keybindings (letters, digits, navigation,
//! function keys). Not a full xkbcommon-level keymap - see module docs in
//! `lib.rs` for why that trade-off was made for this pass.

pub fn keysym_to_name(keysym: u32) -> Option<String> {
    Some(match keysym {
        0x0020 => "Space".to_string(),
        0x0061..=0x007a => (((keysym - 0x0061) as u8 + b'a') as char).to_string(),
        0x0030..=0x0039 => (((keysym - 0x0030) as u8 + b'0') as char).to_string(),
        0xff0d => "Return".to_string(),
        0xff1b => "Escape".to_string(),
        0xff09 => "Tab".to_string(),
        0xff08 => "BackSpace".to_string(),
        0xffff => "Delete".to_string(),
        0xff51 => "Left".to_string(),
        0xff52 => "Up".to_string(),
        0xff53 => "Right".to_string(),
        0xff54 => "Down".to_string(),
        0xff55 => "Prior".to_string(),
        0xff56 => "Next".to_string(),
        0xff50 => "Home".to_string(),
        0xff57 => "End".to_string(),
        0xffbe..=0xffc9 => format!("F{}", keysym - 0xffbe + 1),
        0x1008ff13 => "XF86AudioRaiseVolume".to_string(),
        0x1008ff11 => "XF86AudioLowerVolume".to_string(),
        0x1008ff12 => "XF86AudioMute".to_string(),
        0x1008ff02 => "XF86MonBrightnessUp".to_string(),
        0x1008ff03 => "XF86MonBrightnessDown".to_string(),
        _ => return None,
    })
}

/// Case-insensitive: config authors reasonably write both `"Space"` and
/// `"space"`, and X11 key names aren't consistently capitalized in the wild.
pub fn name_to_keysym(name: &str) -> Option<u32> {
    match name.to_ascii_lowercase().as_str() {
        "space" => return Some(0x0020),
        "return" | "enter" => return Some(0xff0d),
        "escape" => return Some(0xff1b),
        "tab" => return Some(0xff09),
        "backspace" => return Some(0xff08),
        "delete" => return Some(0xffff),
        "left" => return Some(0xff51),
        "up" => return Some(0xff52),
        "right" => return Some(0xff53),
        "down" => return Some(0xff54),
        "prior" | "pageup" => return Some(0xff55),
        "next" | "pagedown" => return Some(0xff56),
        "home" => return Some(0xff50),
        "end" => return Some(0xff57),
        "xf86audioraisevolume" => return Some(0x1008ff13),
        "xf86audiolowervolume" => return Some(0x1008ff11),
        "xf86audiomute" => return Some(0x1008ff12),
        "xf86monbrightnessup" => return Some(0x1008ff02),
        "xf86monbrightnessdown" => return Some(0x1008ff03),
        _ => {}
    }
    if name.len() == 1 {
        let c = name.chars().next().unwrap().to_ascii_lowercase();
        if c.is_ascii_lowercase() {
            return Some(0x0061 + (c as u32 - 'a' as u32));
        }
        if c.is_ascii_digit() {
            return Some(0x0030 + (c as u32 - '0' as u32));
        }
    }
    if let Some(rest) = name.to_ascii_lowercase().strip_prefix('f') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=12).contains(&n) {
                return Some(0xffbe + n - 1);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_roundtrip() {
        for c in 'a'..='z' {
            let name = c.to_string();
            let ks = name_to_keysym(&name).unwrap();
            assert_eq!(keysym_to_name(ks), Some(name));
        }
        for c in '0'..='9' {
            let name = c.to_string();
            let ks = name_to_keysym(&name).unwrap();
            assert_eq!(keysym_to_name(ks), Some(name));
        }
    }

    #[test]
    fn lowercase_names_resolve_case_insensitively() {
        // Config authors reasonably write "Mod4+Shift+space"; this must
        // resolve to the same keysym as "Space".
        assert_eq!(name_to_keysym("space"), name_to_keysym("Space"));
        assert_eq!(name_to_keysym("return"), name_to_keysym("Return"));
        assert_eq!(name_to_keysym("f5"), name_to_keysym("F5"));
    }

    #[test]
    fn named_keys_roundtrip() {
        for name in ["Return", "Escape", "Tab", "Left", "Right", "Up", "Down", "F1", "F12"] {
            let ks = name_to_keysym(name).unwrap();
            assert_eq!(keysym_to_name(ks), Some(name.to_string()));
        }
    }
}
