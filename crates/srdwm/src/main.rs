use srdwm_config::Engine;
use srdwm_core::{Event, WindowManager};
use srdwm_platform::{Platform, PlatformKind};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

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

/// Applies the WindowManager's current layout decisions to the real
/// platform: re-tiles the active workspace if needed, then pushes geometry
/// and decoration state for every visible window.
fn sync(wm: &Rc<RefCell<WindowManager>>, platform: &mut dyn Platform) {
    let ws = wm.borrow().current_workspace();
    wm.borrow_mut().arrange_workspace(ws);

    let focused = wm.borrow().focused_id();
    let snapshot: Vec<_> = wm.borrow().visible_windows().cloned().collect();
    for w in snapshot {
        if let Err(e) = platform.apply_geometry(w.id, w.geometry) {
            log::warn!("apply_geometry({}) failed: {e}", w.id);
        }
        if let Err(e) = platform.redraw_decoration(w.id, &w, focused == Some(w.id)) {
            log::warn!("redraw_decoration({}) failed: {e}", w.id);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    log::info!("srdwm starting");

    let wm = Rc::new(RefCell::new(WindowManager::new()));
    let dir = config_dir();
    let engine = Engine::new(wm.clone(), &dir)?;
    match engine.load_init() {
        Ok(()) => log::info!("loaded config from {}", dir.display()),
        Err(e) => log::warn!("no usable config at {} ({e}); running with built-in defaults", dir.display()),
    }
    apply_default_layout(&engine, &wm);
    let running = engine.running_flag();

    let kind = srdwm_platform::detect();
    log::info!("selected platform backend: {}", kind.name());

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

    sync(&wm, platform.as_mut());

    while running.get() {
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
                    if !engine.dispatch_keybinding(&combo) {
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
