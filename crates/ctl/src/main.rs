//! Control CLI for srdwm's control socket (`crates/platform/src/ipc.rs`,
//! shared by every backend that has one). Deliberately dumb: builds one
//! request line from argv, writes it, reads the response(s), prints them.
//! All real logic - and all JSON encoding - lives server-side; this
//! stays a thin pipe so it never needs a JSON dependency of its own.
//!
//! Usage:
//!   srd clients                        list windows, one JSON object
//!   srd workspaces                     list workspaces, one JSON object
//!   srd settings                       current shadows/rounded_corners/
//!                                       animations/night_light/reading_mode
//!                                       state, one JSON object
//!   srd keyboard layout                active XKB layout name, one JSON object
//!   srd subscribe                      like `clients`, then one JSON
//!                                       object per line forever, each time
//!                                       the window list, workspace list,
//!                                       keyboard layout, or monitor list
//!                                       actually changes (one "clients"/
//!                                       "workspaces"/"keyboard_layout"/
//!                                       "monitors"-tagged line per event)
//!
//! `dispatch` reads as a real verb phrase, not a joined identifier --
//! `toggle floating`, not `toggle_floating`. Multi-word actions are
//! genuinely separate arguments (ordinary shell word-splitting), matched
//! here on the words that follow the leading verb rather than one fused
//! snake_case token:
//!   srd dispatch focus ID
//!   srd dispatch close ID
//!   srd dispatch toggle visibility ID   hide/show a window
//!   srd dispatch toggle maximize ID
//!   srd dispatch toggle fullscreen ID
//!   srd dispatch toggle floating ID
//!   srd dispatch toggle pinned ID
//!   srd dispatch move window ID left|right|up|down
//!   srd dispatch move workspace ID WORKSPACE
//!   srd dispatch activate workspace ID
//!   srd dispatch cycle keyboard layout  no ID - there's only ever one keyboard
//!   srd dispatch set output position NAME|ID X Y   NAME (e.g. HDMI-A-1, as
//!                                    `srd monitors` reports it) or a plain
//!                                    numeric id, either works
//!   srd dispatch set output enabled NAME|ID true|false   real DRM power
//!                                    state, not just hiding it - a
//!                                    disabled output stops presenting
//!                                    and its `wl_output` global goes away
//!                                    until re-enabled
//!   srd dispatch set output split NAME|ID PARTS [rows|columns]   divides
//!                                    one real output into PARTS logical
//!                                    monitors for placement/tiling --
//!                                    columns (default) side by side,
//!                                    rows stacked; PARTS <= 1 clears an
//!                                    existing split. Live, no restart.
//!   srd set border_width 3          live theme values, applied immediately
//!   srd set border_color '#cba6f7'  (hex string)
//!   srd set corner_radius 10
//!   srd set gap_inner 8
//!   srd set gap_outer 16
//!   srd set shadows true
//!   srd set rounded_corners true
//!   srd set animations true
//!   srd set night_light true        warm tint; false clears it regardless
//!   srd set reading_mode true       desaturating tint; same "false clears
//!                                    either" rule - `srd settings` reads
//!                                    back which one (if any) is active
//!   srd set decoration_mode server|client
//!
//! The socket is Unix-domain; on platforms without one this always fails
//! cleanly rather than not building at all, since it's one binary in a
//! workspace that otherwise targets Windows and macOS too.

#[cfg(unix)]
fn main() {
    // Rust masks SIGPIPE to SIG_IGN at process startup (so a write past a
    // closed pipe/socket surfaces as a normal `Err`, not a silent kill) --
    // restoring the default here (SIG_DFL) is what a well-behaved Unix CLI
    // tool is expected to do, since `srd subscribe`'s whole design is
    // printing one line per event forever until its reader goes away.
    // Without this, that reader closing (AGS quitting, a piped `head -1`,
    // anything) doesn't kill this process the normal Unix way - instead
    // the next `println!` gets `Err(BrokenPipe)`, and `std::io::stdio::
    // _print` panics on that rather than returning it, so the process dies
    // through a Rust unwind/abort instead of the expected SIGPIPE. Caught
    // live: a peer session's AGS instance running `srd subscribe` as a
    // long-lived child over a stdout pipe hit exactly this panic every
    // single time it quit.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = match build_request(&args) {
        Ok(r) => r,
        Err(msg) => {
            eprintln!("srd: {msg}");
            print_usage();
            std::process::exit(2);
        }
    };

    if args.first().map(String::as_str) == Some("subscribe") {
        if let Err(e) = unix::stream(&request) {
            eprintln!("srd: {e}");
            std::process::exit(1);
        }
        return;
    }

    let response = match unix::send(&request) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("srd: {e}");
            std::process::exit(1);
        }
    };
    println!("{response}");
    // A rejected `dispatch`/`set` (a bad key, a malformed value, a window
    // id that doesn't exist) is still a *successful* request/response --
    // `unix::send` returns `Ok` either way, since nothing went wrong at
    // the transport level - so a caller checking only the exit code, not
    // the JSON body, saw every rejection as a success. Reported live by an
    // AGS peer session: it memoizes the last value sent per `srd set` key
    // to avoid re-spawning a process on every option change, and a
    // rejected write counted as a success would have pinned that setting
    // permanently. `{"ok":false,...}` is the one response shape that ever
    // contains a literal `"ok":` field at all - every query command
    // (`clients`/`monitors`/`workspaces`/`keyboard_layout`) returns a
    // differently-shaped body with no such field, so this can't misfire
    // against one of those. A plain substring check, not real JSON
    // parsing, on purpose: `crates/platform/src/ipc.rs`'s `err`/`ok`
    // helpers always emit this exact literal (`serde_json` preserves
    // struct field declaration order), and this binary is deliberately
    // dumb/dependency-free - see this file's own module doc comment.
    if response.contains(r#""ok":false"#) {
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("srd: srdwm's control socket is Unix-only; nothing to connect to on this platform");
    std::process::exit(1);
}

fn build_request(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("clients") => Ok(r#"{"cmd":"clients"}"#.to_string()),
        Some("monitors") => Ok(r#"{"cmd":"monitors"}"#.to_string()),
        Some("workspaces") => Ok(r#"{"cmd":"workspaces"}"#.to_string()),
        Some("settings") => Ok(r#"{"cmd":"settings"}"#.to_string()),
        Some("keyboard") if args.get(1).map(String::as_str) == Some("layout") => Ok(r#"{"cmd":"keyboard_layout"}"#.to_string()),
        Some("keyboard") => Err("did you mean 'srd keyboard layout'?".to_string()),
        Some("subscribe") => Ok(r#"{"cmd":"subscribe"}"#.to_string()),
        Some("dispatch") => build_dispatch(&args[1..]),
        // `srd capture workspace <id> <path> [<width>x<height>]` - an
        // off-screen render of that workspace's window tree, written to
        // `path` as a PPM image. Not under `dispatch`: every `dispatch`
        // action mutates window/workspace state, this only reads it. Sits
        // next to `clients`/`monitors` in spirit, just backend-owned
        // (needs a renderer) rather than answerable from core state alone
        // - see `WindowManager::request_capture_workspace`'s own doc
        // comment for why this exists (a workspace switcher's thumbnail
        // for a workspace that isn't the one currently on screen, which no
        // screencopy protocol can see).
        Some("capture") => {
            if args.get(1).map(String::as_str) != Some("workspace") {
                return Err("'capture' only supports 'workspace' - usage: srd capture workspace <id> <path> [<width>x<height>]".to_string());
            }
            let id: u64 = args.get(2).ok_or("capture needs a workspace id")?.parse().map_err(|_| "id must be a number".to_string())?;
            let path = args.get(3).ok_or("capture needs an output path")?;
            let size = match args.get(4) {
                None => String::new(),
                Some(spec) => {
                    let (w, h) = spec.split_once('x').ok_or("size must look like <width>x<height>")?;
                    let w: u64 = w.parse().map_err(|_| "width must be a number".to_string())?;
                    let h: u64 = h.parse().map_err(|_| "height must be a number".to_string())?;
                    format!(r#","width":{w},"height":{h}"#)
                }
            };
            Ok(format!(r#"{{"cmd":"capture_workspace","id":{id},"path":{path:?}{size}}}"#))
        }
        // Value encoding is picked per key rather than guessed from the
        // string's shape (e.g. treating anything that parses as a number
        // as one): `border_color` is a hex string that happens to start
        // with punctuation, not a bare word, and JSON's `true`/`false`
        // need to land unquoted for the server's `as_bool()` to see them
        // as booleans at all, not a string it then has to reject.
        Some("set") => {
            let key = args.get(1).ok_or(
                "set needs a key (border_width/border_color/corner_radius/gap_inner/gap_outer/shadows/rounded_corners/animations/night_light/reading_mode/phone_mode/multi_cursor/decoration_mode)",
            )?;
            let raw = args.get(2).ok_or("set needs a value")?;
            let value = match key.as_str() {
                "border_width" | "corner_radius" | "gap_inner" | "gap_outer" => {
                    raw.parse::<u64>().map_err(|_| format!("{key} needs a numeric value"))?.to_string()
                }
                "shadows" | "rounded_corners" | "animations" | "night_light" | "reading_mode" | "phone_mode" | "multi_cursor" => match raw.as_str() {
                    "true" | "false" => raw.clone(),
                    _ => return Err(format!("{key} needs 'true' or 'false'")),
                },
                "decoration_mode" => match raw.as_str() {
                    "server" | "client" => format!("{:?}", raw),
                    _ => return Err(format!("{key} needs 'server' or 'client'")),
                },
                "border_color" => format!("{:?}", raw),
                _ => return Err(format!("unknown set key '{key}'")),
            };
            Ok(format!(r#"{{"cmd":"set","key":"{key}","value":{value}}}"#))
        }
        _ => Err(
            "expected 'clients', 'monitors', 'workspaces', 'settings', 'keyboard layout', 'subscribe', 'dispatch <action> <id>', 'capture workspace <id> <path>', or 'set <key> <value>'"
                .to_string(),
        ),
    }
}

/// `dispatch`'s own verb-phrase parser, split out of `build_request` for
/// its own sake - reads like a real command (`toggle floating`, `move
/// window`), not a single joined snake_case token (`toggle_floating`).
/// Live report: a joined identifier reads as an internal detail leaking
/// into the CLI surface, not a real command a person would type.
///
/// `set`'s keys (`corner_radius`, `border_color`, ...) deliberately keep
/// their existing snake_case names, unlike `dispatch`'s actions - they're
/// property/setting names, not verb phrases (the same distinction `git
/// config user.name` or `systemctl set-property CPUQuota=` draw: a key
/// stays an identifier, a command reads like English), and renaming them
/// would break the AGS peer's already-working live Settings panel, which
/// calls `srd set` directly today. `dispatch`'s actions carry no such
/// cost yet - the peer's own integration for these is either brand new
/// (the four just added) or already being migrated to `zwlr-foreign-
/// toplevel` instead of `srd dispatch` entirely, so this is the moment to
/// fix the syntax before more depends on it, not after.
fn build_dispatch(args: &[String]) -> Result<String, String> {
    let verb = args.first().ok_or("dispatch needs an action, e.g. 'focus', 'close', 'toggle maximize', 'move window'")?;
    let usage_hint =
        "expected one of: focus, close, lock, toggle visibility/maximize/fullscreen/floating/pinned, move window/workspace, activate workspace, cycle keyboard layout, pin input, unpin input";
    match verb.as_str() {
        "focus" | "close" => {
            let id: u64 = args.get(1).ok_or("dispatch needs an id")?.parse().map_err(|_| "id must be a number".to_string())?;
            Ok(format!(r#"{{"cmd":"{verb}","id":{id}}}"#))
        }
        // No id - there's only ever one session to lock.
        "lock" => Ok(r#"{"cmd":"lock"}"#.to_string()),
        "toggle" => {
            let noun = args.get(1).ok_or("'toggle' needs a target: visibility, maximize, fullscreen, floating, or pinned")?;
            let cmd = match noun.as_str() {
                "visibility" => "toggle_visibility",
                "maximize" => "toggle_maximize",
                "fullscreen" => "toggle_fullscreen",
                "floating" => "toggle_floating",
                "pinned" => "toggle_pinned",
                _ => return Err(format!("unknown 'toggle' target '{noun}' - {usage_hint}")),
            };
            let id: u64 = args.get(2).ok_or("dispatch needs an id")?.parse().map_err(|_| "id must be a number".to_string())?;
            Ok(format!(r#"{{"cmd":"{cmd}","id":{id}}}"#))
        }
        "move" => {
            let noun = args.get(1).ok_or("'move' needs a target: window or workspace")?;
            let id: u64 = args.get(2).ok_or("dispatch needs an id")?.parse().map_err(|_| "id must be a number".to_string())?;
            match noun.as_str() {
                "window" => {
                    let direction = args.get(3).ok_or("'move window' needs a direction (left/right/up/down)")?;
                    if !matches!(direction.as_str(), "left" | "right" | "up" | "down") {
                        return Err(format!("direction must be one of: left, right, up, down (got '{direction}')"));
                    }
                    Ok(format!(r#"{{"cmd":"move_window","id":{id},"direction":"{direction}"}}"#))
                }
                "workspace" => {
                    let workspace: u64 =
                        args.get(3).ok_or("'move workspace' needs a target workspace")?.parse().map_err(|_| "workspace must be a number".to_string())?;
                    Ok(format!(r#"{{"cmd":"move_to_workspace","id":{id},"workspace":{workspace}}}"#))
                }
                _ => Err(format!("unknown 'move' target '{noun}' - {usage_hint}")),
            }
        }
        "activate" => {
            if args.get(1).map(String::as_str) != Some("workspace") {
                return Err(format!("'activate' only supports 'workspace' - {usage_hint}"));
            }
            let id: u64 = args.get(2).ok_or("dispatch needs an id")?.parse().map_err(|_| "id must be a number".to_string())?;
            Ok(format!(r#"{{"cmd":"activate_workspace","id":{id}}}"#))
        }
        // No id - there's only ever one keyboard as far as this IPC is
        // concerned, unlike every other action here which targets a
        // specific window or workspace.
        "cycle" => {
            if args.get(1).map(String::as_str) != Some("keyboard") || args.get(2).map(String::as_str) != Some("layout") {
                return Err(format!("'cycle' only supports 'keyboard layout' - {usage_hint}"));
            }
            Ok(r#"{"cmd":"cycle_keyboard_layout"}"#.to_string())
        }
        // `srd dispatch set output position <name|id> <x> <y>` - the CLI
        // surface for the IPC layer's own `set_output_position`, which
        // existed before this but had no way to reach it outside a client
        // willing to speak the raw socket itself. `<name|id>` accepts
        // either: try parsing as a plain integer id first (matching every
        // other dispatch target here), and if that fails, pass it through
        // as a monitor name instead - `srd monitors` and `wlr-output-
        // management-v1` both key on name (`eDP-1`), not
        // an arbitrary index, so a caller listing outputs that way
        // shouldn't have to look an id up first just to hand it straight
        // back.
        "set" => {
            if args.get(1).map(String::as_str) != Some("output") {
                return Err(format!("'set' only supports 'output position'/'output enabled'/'output split' - {usage_hint}"));
            }
            let noun = args.get(2).ok_or("'set output' needs a target: position, enabled or split")?;
            let target = args.get(3).ok_or("'set output' needs a monitor name or id")?;
            match noun.as_str() {
                "position" => {
                    let x: i64 = args.get(4).ok_or("'set output position' needs an x coordinate")?.parse().map_err(|_| "x must be a number".to_string())?;
                    let y: i64 = args.get(5).ok_or("'set output position' needs a y coordinate")?.parse().map_err(|_| "y must be a number".to_string())?;
                    match target.parse::<u64>() {
                        Ok(id) => Ok(format!(r#"{{"cmd":"set_output_position","id":{id},"x":{x},"y":{y}}}"#)),
                        Err(_) => Ok(format!(r#"{{"cmd":"set_output_position","name":"{target}","x":{x},"y":{y}}}"#)),
                    }
                }
                "enabled" => {
                    let enabled = match args.get(4).map(String::as_str) {
                        Some("true") => true,
                        Some("false") => false,
                        _ => return Err("'set output enabled' needs true or false".to_string()),
                    };
                    match target.parse::<u64>() {
                        Ok(id) => Ok(format!(r#"{{"cmd":"set_output_enabled","id":{id},"enabled":{enabled}}}"#)),
                        Err(_) => Ok(format!(r#"{{"cmd":"set_output_enabled","name":"{target}","enabled":{enabled}}}"#)),
                    }
                }
                // `srd dispatch set output split <name|id> <parts> [rows|columns]`
                // - the live equivalent of `srd.monitor.split(name, parts,
                // direction)` in Lua config, which previously only ever took
                // effect at config load/reload. `parts <= 1` clears an
                // existing split. `columns` (side-by-side, splitting width)
                // is the default when the direction is omitted, matching the
                // Lua function's own default.
                "split" => {
                    let parts: u64 = args.get(4).ok_or("'set output split' needs a part count")?.parse().map_err(|_| "parts must be a number".to_string())?;
                    let rows = match args.get(5).map(String::as_str) {
                        None | Some("columns") => false,
                        Some("rows") => true,
                        Some(other) => return Err(format!("'set output split' direction must be 'rows' or 'columns', got '{other}'")),
                    };
                    match target.parse::<u64>() {
                        Ok(id) => Ok(format!(r#"{{"cmd":"set_monitor_split","id":{id},"parts":{parts},"rows":{rows}}}"#)),
                        Err(_) => Ok(format!(r#"{{"cmd":"set_monitor_split","name":"{target}","parts":{parts},"rows":{rows}}}"#)),
                    }
                }
                _ => Err(format!("unknown 'set output' target '{noun}' - {usage_hint}")),
            }
        }
        // `srd dispatch create fake-monitor <name> <width>x<height>` /
        // `srd dispatch remove fake-monitor <name>` - a fully virtual
        // `wl_output` with no real hardware behind it. See
        // `crates/wayland/src/udev/virtual_heads.rs`'s own module doc
        // comment for the full design and scope (content is readable via
        // any `zwlr_screencopy_manager_v1` client - `grim -o <name>`,
        // concretely - there is no real display to look at directly).
        "create" | "remove" => {
            if args.get(1).map(String::as_str) != Some("fake-monitor") {
                return Err(format!("'{verb}' only supports 'fake-monitor' - {usage_hint}"));
            }
            let name = args.get(2).ok_or("'fake-monitor' needs a name")?;
            if verb == "remove" {
                return Ok(format!(r#"{{"cmd":"remove_fake_monitor","name":{name:?}}}"#));
            }
            let size = args.get(3).ok_or("'create fake-monitor' needs a <width>x<height>")?;
            let (w, h) = size.split_once('x').ok_or("size must be '<width>x<height>', e.g. 1920x1080")?;
            let width: u32 = w.parse().map_err(|_| "width must be a number".to_string())?;
            let height: u32 = h.parse().map_err(|_| "height must be a number".to_string())?;
            Ok(format!(r#"{{"cmd":"create_fake_monitor","name":{name:?},"width":{width},"height":{height}}}"#))
        }
        // `srd dispatch pin input <pid> <window-id>` / `srd dispatch
        // unpin input <pid>` - the CLI surface for `pin_input`, Phase 2
        // of the multi-cursor plan (`docs/TODO.md`). `<pid>` is the
        // controlling tool's own process id (`std::process::id()` from
        // whichever program created the `zwlr_virtual_pointer_unstable_v1`
        // object to be pinned), not a window or workspace id.
        "pin" | "unpin" => {
            if args.get(1).map(String::as_str) != Some("input") {
                return Err(format!("'{verb}' only supports 'input' - {usage_hint}"));
            }
            let pid: i64 = args.get(2).ok_or("dispatch needs a pid")?.parse().map_err(|_| "pid must be a number".to_string())?;
            if verb == "unpin" {
                return Ok(format!(r#"{{"cmd":"pin_input","pid":{pid}}}"#));
            }
            let id: u64 = args.get(3).ok_or("'pin input' needs a window id")?.parse().map_err(|_| "window id must be a number".to_string())?;
            Ok(format!(r#"{{"cmd":"pin_input","pid":{pid},"id":{id}}}"#))
        }
        _ => Err(format!("unknown dispatch action '{verb}' - {usage_hint}")),
    }
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  srd clients");
    eprintln!("  srd monitors");
    eprintln!("  srd workspaces");
    eprintln!("  srd settings");
    eprintln!("  srd keyboard layout");
    eprintln!("  srd subscribe");
    eprintln!("  srd dispatch focus <id>");
    eprintln!("  srd dispatch lock");
    eprintln!("  srd dispatch close <id>");
    eprintln!("  srd dispatch toggle visibility <id>");
    eprintln!("  srd dispatch toggle maximize <id>");
    eprintln!("  srd dispatch toggle fullscreen <id>");
    eprintln!("  srd dispatch toggle floating <id>");
    eprintln!("  srd dispatch toggle pinned <id>");
    eprintln!("  srd dispatch move window <id> <left|right|up|down>");
    eprintln!("  srd dispatch move workspace <id> <workspace>");
    eprintln!("  srd dispatch activate workspace <id>");
    eprintln!("  srd dispatch cycle keyboard layout");
    eprintln!("  srd dispatch set output position <name|id> <x> <y>");
    eprintln!("  srd dispatch set output enabled <name|id> <true|false>");
    eprintln!("  srd dispatch set output split <name|id> <parts> [rows|columns]");
    eprintln!("  srd dispatch pin input <pid> <window-id>");
    eprintln!("  srd dispatch unpin input <pid>");
    eprintln!("  srd dispatch create fake-monitor <name> <width>x<height>");
    eprintln!("  srd dispatch remove fake-monitor <name>");
    eprintln!("  srd capture workspace <id> <path> [<width>x<height>]");
    eprintln!("  srd set border_width <n>");
    eprintln!("  srd set border_color <#hex>");
    eprintln!("  srd set corner_radius <n>");
    eprintln!("  srd set gap_inner <n>");
    eprintln!("  srd set gap_outer <n>");
    eprintln!("  srd set shadows <true|false>");
    eprintln!("  srd set rounded_corners <true|false>");
    eprintln!("  srd set animations <true|false>");
    eprintln!("  srd set night_light <true|false>");
    eprintln!("  srd set reading_mode <true|false>");
    eprintln!("  srd set phone_mode <true|false>");
    eprintln!("  srd set multi_cursor <true|false>");
    eprintln!("  srd set decoration_mode <server|client>");
}

#[cfg(test)]
mod tests {
    use super::build_request;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn toggle_floating_and_toggle_pinned_read_as_two_words() {
        assert_eq!(build_request(&args(&["dispatch", "toggle", "floating", "42"])).unwrap(), r#"{"cmd":"toggle_floating","id":42}"#);
        assert_eq!(build_request(&args(&["dispatch", "toggle", "pinned", "42"])).unwrap(), r#"{"cmd":"toggle_pinned","id":42}"#);
    }

    #[test]
    fn toggle_with_an_unknown_target_is_rejected_client_side() {
        assert!(build_request(&args(&["dispatch", "toggle", "sideways", "42"])).is_err());
    }

    #[test]
    fn capture_workspace_with_and_without_a_size() {
        assert_eq!(
            build_request(&args(&["capture", "workspace", "1", "/tmp/ws1.ppm"])).unwrap(),
            r#"{"cmd":"capture_workspace","id":1,"path":"/tmp/ws1.ppm"}"#
        );
        assert_eq!(
            build_request(&args(&["capture", "workspace", "1", "/tmp/ws1.ppm", "200x120"])).unwrap(),
            r#"{"cmd":"capture_workspace","id":1,"path":"/tmp/ws1.ppm","width":200,"height":120}"#
        );
        assert!(build_request(&args(&["capture", "workspace", "1", "/tmp/ws1.ppm", "bogus"])).is_err());
        assert!(build_request(&args(&["capture", "workspace", "1"])).is_err(), "missing path must error");
        assert!(build_request(&args(&["capture", "monitor", "1", "/tmp/x.ppm"])).is_err(), "only 'workspace' is a valid capture target");
    }

    #[test]
    fn move_window_needs_a_direction_and_validates_it() {
        assert_eq!(build_request(&args(&["dispatch", "move", "window", "42", "left"])).unwrap(), r#"{"cmd":"move_window","id":42,"direction":"left"}"#);
        assert!(build_request(&args(&["dispatch", "move", "window", "42"])).is_err(), "missing direction must error, not silently omit it");
        assert!(build_request(&args(&["dispatch", "move", "window", "42", "sideways"])).is_err(), "an invalid direction must be rejected client-side");
    }

    #[test]
    fn move_workspace_needs_a_numeric_workspace() {
        assert_eq!(build_request(&args(&["dispatch", "move", "workspace", "42", "2"])).unwrap(), r#"{"cmd":"move_to_workspace","id":42,"workspace":2}"#);
        assert!(build_request(&args(&["dispatch", "move", "workspace", "42"])).is_err(), "missing workspace must error");
        assert!(build_request(&args(&["dispatch", "move", "workspace", "42", "not-a-number"])).is_err());
    }

    #[test]
    fn set_output_position_accepts_a_numeric_id() {
        assert_eq!(
            build_request(&args(&["dispatch", "set", "output", "position", "1", "1920", "0"])).unwrap(),
            r#"{"cmd":"set_output_position","id":1,"x":1920,"y":0}"#
        );
    }

    #[test]
    fn set_output_position_accepts_a_name_when_not_purely_numeric() {
        // `srd monitors`/`wlr-output-management-v1` both key on the real
        // connector name (`HDMI-A-1`), not an arbitrary id - a caller
        // that already has the name shouldn't have to look its id up
        // first just to send it straight back.
        assert_eq!(
            build_request(&args(&["dispatch", "set", "output", "position", "HDMI-A-1", "1920", "0"])).unwrap(),
            r#"{"cmd":"set_output_position","name":"HDMI-A-1","x":1920,"y":0}"#
        );
    }

    #[test]
    fn set_output_position_needs_both_coordinates() {
        assert!(build_request(&args(&["dispatch", "set", "output", "position", "1", "1920"])).is_err(), "missing y must error");
        assert!(build_request(&args(&["dispatch", "set", "output", "position", "1", "not-a-number", "0"])).is_err());
    }

    #[test]
    fn set_multi_cursor_accepts_only_true_or_false() {
        assert_eq!(build_request(&args(&["set", "multi_cursor", "true"])).unwrap(), r#"{"cmd":"set","key":"multi_cursor","value":true}"#);
        assert_eq!(build_request(&args(&["set", "multi_cursor", "false"])).unwrap(), r#"{"cmd":"set","key":"multi_cursor","value":false}"#);
        assert!(build_request(&args(&["set", "multi_cursor", "maybe"])).is_err());
    }

    #[test]
    fn set_phone_mode_accepts_only_true_or_false() {
        assert_eq!(build_request(&args(&["set", "phone_mode", "true"])).unwrap(), r#"{"cmd":"set","key":"phone_mode","value":true}"#);
        assert_eq!(build_request(&args(&["set", "phone_mode", "false"])).unwrap(), r#"{"cmd":"set","key":"phone_mode","value":false}"#);
        assert!(build_request(&args(&["set", "phone_mode", "maybe"])).is_err());
    }

    #[test]
    fn set_output_enabled_accepts_a_numeric_id_and_a_name() {
        assert_eq!(build_request(&args(&["dispatch", "set", "output", "enabled", "1", "false"])).unwrap(), r#"{"cmd":"set_output_enabled","id":1,"enabled":false}"#);
        assert_eq!(
            build_request(&args(&["dispatch", "set", "output", "enabled", "HDMI-A-1", "true"])).unwrap(),
            r#"{"cmd":"set_output_enabled","name":"HDMI-A-1","enabled":true}"#
        );
    }

    #[test]
    fn set_output_split_accepts_a_numeric_id_and_a_name() {
        assert_eq!(build_request(&args(&["dispatch", "set", "output", "split", "1", "2"])).unwrap(), r#"{"cmd":"set_monitor_split","id":1,"parts":2,"rows":false}"#);
        assert_eq!(
            build_request(&args(&["dispatch", "set", "output", "split", "HDMI-A-1", "3", "rows"])).unwrap(),
            r#"{"cmd":"set_monitor_split","name":"HDMI-A-1","parts":3,"rows":true}"#
        );
    }

    #[test]
    fn set_output_split_defaults_direction_to_columns() {
        assert_eq!(build_request(&args(&["dispatch", "set", "output", "split", "eDP-1", "2", "columns"])).unwrap(), r#"{"cmd":"set_monitor_split","name":"eDP-1","parts":2,"rows":false}"#);
    }

    #[test]
    fn set_output_split_rejects_an_unknown_direction() {
        assert!(build_request(&args(&["dispatch", "set", "output", "split", "eDP-1", "2", "sideways"])).is_err());
    }

    #[test]
    fn set_output_split_needs_a_part_count() {
        assert!(build_request(&args(&["dispatch", "set", "output", "split", "eDP-1"])).is_err());
        assert!(build_request(&args(&["dispatch", "set", "output", "split", "eDP-1", "not-a-number"])).is_err());
    }

    #[test]
    fn create_fake_monitor_builds_a_sized_request() {
        assert_eq!(
            build_request(&args(&["dispatch", "create", "fake-monitor", "FAKE-1", "1920x1080"])).unwrap(),
            r#"{"cmd":"create_fake_monitor","name":"FAKE-1","width":1920,"height":1080}"#
        );
    }

    #[test]
    fn remove_fake_monitor_needs_no_size() {
        assert_eq!(build_request(&args(&["dispatch", "remove", "fake-monitor", "FAKE-1"])).unwrap(), r#"{"cmd":"remove_fake_monitor","name":"FAKE-1"}"#);
    }

    #[test]
    fn create_fake_monitor_rejects_a_malformed_size() {
        assert!(build_request(&args(&["dispatch", "create", "fake-monitor", "FAKE-1", "1920"])).is_err());
        assert!(build_request(&args(&["dispatch", "create", "fake-monitor", "FAKE-1", "widexhigh"])).is_err());
    }

    #[test]
    fn pin_input_builds_a_request_with_pid_and_window_id() {
        assert_eq!(build_request(&args(&["dispatch", "pin", "input", "12345", "7"])).unwrap(), r#"{"cmd":"pin_input","pid":12345,"id":7}"#);
    }

    #[test]
    fn unpin_input_builds_a_request_with_no_window_id() {
        assert_eq!(build_request(&args(&["dispatch", "unpin", "input", "12345"])).unwrap(), r#"{"cmd":"pin_input","pid":12345}"#);
    }

    #[test]
    fn pin_input_requires_the_literal_noun_input() {
        assert!(build_request(&args(&["dispatch", "pin", "output", "12345", "7"])).is_err());
    }

    #[test]
    fn pin_input_requires_a_window_id_but_unpin_does_not() {
        assert!(build_request(&args(&["dispatch", "pin", "input", "12345"])).is_err(), "'pin input' needs a window id");
        assert!(build_request(&args(&["dispatch", "unpin", "input", "12345", "7"])).is_ok(), "'unpin input' ignores a trailing window id rather than erroring");
    }

    #[test]
    fn pin_input_rejects_a_non_numeric_pid() {
        assert!(build_request(&args(&["dispatch", "pin", "input", "not-a-number", "7"])).is_err());
    }

    #[test]
    fn set_output_enabled_rejects_anything_but_true_or_false() {
        assert!(build_request(&args(&["dispatch", "set", "output", "enabled", "1", "yes"])).is_err());
        assert!(build_request(&args(&["dispatch", "set", "output", "enabled", "1"])).is_err(), "missing value must error");
    }

    #[test]
    fn lock_needs_no_id() {
        assert_eq!(build_request(&args(&["dispatch", "lock"])).unwrap(), r#"{"cmd":"lock"}"#);
    }

    #[test]
    fn focus_and_close_stay_single_word_verbs() {
        assert_eq!(build_request(&args(&["dispatch", "focus", "42"])).unwrap(), r#"{"cmd":"focus","id":42}"#);
        assert_eq!(build_request(&args(&["dispatch", "close", "42"])).unwrap(), r#"{"cmd":"close","id":42}"#);
    }

    #[test]
    fn activate_workspace_and_cycle_keyboard_layout_read_as_real_phrases() {
        assert_eq!(build_request(&args(&["dispatch", "activate", "workspace", "1"])).unwrap(), r#"{"cmd":"activate_workspace","id":1}"#);
        assert_eq!(build_request(&args(&["dispatch", "cycle", "keyboard", "layout"])).unwrap(), r#"{"cmd":"cycle_keyboard_layout"}"#);
    }

    #[test]
    fn a_joined_snake_case_action_is_no_longer_accepted() {
        // The old single-token spelling must not silently keep working --
        // a caller still using it should get a clear error, not a wrong
        // command reaching the server.
        assert!(build_request(&args(&["dispatch", "toggle_floating", "42"])).is_err());
    }

    #[test]
    fn keyboard_layout_query_reads_as_two_words_too() {
        assert_eq!(build_request(&args(&["keyboard", "layout"])).unwrap(), r#"{"cmd":"keyboard_layout"}"#);
        assert!(build_request(&args(&["keyboard_layout"])).is_err());
    }

    #[test]
    fn set_decoration_mode_accepts_only_server_or_client() {
        assert_eq!(build_request(&args(&["set", "decoration_mode", "server"])).unwrap(), r#"{"cmd":"set","key":"decoration_mode","value":"server"}"#);
        assert_eq!(build_request(&args(&["set", "decoration_mode", "client"])).unwrap(), r#"{"cmd":"set","key":"decoration_mode","value":"client"}"#);
        assert!(build_request(&args(&["set", "decoration_mode", "both"])).is_err());
    }

    #[test]
    fn night_light_and_reading_mode_accept_only_true_or_false() {
        assert_eq!(build_request(&args(&["set", "night_light", "true"])).unwrap(), r#"{"cmd":"set","key":"night_light","value":true}"#);
        assert_eq!(build_request(&args(&["set", "night_light", "false"])).unwrap(), r#"{"cmd":"set","key":"night_light","value":false}"#);
        assert_eq!(build_request(&args(&["set", "reading_mode", "true"])).unwrap(), r#"{"cmd":"set","key":"reading_mode","value":true}"#);
        assert!(build_request(&args(&["set", "night_light", "warm"])).is_err());
    }

    #[test]
    fn settings_query_needs_no_further_arguments() {
        assert_eq!(build_request(&args(&["settings"])).unwrap(), r#"{"cmd":"settings"}"#);
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    /// `<display>` in `srdwm-<display>.sock` is whichever env var the
    /// compositor itself used to name it: `WAYLAND_DISPLAY` (Wayland
    /// backend) or `DISPLAY` (X11 backend) - see `srdwm_platform::
    /// IpcServer::bind`'s callers in `crates/wayland`/`crates/x11` for the
    /// matching naming choice on the server side. Reuses `srdwm_platform::
    /// detect()` rather than re-deriving the same Wayland-vs-X11 decision
    /// a second, potentially-drifting way here.
    fn socket_path() -> Result<PathBuf, String> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or("XDG_RUNTIME_DIR is not set")?;
        let display = match srdwm_platform::detect() {
            srdwm_platform::PlatformKind::X11 => std::env::var("DISPLAY").map_err(|_| "DISPLAY is not set".to_string())?,
            _ => std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string()),
        };
        Ok(dir.join(format!("srdwm-{display}.sock")))
    }

    fn connect() -> Result<UnixStream, String> {
        let path = socket_path()?;
        UnixStream::connect(&path)
            .map_err(|e| format!("can't reach srdwm's control socket at {} ({e}) - is srdwm running, and are WAYLAND_DISPLAY/DISPLAY set correctly?", path.display()))
    }

    pub fn send(request: &str) -> Result<String, String> {
        let mut stream = connect()?;
        stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
        stream.write_all(b"\n").map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
        Ok(line.trim_end().to_string())
    }

    /// `subscribe`'s connection: unlike `send`, this never closes on its
    /// own - the server keeps it open and writes one more line every time
    /// the window list changes, so this prints each one as it arrives
    /// until the process is killed or the compositor closes the socket.
    pub fn stream(request: &str) -> Result<(), String> {
        let mut conn = connect()?;
        conn.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
        conn.write_all(b"\n").map_err(|e| e.to_string())?;
        let reader = BufReader::new(conn);
        for line in reader.lines() {
            match line {
                Ok(line) => println!("{line}"),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
}
