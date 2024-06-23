use srdwm_config::Engine;
use srdwm_core::{Event, WindowManager};
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
    let resize_margin = engine.get_f64("general.resize_margin", srdwm_core::RESIZE_MARGIN as f64).max(1.0) as i32;
    let rounded_corners = engine.get_bool("general.rounded_corners", true);

    // Only the three `theme.*` keys with an unambiguous, already-rendered
    // counterpart are wired - see `srdwm_core::ThemeConfig`'s doc comment.
    // `theme.colors.foreground`/`theme.decorations.title_bar.foreground`
    // and `theme.decorations.border.inactive_color` are deliberately left
    // alone: their own shipped defaults ("#eceff4", "#2e3440") don't match
    // what unfocused text/border actually render as today (an accent-
    // dimming scheme, not a second explicit colour), so wiring them in as
    // written would silently change - in `border.inactive_color`'s case,
    // erase - the unfocused appearance for anyone who never touched
    // theme.* at all. A real design decision belongs there, not a guess
    // made in passing while sweeping for dead keys.
    let mut theme = srdwm_core::ThemeConfig::default();
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.decorations.title_bar.background", "#2e3440")) {
        theme.titlebar_bg = rgb;
    }
    let border_width = engine.get_f64("theme.decorations.border.width", 2.0).max(0.0) as u32;
    theme.default_border_width = border_width;
    if let Some(rgb) = srdwm_core::parse_hex_color(&engine.get_string("theme.decorations.border.active_color", "#88c0d0")) {
        theme.default_border_color = rgb;
    }

    let mut wm = wm.borrow_mut();
    wm.tiling.gap_inner = gap;
    wm.tiling.gap_outer = gap;
    wm.animations_enabled = animations;
    wm.animation_duration_ms = duration;
    wm.shadows_enabled = shadows;
    wm.resize_margin = resize_margin;
    wm.rounded_corners_enabled = rounded_corners;
    wm.theme = theme;
    wm.auto_back_and_forth = engine.get_bool("workspace.auto_back_and_forth", false);
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
fn sync(wm: &Rc<RefCell<WindowManager>>, platform: &mut dyn Platform) {
    for id in wm.borrow_mut().take_close_requests() {
        if let Err(e) = platform.close(id) {
            log::warn!("close({id}) failed: {e}");
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
    // already uses (see their doc comments), so calling it here every tick
    // closes the gap for all of those callers at once. Cheap when nothing
    // actually changed - `set_keyboard_focus` early-returns if the target
    // surface is already focused.
    if let Some(id) = focused {
        if let Err(e) = platform.focus(id) {
            log::warn!("focus({id}) failed: {e}");
        }
    }
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

    env_logger::init();
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
        Err(e) => log::warn!("no usable config at {} ({e}); running with built-in defaults", dir.display()),
    }
    apply_workspace_count(&engine, &wm);
    apply_general_settings(&engine, &wm);
    apply_default_layout(&engine, &wm);
    let running = engine.running_flag();

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

    // The platform is fully connected now (for Wayland, `WAYLAND_DISPLAY`
    // was just set by `srdwm_wayland::connect` - see `udev.rs`/`winit.rs`
    // - and for X11, `DISPLAY` was already set by whatever started the X
    // server srdwm connected to). Only past this point does a process
    // `srd.spawn`ed from a `"ready"` handler have a real display socket to
    // inherit and connect to.
    if !engine.dispatch_event("ready") {
        log::debug!("no 'ready' handler registered");
    }

    sync(&wm, platform.as_mut());

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

        let mut dirty = false;
        for event in events {
            match event {
                Event::KeyPress { key_name, modifiers } => {
                    let combo = format!("{modifiers}{key_name}");
                    if combo == srdwm_core::canonicalize_key_combo(RELOAD_COMBO_LITERAL) {
                        match engine.reload() {
                            Ok(()) => log::info!("config reloaded"),
                            Err(e) => log::error!("config reload failed: {e}"),
                        }
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
            sync(&wm, platform.as_mut());
        }
    }

    log::info!("srdwm shutting down");
    Ok(())
}
