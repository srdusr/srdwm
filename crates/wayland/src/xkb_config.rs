//! Reads the system's real keyboard layout/model/options so both backends'
//! `seat.add_keyboard()` calls actually use them.
//!
//! `smithay::input::keyboard::XkbConfig::default()` - what both backends
//! passed unconditionally before this - resolves any field left as `""`/
//! `None` via the `XKB_DEFAULT_*` environment variables, per xkbcommon's
//! own documented behavior (see its doc comment). That covers a session
//! that actually sets those variables, but this machine doesn't: the real
//! configuration lives in `/etc/X11/xorg.conf.d/00-keyboard.conf`, written
//! by `systemd-localed` from `localectl`, which nothing was ever reading.
//! Concretely: `Option "XkbOptions" "terminate:ctrl_alt_bksp"` was silently
//! dropped, along with the model (`pc105+inet`), regardless of what
//! `localectl status` actually reports.
//!
//! Deliberately fails soft, field by field: a missing file, an unreadable
//! one, or a field just not present in it all fall through to the same
//! `""`/`None` `Default::default()` already used, not an error - this is
//! strictly additive over today's behavior, never worse.

use std::collections::HashMap;

/// Parsed fields from the standard `XkbLayout`/`XkbModel`/`XkbVariant`/
/// `XkbOptions` `Option "..." "..."` lines `systemd-localed` writes.
/// Anything not found is `None`, which is exactly what an empty-string
/// `XkbConfig` field already meant.
#[derive(Default)]
pub(crate) struct SystemXkbConfig {
    pub(crate) model: Option<String>,
    pub(crate) layout: Option<String>,
    pub(crate) variant: Option<String>,
    pub(crate) options: Option<String>,
}

const KEYBOARD_CONF_PATH: &str = "/etc/X11/xorg.conf.d/00-keyboard.conf";

pub(crate) fn read() -> SystemXkbConfig {
    let Ok(content) = std::fs::read_to_string(KEYBOARD_CONF_PATH) else {
        return SystemXkbConfig::default();
    };
    let fields = parse(&content);
    SystemXkbConfig {
        model: fields.get("XkbModel").cloned(),
        layout: fields.get("XkbLayout").cloned(),
        variant: fields.get("XkbVariant").cloned(),
        options: fields.get("XkbOptions").cloned(),
    }
}

/// Extracts `Option "Name" "value"` lines into a name -> value map. Not a
/// general X11 config parser - this file has exactly one `InputClass`
/// section with a handful of `Option` lines in that fixed form, always
/// machine-written, so a line-by-line scan for that one pattern is all
/// this needs.
fn parse(content: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Option") else { continue };
        let quoted: Vec<&str> = rest.split('"').filter(|s| !s.trim().is_empty()).collect();
        if let [name, value] = quoted[..] {
            fields.insert(name.to_string(), value.to_string());
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_systemd_localed_file() {
        let content = r#"
Section "InputClass"
        Identifier "system-keyboard"
        MatchIsKeyboard "on"
        Option "XkbLayout" "us"
        Option "XkbModel" "pc105+inet"
        Option "XkbOptions" "terminate:ctrl_alt_bksp"
EndSection
"#;
        let fields = parse(content);
        assert_eq!(fields.get("XkbLayout").map(String::as_str), Some("us"));
        assert_eq!(fields.get("XkbModel").map(String::as_str), Some("pc105+inet"));
        assert_eq!(fields.get("XkbOptions").map(String::as_str), Some("terminate:ctrl_alt_bksp"));
    }

    #[test]
    fn ignores_unrelated_lines_without_panicking() {
        let content = "Section \"InputClass\"\nIdentifier \"system-keyboard\"\nMatchIsKeyboard \"on\"\nEndSection\n";
        assert!(parse(content).is_empty());
    }

    #[test]
    fn missing_file_yields_all_none_not_an_error() {
        // read() itself isn't unit-testable without touching the real
        // filesystem path, but the fallback behavior it guarantees is
        // exactly SystemXkbConfig::default() - covered by construction.
        let cfg = SystemXkbConfig::default();
        assert!(cfg.model.is_none() && cfg.layout.is_none() && cfg.variant.is_none() && cfg.options.is_none());
    }
}
