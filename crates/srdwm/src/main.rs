use srdwm_config::Engine;
use srdwm_core::{Event, WindowId, WindowManager};
use srdwm_platform::{Platform, PlatformKind};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by `handle_shutdown_signal`, polled once per iteration of the main
/// loop below - see `install_signal_handlers`'s doc comment for why a
/// process-wide `AtomicBool` rather than the loop's own `Rc<Cell<bool>>`
/// (`running`) is what a signal handler is actually allowed to touch.
/// Defined on every platform (the loop below checks it unconditionally) but
/// only ever set to `true` on unix - see `install_signal_handlers`.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Async-signal-safe: the one thing a signal handler may safely do without
/// risking undefined behaviour is set a `std::sync::atomic` flag (a plain
/// `Cell`, or anything that allocates/locks/formats, is not safe to touch
/// here - the handler can interrupt the main thread at literally any
/// instruction, including mid-mutation of a non-atomic value). Actually
/// exiting happens back on the main thread, at the top of the next loop
/// iteration in `main`, once this flag is observed.
#[cfg(unix)]
extern "C" fn handle_shutdown_signal(_signum: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Without this, srdwm had no handler for `SIGTERM`/`SIGINT` at all, so the
/// default disposition (immediate termination, bypassing every Rust `Drop`
/// impl - the language runtime never runs) applied. In practice this means
/// a session manager or `systemd-logind` ending the session the ordinary
/// way - not a crash, not a `kill -9` - still skipped `IpcServer::drop`
/// (`crates/wayland/src/ipc.rs`), leaving `$XDG_RUNTIME_DIR/srdwm-
/// <display>.sock` behind with nothing listening on it. Found live: a peer
/// session's AGS work observed exactly that stale file after a session
/// switch away from srdwm, and flagged (correctly) that a socket file
/// existing proves nothing about whether anything is listening.
/// `srd.quit()`'s existing shutdown path (`running.set(false)`, checked by
/// the main loop) already runs every `Drop` impl correctly when *it*
/// triggers the exit - this just gives an external SIGTERM the same fair
/// chance, instead of pre-empting it. Only `SIGTERM`/`SIGINT`: `SIGKILL`
/// cannot be caught by any process, ever, so a session teardown that
/// escalates straight to that (or waits out no grace period at all) is not
/// fixable from in here.
///
/// Unix only (real POSIX signals) - a no-op on Windows, where `Ctrl+C`
/// handling is a different mechanism (`SetConsoleCtrlHandler`) that
/// `srdwm-windows` is too early-stage to need yet; `SHUTDOWN_REQUESTED`
/// simply never becomes `true` there, so the loop's check below is always
/// cheap and never fires.
#[cfg(unix)]
fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGTERM, handle_shutdown_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, handle_shutdown_signal as *const () as libc::sighandler_t);
        // Every `srd.spawn(...)`/`std::process::Command::spawn` call in this
        // codebase (`fn_spawn` in config/general.rs, scratchpad toggles,
        // screenshot commands, XWayland's own child) fires the process and
        // drops the `Child` handle without ever calling `.wait()` on it --
        // by design, since a compositor's main loop has no business
        // blocking on an arbitrary spawned command. Nothing else was
        // reaping them either, so every one that exited stayed a zombie for
        // the rest of the session: confirmed live via an AGS peer session's
        // own `ps`, six zombies (four different programs, half an hour
        // apart) all parented to this process. Explicitly ignoring SIGCHLD
        // is the standard POSIX fix for exactly "I spawn children I never
        // wait() on and don't care about their exit status" - the kernel
        // reaps them itself the instant they exit, no handler/waitpid loop
        // needed. Harmless to XWayland's own child processes and to
        // anything else this compositor ever spawns for the same reason.
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

/// Reload is handled here, before dispatch, rather than purely through
/// `srd.reload()`'s own Lua-exposed closure: `Engine::reload` clears
/// `key_bindings` before re-running `init.lua`, so a syntax error in the
/// edited config would leave *this exact binding* unreachable too if
/// finding it depended on the just-cleared map - permanently, since
/// nothing would be left to retrigger a reload short of restarting the
/// whole process, which is the one thing this feature exists to avoid
/// needing. Handling the combo directly here means the retry path never
/// depends on the Lua state that broke. Still requires this exact combo to
/// be bound via `srd.bind` in whatever config was loaded at *startup* --
/// that's what gets it into the platform's initial grab/intercept list in
/// the first place (see `Engine::reload`'s own doc comment).
///
/// Written the conventional Super-first way, same as every combo in the
/// shipped config - NOT the canonical Ctrl/Shift/Alt/Mod4 order
/// `format!("{modifiers}{key_name}")` below actually produces from a real
/// keypress (`"Ctrl+Mod4+r"`). This constant used to be compared against
/// that directly and could never match, for the exact reason
/// `srdwm_core::canonicalize_key_combo`'s doc comment describes for
/// `srd.bind` - this is the one place in this codebase that reads combo
/// strings without going through it. Canonicalized below instead of
/// rewriting the literal, so it stays legible as "what you'd type in Lua".
const RELOAD_COMBO_LITERAL: &str = "Mod4+Ctrl+r";

/// How often the config directory is checked for edits when
/// `general.config_reload_on_write` is on. See `config_mtime`.
const CONFIG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Puts a config error in front of the user instead of only in a log they
/// have no reason to be reading.
///
/// A Lua config is a program, so a user breaking it is an ordinary event,
/// not an exceptional one - and the failure is close to silent from the
/// user's side: the compositor keeps running, their edit simply does
/// nothing. Nothing on screen said why. `notify-send` is the same
/// best-effort mechanism `srd.notify` already uses (see `fn_notify`); when
/// no notification daemon is running it falls back to the log, which is no
/// worse than the previous behaviour and never fatal.
///
/// Asked as "what happens when our config fails/user does something wrong
/// which can be expected since lua programmable config".
fn report_config_error(message: &str) {
    log::error!("{message}");
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("notify-send")
            .arg("--urgency=critical")
            .arg("srdwm")
            .arg(message)
            .status();
    }
}

/// The newest modification time across the config directory's own `.lua`
/// files, used to notice an edit and reload without being asked.
///
/// A plain `stat` sweep rather than an inotify/`notify`-crate watch: it
/// needs no new dependency, behaves identically on every platform this
/// project targets (the standing rule is that everything must work
/// everywhere, Windows and macOS included), and cannot leak watch
/// descriptors on a directory that is edited and replaced by an editor
/// writing through a temp file - the common case, and the one inotify
/// watches on individual files famously miss. One directory read of a
/// handful of files, at most once a second, is not a measurable cost next
/// to a compositor frame.
///
/// Non-recursive on purpose: `srd.load("module")` resolves relative to this
/// same directory, so a flat sweep already covers every file a config can
/// pull in without walking arbitrary user directories.
fn config_mtime(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "lua"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

/// Where the Lua config lives: `$SRDWM_CONFIG_PATH`, else
/// `$XDG_CONFIG_HOME/srd`, else `~/.config/srd`.
///
/// Just `srd`, not `srdwm/srd` - the extra level said the same thing twice.
fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("SRDWM_CONFIG_PATH") {
        return PathBuf::from(p);
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("srd");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config/srd");
    }
    PathBuf::from("config/srd")
}

/// Creates however many workspaces `workspace.count` asks for, beyond the
/// single one `WindowManager::new` already starts with.
///
/// `workspace.count` was a validated, defaulted config value that nothing
/// ever actually read: `WindowManager` starts with exactly one workspace,
/// and `switch_workspace`/`move_window_to_workspace` silently no-op
/// against a workspace ID that doesn't exist yet - so every
/// `Mod4+2`..`Mod4+9` binding in the shipped config did nothing at all
/// beyond the first workspace, regardless of what `workspace.count` said.
/// Run *before* `apply_default_layout`, so newly created workspaces get
/// the configured default layout too, not the hardcoded `"dynamic"` they're
/// created with here.
fn apply_workspace_count(engine: &Engine, wm: &Rc<RefCell<WindowManager>>) {
    let count = engine.get_f64("workspace.count", 1.0).max(1.0) as usize;
    // Same dead-config shape as everything else `apply_general_settings`
    // fixes: `workspace.names` was validated/defaulted (a 10-entry list,
    // "1".."9","0") but nothing ever read it - every workspace was always
    // named after its own 1-based index instead, regardless of config.
    // Applied by position (`names[i]` names workspace `i+1`), covering
    // however many entries the list actually has - a shorter list just
    // leaves the remaining workspaces at their default numeric name.
    let names = engine.get("workspace.names").and_then(|v| v.as_list().map(|s| s.to_vec())).unwrap_or_default();
    let mut wm = wm.borrow_mut();
    let existing = wm.workspaces().len();
    for i in existing..count {
        wm.add_workspace((i + 1).to_string(), "dynamic");
    }
    let ids: Vec<_> = wm.workspaces().iter().map(|w| w.id).collect();
    for (id, name) in ids.into_iter().zip(names) {
        wm.rename_workspace(id, name);
    }
    log::info!("{} workspace(s)", wm.workspaces().len());
}

/// Reads `general.window_gap`, `general.animations` and
/// `general.animation_duration` into `WindowManager`.
///
/// Same dead-config bug class as `apply_workspace_count`: all three are
/// validated/defaulted by `crates/config` but nothing ever read them --
/// `WindowManager::new` always started `tiling` from `TilingConfig::default`
/// (hardcoded `gap_inner`/`gap_outer: 8/16`, coincidentally matching
/// `window_gap`'s own default of `8`, which is why a *default* config never
/// exposed the gap) and `animations_enabled`/`animation_duration_ms` at
/// their own hardcoded defaults, ignoring anything `init.lua` set.
fn apply_general_settings(engine: &Engine, wm: &Rc<RefCell<WindowManager>>) {
    let gap = engine.get_f64("general.window_gap", 8.0).max(0.0) as u32;
    let animations = engine.get_bool("general.animations", true);
    let duration = engine.get_f64("general.animation_duration", 200.0).max(0.0) as u32;
    let shadows = engine.get_bool("general.shadows", true);
    let close_focus_follows_workspace = engine.get_bool("general.close_focus_follows_workspace", false);
    let resize_margin = engine.get_f64("general.resize_margin", srdwm_core::RESIZE_MARGIN as f64).max(1.0) as i32;
    // Genuinely absent, not `false`, when the user's config never sets it
    // - see `WindowManager::rounded_corners_enabled`'s doc comment for why
    // this can't just be `get_bool(..., true)` like every other flag here.
    let rounded_corners = engine.get("general.rounded_corners").and_then(|v| v.as_bool());
    let gpu = engine.get_bool("general.gpu", false);
    let phone_mode = engine.get_bool("general.phone_mode", false);
    let multi_cursor = engine.get_bool("general.multi_cursor", false);
    let desktop_icons = engine.get_bool("general.desktop_icons", true);
    let desktop_icons_all_monitors = engine.get_bool("general.desktop_icons_all_monitors", true);
    let reserve_top = engine.get_f64("general.reserve_top", 0.0).max(0.0) as u32;
    let reserve_bottom = engine.get_f64("general.reserve_bottom", 0.0).max(0.0) as u32;
    let reserve_left = engine.get_f64("general.reserve_left", 0.0).max(0.0) as u32;
    let reserve_right = engine.get_f64("general.reserve_right", 0.0).max(0.0) as u32;
    let file_manager = engine.get_string("general.file_manager", "");
    let desktop_icon_single_click = engine.get_bool("general.desktop_icon_single_click", false);
    let terminal = engine.get_string("general.terminal", "");
    let focus_follows_mouse = engine.get_bool("general.focus_follows_mouse", false);
    let auto_raise = engine.get_bool("general.auto_raise", false);

    // `theme.colors.foreground` stays unwired: it has no unambiguous
    // rendered counterpart of its own (nothing currently paints "generic
    // foreground text" outside the titlebar, which has its own two keys
    // below), so wiring it would be a guess at what it should affect.
    //
    // Everything else here has a real, non-destructive default: each key's
    // fallback in `get_string`/`get_f64` below is the exact value
    // `ThemeConfig::default()` already ships, so a config that never
    // touches `theme.*` renders identically to before this function read
    // it at all - these are additions, not behaviour changes.
    let mut theme = srdwm_core::ThemeConfig::default();
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.decorations.title_bar.background", "#2e3440")) {
        theme.titlebar_bg = rgb;
    }
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.decorations.title_bar.foreground_focused", "#88c0d0")) {
        theme.titlebar_fg_focused = rgb;
    }
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.decorations.title_bar.foreground_unfocused", "#4c566a")) {
        theme.titlebar_fg_unfocused = rgb;
    }
    // "center" or "left" (default) - see `ThemeConfig::title_centered`'s
    // own doc comment. Anything other than exactly "center" is treated as
    // "left", the existing default, rather than erroring on a typo.
    theme.title_centered = engine.get_string("theme.decorations.title_bar.text_align", "left") == "center";
    // "left" or "right" (default) - see `ThemeConfig::buttons_left`'s own
    // doc comment. Anything other than exactly "left" is treated as
    // "right", the existing default.
    theme.buttons_left = engine.get_string("theme.decorations.title_bar.button_side", "right") == "left";
    // `"close,minimize,maximize"`-style override for the three buttons'
    // relative order - see `ThemeConfig::button_order`'s own doc
    // comment. Unset (empty string, the default `get_string` fallback
    // here) leaves this project's own two built-in defaults untouched. A
    // set-but-unparseable value (a typo, a missing button, a repeat) logs
    // and falls back the same way rather than silently hiding a button.
    let button_order_str = engine.get_string("theme.decorations.title_bar.button_order", "");
    theme.button_order = if button_order_str.is_empty() {
        None
    } else {
        let parsed = srdwm_core::parse_button_order(&button_order_str);
        if parsed.is_none() {
            log::warn!("theme.decorations.title_bar.button_order = '{button_order_str}' is not a valid ordering of close, minimize, maximize; keeping the built-in default");
        }
        parsed
    };
    // "hover" (default, classic macOS: glyph hidden until hovered, then
    // animates in) or "always" (modern GNOME/Adwaita: glyph always
    // visible) - see `ThemeConfig::button_glyph_always`'s own doc
    // comment for the research behind offering both.
    theme.button_glyph_always = engine.get_string("theme.decorations.title_bar.button_glyph", "hover") == "always";
    // "traffic_lights" (default: filled, coloured macOS-style dots) or
    // "traditional" (plain glyphs straight on the titlebar background,
    // square maximize, no fill) - see `ThemeConfig::traffic_light_buttons`'s
    // own doc comment. Anything other than exactly "traditional" keeps the
    // traffic-light default, same fallback shape as every other string
    // switch above.
    theme.traffic_light_buttons = engine.get_string("theme.decorations.title_bar.button_style", "traffic_lights") != "traditional";
    // "dynamic" (default: only the buttons the window can actually use) or
    // "fixed" (always the full set) - see `ThemeConfig::dynamic_buttons`.
    theme.dynamic_buttons = engine.get_string("theme.decorations.title_bar.button_mode", "dynamic") != "fixed";
    let border_width = engine.get_f64("theme.decorations.border.width", 2.0).max(0.0) as u32;
    theme.default_border_width = border_width;
    // 12, not the original 6: matches real macOS's own ~0.36 radius-to-
    // titlebar-height proportion (docs/TODO.md's macOS-comparison research)
    // against `TITLEBAR_HEIGHT = 32` - see that constant's own doc comment
    // for where that value comes from.
    let corner_radius = engine.get_f64("theme.decorations.border.radius", 12.0).max(0.0) as u32;
    theme.default_corner_radius = corner_radius;
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.decorations.border.active_color", "#88c0d0")) {
        theme.default_border_color = rgb;
    }
    // A *factor* applied to `border.active_color`, not a second explicit
    // colour - `border.inactive_color` still doesn't exist as a config
    // key, because an absolute override would erase the dimming scheme
    // instead of participating in it (see `ThemeConfig::border_inactive_
    // dim`'s doc comment). `1.0` keeps unfocused identical to focused;
    // `0.0` fades it to black.
    theme.border_inactive_dim = engine.get_f64("theme.decorations.border.inactive_dim", theme.border_inactive_dim as f64).clamp(0.0, 1.0) as f32;
    // "server" (srdwm draws the titlebar, Windows/macOS-style) or "client"
    // (srdwm steps back for anything with no decoration opinion of its
    // own, GNOME-style) - see `ThemeConfig::default_decorated`'s own doc
    // comment. Anything other than exactly "client" is treated as
    // "server", the existing default, rather than erroring on a typo.
    theme.default_decorated = engine.get_string("theme.decorations.default_mode", "server") != "client";

    // `theme.lock.*` - srdwm's own session-lock UI (`crates/wayland/src/
    // native_lock.rs`), configured the same "start from ThemeConfig-style
    // Nord defaults, override per key" way as everything above.
    let mut lock = srdwm_core::LockConfig::default();
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.lock.box_bg", "#2e3440")) {
        lock.box_bg = rgb;
    }
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.lock.box_border", "#88c0d0")) {
        lock.box_border = rgb;
    }
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.lock.text_color", "#eceff4")) {
        lock.text_color = rgb;
    }
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.lock.error_color", "#bf616a")) {
        lock.error_color = rgb;
    }
    lock.corner_radius = engine.get_f64("theme.lock.corner_radius", lock.corner_radius as f64).max(0.0) as u32;
    lock.blur_radius = engine.get_f64("theme.lock.blur_radius", lock.blur_radius as f64).max(0.0) as u32;
    lock.show_caps_lock = engine.get_bool("theme.lock.show_caps_lock", lock.show_caps_lock);
    lock.show_failed_attempts = engine.get_bool("theme.lock.show_failed_attempts", lock.show_failed_attempts);
    let fail_message = engine.get_string("theme.lock.fail_message", &lock.fail_message);
    lock.fail_message = fail_message;
    // A single character, not a string - silently keeping the default
    // rather than erroring is deliberate here (same "malformed value falls
    // back to default rather than erroring a second time" stance `parse_
    // hex_color` callers already take above), since a multi-character
    // "dot" would misalign the whole password row's width calculations,
    // which assume one glyph per typed character.
    if let Some(ch) = engine.get_string("theme.lock.dot_char", &lock.dot_char.to_string()).chars().next() {
        lock.dot_char = ch;
    }
    lock.show_clock = engine.get_bool("theme.lock.show_clock", lock.show_clock);
    lock.show_keyboard = engine.get_bool("theme.lock.show_keyboard", lock.show_keyboard);
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.lock.avatar_bg", "#88c0d0")) {
        lock.avatar_bg = rgb;
    }

    let mut wm = wm.borrow_mut();
    wm.tiling.gap_inner = gap;
    wm.tiling.gap_outer = gap;
    wm.animations_enabled = animations;
    wm.animation_duration_ms = duration;
    wm.shadows_enabled = shadows;
    wm.close_focus_follows_workspace = close_focus_follows_workspace;
    wm.resize_margin = resize_margin;
    wm.rounded_corners_enabled = rounded_corners;
    wm.gpu_enabled = gpu;
    wm.phone_mode = phone_mode;
    wm.multi_cursor_enabled = multi_cursor;
    wm.desktop_icons_enabled = desktop_icons;
    wm.desktop_icons_all_monitors = desktop_icons_all_monitors;
    wm.reserve_top = reserve_top;
    wm.reserve_bottom = reserve_bottom;
    wm.reserve_left = reserve_left;
    wm.reserve_right = reserve_right;
    wm.file_manager = file_manager;
    wm.desktop_icon_single_click = desktop_icon_single_click;
    wm.terminal = terminal;
    wm.focus_follows_mouse = focus_follows_mouse;
    wm.auto_raise = auto_raise;
    wm.theme = theme;
    wm.lock = lock;
    wm.auto_back_and_forth = engine.get_bool("workspace.auto_back_and_forth", false);
    // `false` (the default) keeps srdwm's original single-shared-workspace
    // design exactly as it always was; `true` switches to Hyprland/niri-
    // style independent per-monitor workspace sets. See `WindowManager::
    // per_monitor_workspaces`'s own doc comment for what this actually
    // changes.
    wm.per_monitor_workspaces = engine.get_bool("workspace.per_monitor", false);
    // Validated/defaulted since the config engine's beginning but never
    // read anywhere until now - see `WindowManager::primary_layout`'s own
    // doc comment for what actually applies them (a no-op outside
    // `workspace.per_monitor` mode).
    wm.primary_layout = engine.get_string("monitor.primary_layout", "");
    wm.secondary_layout = engine.get_string("monitor.secondary_layout", "");
}

/// Applies `general.default_layout` to the workspaces that exist at
/// startup.
///
/// srdwm is dynamic-first, not a tiling WM: the built-in default is
/// `"dynamic"`, where windows keep whatever geometry they have and only
/// *new* ones are positioned (by `SmartPlacement`, with Windows-style
/// drag-to-edge snapping). Tiling is one opt-in layout among several.
///
/// Run *after* `load_init`, so a config that sets the key takes effect;
/// `srd.layout.set()` is the separate runtime switch (the shipped config
/// only calls it from key bindings), so this does not fight with it.
fn apply_default_layout(engine: &Engine, wm: &Rc<RefCell<WindowManager>>) {
    let name = engine.get_string("general.default_layout", "dynamic");
    let mut wm = wm.borrow_mut();
    if !wm.available_layouts().iter().any(|l| *l == name) {
        log::warn!("general.default_layout = '{name}' is not a registered layout; keeping built-in default");
        return;
    }
    let ids: Vec<_> = wm.workspaces().iter().map(|w| w.id).collect();
    for id in ids {
        wm.set_layout(id, name.clone());
    }
    log::info!("default layout: {name}");
}

fn print_usage() {
    eprintln!("Usage: srdwm [--wayland | --x11] [--version] [--help]");
    eprintln!();
    eprintln!("With neither flag, srdwm always starts as Wayland (its DRM/udev backend");
    eprintln!("on a bare tty, or nested under a host Wayland/X11 session otherwise).");
    eprintln!("--x11 connects to an already-running X server (DISPLAY must be set);");
    eprintln!("srdwm never starts one itself.");
}

/// Reads `--wayland`/`--x11` out of the process argv, so the caller (a
/// session manager, `startx`, ...) picks the backend explicitly instead of
/// srdwm guessing it from `DISPLAY`/`WAYLAND_DISPLAY`. `--help`/`--version`
/// exit the process directly, the same as any other CLI tool - there's no
/// reason to run the compositor past either.
///
/// Returns `Ok(None)` when neither flag was given, meaning "use the
/// platform default" (see `default_platform_kind`).
fn parse_backend_flag(args: &[String]) -> Result<Option<PlatformKind>, String> {
    let mut requested = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--version" => {
                println!("srdwm {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--wayland" => requested = Some(PlatformKind::Wayland),
            "--x11" => requested = Some(PlatformKind::X11),
            other => return Err(format!("unrecognized argument '{other}'")),
        }
    }
    Ok(requested)
}

/// The backend used when neither `--wayland` nor `--x11` is given: always
/// Wayland on unix (its own DRM/udev backend on a bare tty, or the nested winit backend
/// under a host session - see `srdwm_wayland::connect`), the compile-time
/// native backend on Windows/macOS. Previously this fell to
/// `srdwm_platform::detect()`, which inferred X11 from a stray `DISPLAY`
/// env var; that made backend choice implicit and depend on ambient shell
/// state instead of what was actually asked for. `detect()` still exists
/// (and is still tested) for callers that want that heuristic, but the
/// binary no longer uses it as its default.
fn default_platform_kind() -> PlatformKind {
    #[cfg(target_os = "windows")]
    {
        return PlatformKind::Windows;
    }
    #[cfg(target_os = "macos")]
    {
        return PlatformKind::MacOS;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PlatformKind::Wayland
    }
}

/// Applies the WindowManager's current layout decisions to the real
/// platform: re-tiles the active workspace if needed, then pushes geometry
/// and decoration state for every visible window - and, just as
/// importantly, hides every window that *isn't* currently visible.
///
/// That second half didn't happen at all until this was written: switching
/// workspace only ever updated `WindowManager::current_workspace` and
/// `visible_windows()`'s own filter - nothing downstream of that ever told
/// the platform to unmap a window that just fell off the active workspace,
/// or remap one that just became active. So `srd.workspace.switch()` always
/// "worked" internally, but on screen nothing ever changed: windows from
/// every workspace stayed mapped and visible forever, all sharing the same
/// screen. Every window not currently visible is explicitly hidden via
/// `Platform::minimize` (a pure backend-level map/unmap primitive, entirely
/// separate from `Window.minimized` - see its own doc comment - so this
/// can't be confused with the user's own minimize state); every visible one
/// is explicitly shown via `Platform::restore` before geometry/decoration
/// are pushed, since on X11 `apply_geometry` only reconfigures an existing
/// mapping, it doesn't create one.
fn sync(wm: &Rc<RefCell<WindowManager>>, platform: &mut dyn Platform, last_synced_focus: &mut Option<WindowId>) {
    for id in wm.borrow_mut().take_close_requests() {
        if let Err(e) = platform.close(id) {
            log::warn!("close({id}) failed: {e}");
        }
    }

    // Same "core records the intent, this loop forwards it to the platform
    // that can actually act on it" shape as `close_requests` above - a
    // count rather than a single flag, so more than one `srd dispatch
    // cycle_keyboard_layout` in one tick isn't silently collapsed into one
    // real cycle.
    for _ in 0..wm.borrow_mut().take_keyboard_layout_cycle_requests() {
        match platform.cycle_keyboard_layout() {
            Ok(name) => wm.borrow_mut().set_keyboard_layout(name),
            Err(e) => log::warn!("cycle_keyboard_layout() failed: {e}"),
        }
    }

    let ws = wm.borrow().current_workspace();
    wm.borrow_mut().arrange_workspace(ws);

    let focused = wm.borrow().focused_id();
    // `Platform::focus` was never actually called from anywhere in this
    // loop before - every focus-changing path that isn't a direct mouse
    // click (`srd dispatch focus`/`toggle_visibility`, `srd.window.focus()`/
    // `.next()`/`.prev()`, the scratchpad feature) only ever touched
    // `WindowManager`'s own bookkeeping, since none of those callers can
    // reach `CompState`/real Wayland focus themselves (`crates/platform`'s
    // `IpcServer` in particular has no way to). A window could render as
    // focused (border/titlebar colour already reads live core state) while
    // real keyboard input kept going wherever it was before, and on X11,
    // `_NET_ACTIVE_WINDOW` never moved - confirmed live: `srd dispatch
    // focus` on an XWayland window left it at `0x0`. `Platform::focus`'s
    // own impls now go through the same real-focus-sync path a mouse click
    // already uses (see their doc comments), so calling it here every dirty
    // tick closes the gap for all of those callers at once.
    //
    // Gated on the focused id actually having *changed* since the last
    // call, not just "call every time, it's cheap when nothing changed" as
    // originally reasoned - that reasoning covered `set_keyboard_focus`
    // alone (a real early-return-if-already-focused no-op) but missed that
    // `focus_window` (core) has its own side effect of switching to the
    // focused window's workspace if it differs from the current one. Live-
    // reproduced this session: `srd dispatch activate_workspace` changed
    // `current_workspace` correctly, and within the same dirty tick this
    // unconditional re-assertion of the still-focused (unchanged, still on
    // the *old* workspace) window's focus saw that mismatch and switched
    // straight back - every workspace switch with no accompanying focus
    // change silently undid itself within milliseconds, confirmed via the
    // core diagnostic logging two `switch_workspace` calls a few
    // milliseconds apart, the second putting it right back where it
    // started. This gate alone wasn't the complete fix, though - see the
    // unconditional update just below for the other half.
    if focused != *last_synced_focus {
        if let Some(id) = focused {
            if let Err(e) = platform.focus(id) {
                log::warn!("focus({id}) failed: {e}");
            }
        }
    }
    // Unconditional, not just inside the block above: a real mouse click
    // changes `wm.focused_id()` through a completely separate, synchronous
    // path (`input::focus_window`, called directly from the click handler)
    // that never touches `last_synced_focus` at all. Updating it only when
    // *this* function was the one to act on a change meant the very first
    // real click after startup left it permanently stale - every dirty
    // tick from then on saw `focused != *last_synced_focus` (comparing the
    // click's real, current value against this now-ancient one), treated
    // it as a fresh genuine change, and called `platform.focus()` again
    // for a focus that hadn't actually moved since the last tick.
    // `focus_window`'s own workspace-follow side effect then fired on that
    // spurious re-assertion and switched back to the still-focused
    // window's own (unchanged) workspace - silently reverting any
    // `activate_workspace` IPC call within roughly a millisecond. Reported
    // live as `srd dispatch activate workspace` having no visible effect;
    // confirmed via temporary diagnostic logging in `switch_workspace`/
    // `focus_window` showing exactly this switch-then-immediate-revert
    // pair. This line now runs every tick regardless, so `last_synced_
    // focus` always reflects the real current focus by the time this
    // function returns - genuine changes (from any source: click, IPC,
    // Alt-Tab) are still caught by the comparison above, but a tick where
    // nothing changed can no longer be mistaken for one where it did.
    *last_synced_focus = focused;
    let (visible, hidden) = {
        let wm = wm.borrow();
        // Bottom-to-top stacking order, not `visible_windows()`'s arbitrary
        // `HashMap` iteration order. `Platform::apply_geometry`/
        // `redraw_decoration` both end up calling `CompState::sync_geometry`
        // on the Wayland backends, which - as a documented side effect of
        // smithay's own `Space::map_element` - re-raises whichever window
        // it's called for to the top of `Space`'s real render order. With
        // an arbitrary iteration order, EVERY call to this function (i.e.
        // on essentially any dirty event: a keystroke, a resize frame, a
        // workspace poll) re-shuffled every visible window's on-screen
        // z-order to whatever `HashMap` happened to yield that tick,
        // completely unrelated to which window was actually focused --
        // reported live as focus never visibly "sticking" to a window,
        // since within a frame or two of a real focus change, the next
        // `sync()` tick's arbitrary-order pass silently raised some other
        // window back over it. Iterating bottom-to-top instead means each
        // pass's cascade of re-raises ends, deterministically, with the
        // true topmost window raised last - restoring the same order it
        // started with instead of scrambling it.
        let mut visible: Vec<srdwm_core::Window> = wm.visible_windows_front_to_back().cloned().collect();
        visible.reverse();
        let visible_ids: std::collections::HashSet<_> = visible.iter().map(|w| w.id).collect();
        let hidden: Vec<_> = wm.windows().filter(|w| !visible_ids.contains(&w.id)).map(|w| w.id).collect();
        (visible, hidden)
    };

    for id in hidden {
        if let Err(e) = platform.minimize(id) {
            log::warn!("hiding window {id} (not on the active workspace) failed: {e}");
        }
    }

    for w in visible {
        if let Err(e) = platform.restore(w.id) {
            log::warn!("showing window {} failed: {e}", w.id);
        }
        if let Err(e) = platform.apply_geometry(w.id, w.geometry) {
            log::warn!("apply_geometry({}) failed: {e}", w.id);
        }
        if let Err(e) = platform.redraw_decoration(w.id, &w, focused == Some(w.id)) {
            log::warn!("redraw_decoration({}) failed: {e}", w.id);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_signal_handlers();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let requested_backend = match parse_backend_flag(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("srdwm: {e}");
            print_usage();
            std::process::exit(2);
        }
    };

    // `log::*` throughout this codebase and `tracing::*` inside smithay
    // (its own buffer-import failures, e.g. "Failed to import surface" /
    // "Unknown buffer format for", are emitted via `tracing::warn!`/
    // `error!`, not `log::`) are two independent facades - plain
    // `env_logger::init()` only ever installed a `log` backend, so every
    // smithay-side diagnostic was silently dropped with no subscriber to
    // receive it. `tracing_subscriber::fmt()` bridges `log::*` calls into
    // `tracing` itself (its default `tracing-log` feature installs that
    // bridge as part of `init()`) and then prints both, still filtered by
    // the same `RUST_LOG` directive syntax `env_logger` used - a separate,
    // explicit `tracing_log::LogTracer::init()` call here crashed srdwm on
    // every single startup (`SetLoggerError` - `log::set_boxed_logger` can
    // only ever succeed once process-wide, and `init()` below already
    // claims it) instead of merely being redundant with it, since `init()`
    // internally unwraps that install rather than tolerating it being taken
    // already. Confirmed live: this is what put srdwm into a boot loop.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    log::info!("srdwm starting");

    // Decided before the config loads (it needs no engine, just argv/the
    // compile target) so `platform.backend`/`platform.os` below are correct
    // by the time `init.lua` - and anything it `srd.load`s - runs, and
    // config can branch on them (`if srd.get("platform.backend") == "x11"
    // then ... end`). This is the one thing srdwm is cross-platform-first
    // about in `main.rs`: Windows/macOS builds compute the same two keys
    // from their own compile-time target, so config written against them
    // doesn't need a different branching mechanism per OS.
    let kind = requested_backend.unwrap_or_else(default_platform_kind);
    log::info!("selected platform backend: {}", kind.name());

    let wm = Rc::new(RefCell::new(WindowManager::new()));
    let dir = config_dir();
    let engine = Engine::new(wm.clone(), &dir)?;
    engine.set_string("platform.backend", kind.name());
    engine.set_string("platform.os", std::env::consts::OS);
    match engine.load_init() {
        Ok(()) => log::info!("loaded config from {}", dir.display()),
        Err(e) => {
            log::warn!("no usable config at {} ({e}); running with built-in defaults", dir.display());
            report_config_error(&format!("Config failed to load, using built-in defaults.\n{e}"));
        }
    }
    apply_workspace_count(&engine, &wm);
    apply_general_settings(&engine, &wm);
    apply_default_layout(&engine, &wm);
    let running = engine.running_flag();
    // `general.config_reload_on_write` - on by default. A programmable
    // config is edited far more often than it is reloaded deliberately, and
    // a failed reload can no longer leave the session in a broken state
    // (`do_reload` restores the previous working config), so the safe
    // default is the convenient one. Set it false for a config that
    // deliberately does expensive work at load time.
    let reload_on_write = engine.get_bool("general.config_reload_on_write", true);
    let mut last_config_mtime = config_mtime(&dir);
    let mut last_config_poll = std::time::Instant::now();

    let mut platform: Box<dyn Platform> = match kind {
        #[cfg(all(unix, not(target_os = "macos")))]
        PlatformKind::X11 => {
            let mut p = srdwm_x11::X11Platform::connect(wm.clone())?;
            let combos = engine.bound_keys();
            p.grab_keybindings(&combos)?;
            log::info!("grabbed {} keybinding(s)", combos.len());
            Box::new(p)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        PlatformKind::Wayland => {
            let combos = engine.bound_keys();
            log::info!("{} keybinding(s) will be intercepted from clients", combos.len());
            srdwm_wayland::connect(wm.clone(), &combos, &engine.repeat_keys())?
        }
        #[cfg(windows)]
        PlatformKind::Windows => Box::new(srdwm_windows::WindowsPlatform::new()?),
        #[cfg(target_os = "macos")]
        PlatformKind::MacOS => Box::new(srdwm_macos::MacOsPlatform::new()?),
        #[allow(unreachable_patterns)]
        other => return Err(format!("platform backend '{}' is not available on this build", other.name()).into()),
    };

    let monitors = platform.monitors()?;
    log::info!("detected {} monitor(s)", monitors.len());
    wm.borrow_mut().set_monitors(monitors);

    // Read once so `WindowManager::keyboard_layout` (surfaced over `srd`,
    // for an AGS peer session's keyboard-layout badge) has a real value
    // from the start rather than an empty string until the first real
    // cycle. `Err` here just means this backend has no real XKB-backed
    // seat to ask (X11, Windows, macOS) - not worth failing startup over,
    // same as any other platform capability that's honestly unsupported.
    match platform.keyboard_layout() {
        Ok(name) => wm.borrow_mut().set_keyboard_layout(name),
        Err(e) => log::debug!("keyboard_layout() unavailable on this backend: {e}"),
    }

    // The platform is fully connected now (for Wayland, `WAYLAND_DISPLAY`
    // was just set by `srdwm_wayland::connect` - see `udev`/`winit`
    // - and for X11, `DISPLAY` was already set by whatever started the X
    // server srdwm connected to). Only past this point does a process
    // `srd.spawn`ed from a `"ready"` handler have a real display socket to
    // inherit and connect to.
    if !engine.dispatch_event("ready") {
        log::debug!("no 'ready' handler registered");
    }

    let mut last_synced_focus: Option<WindowId> = None;
    sync(&wm, platform.as_mut(), &mut last_synced_focus);

    while running.get() {
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            // Routed through the same flag `srd.quit()` already sets,
            // rather than breaking directly - so a `SIGTERM`/`SIGINT`
            // shutdown runs through exactly the same path (and any future
            // logic added to it) as a normal quit, not a second one.
            running.set(false);
            continue;
        }
        let events = match platform.poll_events() {
            Ok(events) => events,
            Err(e) => {
                log::error!("poll_events failed: {e}");
                break;
            }
        };

        // `general.config_reload_on_write`: notice an edited config and
        // apply it without the user having to press the reload combo.
        //
        // Polled at most once a second (see `config_mtime`), and only when
        // the newest `.lua` mtime has actually moved - so the steady-state
        // cost is one directory read per second and nothing else. A failed
        // reload here is not fatal and does not spam: `do_reload` puts the
        // previous working config back, and the mtime has already been
        // recorded, so a file that stays broken is reported once, not on
        // every tick. Saving a fixed version moves the mtime again and
        // reloads for real.
        if reload_on_write {
            let now = std::time::Instant::now();
            if now.duration_since(last_config_poll) >= CONFIG_POLL_INTERVAL {
                last_config_poll = now;
                let current = config_mtime(&dir);
                if current.is_some() && current != last_config_mtime {
                    last_config_mtime = current;
                    match engine.reload() {
                        Ok(()) => log::info!("config reloaded (file changed on disk)"),
                        Err(e) => report_config_error(&format!("Config edit not applied, keeping the last working one.\n{e}")),
                    }
                    apply_general_settings(&engine, &wm);
                    // After `apply_general_settings`, which rebuilds the
                    // theme from the config file - see
                    // `WindowManager::live_settings` for why a hand-made
                    // change has to win over that on a *reload*, even
                    // though the file wins at startup.
                    let replayed = srdwm_platform::replay_live_settings(&wm);
                    if replayed > 0 {
                        log::info!("re-applied {replayed} live setting(s) after the reload");
                    }
                }
            }
        }
        // The desktop menu's "Refresh" row, drained here rather than in a
        // backend: reloading Lua and firing a Lua handler both need the
        // `Engine`, which only this loop owns. See
        // `WindowManager::request_refresh`.
        if wm.borrow_mut().drain_refresh_request() {
            match engine.reload() {
                Ok(()) => log::info!("config reloaded (desktop refresh)"),
                Err(e) => report_config_error(&format!("Config reload failed, keeping the last working one.\n{e}")),
            }
            apply_general_settings(&engine, &wm);
            srdwm_platform::replay_live_settings(&wm);
            // After the reload, so a handler edited in the config since
            // startup is the one that runs.
            engine.dispatch_event("refresh");
        }

        let mut dirty = false;
        for event in events {
            match event {
                Event::KeyPress { key_name, modifiers } => {
                    let combo = format!("{modifiers}{key_name}");
                    if combo == srdwm_core::canonicalize_key_combo(RELOAD_COMBO_LITERAL) {
                        match engine.reload() {
                            Ok(()) => log::info!("config reloaded"),
                            Err(e) => report_config_error(&format!("Config reload failed, keeping the last working one.\n{e}")),
                        }
                        apply_general_settings(&engine, &wm);
                        srdwm_platform::replay_live_settings(&wm);
                    } else if !engine.dispatch_keybinding(&combo) {
                        log::debug!("no binding for '{combo}'");
                    }
                    dirty = true;
                }
                Event::WindowCreated(id) => {
                    log::info!("window {id} created");
                    dirty = true;
                }
                Event::WindowDestroyed(id) => {
                    log::info!("window {id} destroyed");
                    dirty = true;
                }
                Event::WindowMoved { .. } | Event::WindowResized { .. } => dirty = true,
                // `CoreEvent::WindowFocused` existed as a variant already
                // (pushed by `input.rs`'s `focus_window` on every real
                // focus change) but had no arm here at all - it fell
                // through to the catch-all below, which does not set
                // `dirty`. A focus change that doesn't also move, resize,
                // create, or destroy a window (the common case: clicking a
                // second, already-visible window) never reached `sync()`
                // at all through this loop, so nothing here ever gave the
                // border/titlebar recolor a second chance if the direct,
                // synchronous `set_window_activated` path inside the click
                // handler itself didn't already catch it. Reported live as
                // a window's border staying stuck at whatever focus state
                // it last happened to redraw with, in either direction,
                // regardless of which window was actually focused now.
                Event::WindowFocused(_) => dirty = true,
                Event::WorkspaceChanged => dirty = true,
                // Laptop lid. The handler is a plain Lua function, so the
                // config decides what to do (lock, suspend, nothing).
                Event::LidSwitch { closed } => {
                    let name = if closed { "lid_closed" } else { "lid_open" };
                    if !engine.dispatch_event(name) {
                        log::debug!("no handler registered for '{name}'");
                    }
                }
                // A monitor was plugged in or unplugged. Re-query the whole
                // list rather than applying the single monitor in the event:
                // outputs are laid out left-to-right, so adding or removing
                // one shifts the positions of the others too.
                Event::MonitorAdded(_) | Event::MonitorRemoved(_) => {
                    match platform.monitors() {
                        Ok(monitors) => {
                            log::info!("monitor layout changed: {} monitor(s)", monitors.len());
                            wm.borrow_mut().set_monitors(monitors);
                        }
                        Err(e) => log::warn!("failed to re-query monitors after hotplug: {e}"),
                    }
                    dirty = true;
                }
                _ => {}
            }
        }
        if dirty {
            sync(&wm, platform.as_mut(), &mut last_synced_focus);
        }
    }

    log::info!("srdwm shutting down");
    Ok(())
}
