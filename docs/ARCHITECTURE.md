# Architecture

## Crate layout

```
crates/
  core/      srdwm-core      window/workspace/monitor state, layout engine,
                              smart placement, hit-testing - pure logic,
                              no I/O, no platform dependency
  config/    srdwm-config    Lua (`srd`) scripting engine, wraps mlua
  platform/  srdwm-platform  the `Platform` trait + PlatformKind detection
  x11/       srdwm-x11       X11 backend (x11rb)
  wayland/   srdwm-wayland   Wayland backend (smithay)
  windows/   srdwm-windows   Windows backend (windows-rs, cfg-gated)
  macos/     srdwm-macos     macOS backend (core-graphics/accessibility-sys, cfg-gated)
  srdwm/     srdwm (bin)     wires config + core + platform together
```

Dependency direction is strictly one-way: `core` depends on nothing else in
the workspace; `platform` depends only on `core`; each backend depends on
`core` + `platform`; `config` depends only on `core` (it never talks to a
platform directly - see below); the `srdwm` binary is the only crate that
depends on everything.

## Why config never touches a platform directly

`srdwm-config`'s `srd.window.close()` (etc.) mutates a
`Rc<RefCell<WindowManager>>` shared with the running backend - it does not
call into `srdwm-x11` or `srdwm-wayland` itself. The backend is the thing
that notices the `WindowManager`'s state changed (on its next `poll_events`
tick, via `main.rs`'s `sync()` helper) and pushes the resulting geometry to
the real X11/Wayland surface.

This indirection is deliberate: it's what let the exact same `srd` API
implementation, with the exact same test suite, work correctly against a
`WindowManager` in isolation (10 config tests never touch a display server)
and then, unmodified, drive a real X11 session and a real Wayland
compositor. If `srd.window.close()` called `platform.close()` directly, the
config crate would need a generic `Platform` handle and every test would
need a fake one.

## The `Platform` trait

```rust
trait Platform {
    fn kind(&self) -> PlatformKind;
    fn poll_events(&mut self) -> Result<Vec<srdwm_core::Event>>;
    fn monitors(&mut self) -> Result<Vec<Monitor>>;
    fn apply_geometry(&mut self, window: WindowId, geometry: Rect) -> Result<()>;
    fn set_title(&mut self, ...) / focus / minimize / restore / close (...);
    fn set_decorated / set_border_color / set_border_width / redraw_decoration (...);
    fn grab_keyboard(&mut self) / ungrab_keyboard(&mut self);
}
```

`poll_events` is the one place each backend bridges its native event model
(X11's blocking `XNextEvent`, Wayland's callback-driven `wl_display`
dispatch) into the common `srdwm_core::Event` queue; everything downstream
of that - layout, placement, focus, drag/resize - is platform-independent.
This mirrors the legacy C++ `Platform` interface's shape (see
`docs/PRIOR_ART.md`), which was one of the few architectural decisions in
that codebase that held up.

Both `X11Platform` and `WaylandPlatform` additionally hold their own
`Rc<RefCell<WindowManager>>` clone (shared with `srdwm-config`), so that
when a backend detects a new window (X11 `MapRequest`, Wayland
`new_toplevel`), it can call `wm.alloc_window_id()` + `wm.add_window(...)`
directly rather than needing a separate "please allocate an ID for me"
round-trip through `main.rs`.

## Decoration strategy per platform

Windows can't be decorated the same way on every platform, so each backend
takes the approach that's actually available to it (informed by the prior
art in `docs/PRIOR_ART.md`):

- **X11**: classic reparenting WM. srdwm creates a frame window, reparents
  the client into it below a drawn titlebar band, and owns all decoration
  pixels directly (Xlib/xcb core drawing). Full control, which is why "full
  title bar support" (buttons, drag, resize, matching Windows/macOS) is most
  complete here.
- **Wayland**: srdwm *is* the compositor, so it negotiates
  `zxdg_decoration_manager_v1` server-side mode and renders a decoration
  band itself via `smithay`'s `SolidColorRenderElement`, composited above
  each client surface. Same `ResizeEdge::hit_test` as X11; see
  `docs/IMPLEMENTATION_STATUS.md` for what's not finished (text, precise
  global-keybinding routing).
- **Windows**: DWM will not give you a custom-width or custom-drawn frame
  without disabling the native one entirely, so the design (not yet built --
  see status doc) keeps DWM's frame and controls it (`DWMWA_BORDER_COLOR`),
  matching how komorebi/glazewm operate rather than fighting DWM.
- **macOS**: there is no public API to draw on another process's window at
  all. The design (also not yet built) is a separate, click-through overlay
  window that tracks the target window's position via the Accessibility
  API, matching AeroSpace's AX-only approach rather than yabai's
  SIP-disabling private APIs.

## Why `srdwm_core::window::ResizeEdge::hit_test` is shared, not duplicated

Titlebar hit-testing (which pixel band is "drag", which is the close
button, which edge is a resize grab) is pure geometry - it doesn't need to
know anything about X11 or Wayland. Putting it in `srdwm-core` means the
X11 and Wayland backends *cannot* drift into behaving differently for the
same click, which was worth the small indirection cost (both backends pass
`(frame_rect, x, y)` in and get back a `TitlebarHit` enum to act on).

## Config loading

`Engine::new(wm, config_dir)` seeds every key documented in
`docs/DEFAULTS.md` before any user script runs (so `srd.get(...)` never
returns `nil` for a documented key), then `Engine::load_init()` executes
`config_dir/init.lua`, which in the shipped example
(`config/srd/init.lua`) calls `srd.load("keybindings")` etc. to pull in the
rest - `srd.load(name)` reads and executes `config_dir/{name}.lua` in the
same Lua state, so later files can see earlier `srd.bind()`/`srd.set()`
calls. Config directory resolution order: `$SRDWM_CONFIG_PATH`, then
`$XDG_CONFIG_HOME/srdwm/srd`, then `~/.config/srdwm/srd` (matching
`docs/DEFAULTS.md`'s documented location).
