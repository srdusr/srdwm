# Implementation status

This mirrors the style of the legacy C++ project's own status doc (now at
`legacy-cpp/docs/IMPLEMENTATION_STATUS.md`), but for the Rust rewrite.
"Verified" means: built with `cargo test --workspace` (0 warnings under
`cargo clippy --workspace`) and, where applicable, actually run and observed
doing the thing described - not just "the code compiles and looks right."

## ✅ Complete and verified

### Core window/workspace/layout engine (`crates/core`)
- `WindowManager`: window/workspace/monitor state, focus cycling, directional
  focus (`Direction::{Left,Right,Up,Down}`), drag/resize state machine,
  hit-testing shared by every backend.
- `MasterStackLayout`: real dwm-style master/stack tiling with configurable
  ratio and gaps (the legacy C++ tiling layout only ever split windows into
  equal-width columns, ignoring its own documented `master_ratio` config key).
- `SmartPlacement`: grid placement with real per-cell occupancy tracking,
  diagonal cascade fallback, and Windows-Snap-style edge magnetism
  (half/quarter/maximize zones). The legacy C++ version's grid placement
  used a `static` round-robin counter that hardcoded a 2-column layout
  regardless of window count; its cascade never actually cascaded; its
  snap-to-edge always returned a fixed centered rectangle.
- 35 unit tests, deterministic (window arrangement no longer depends on
  `HashMap` iteration order - an early version of `arrange_workspace` did,
  and it was caught by a flaky test during this rewrite; see the fix in
  `crates/core/src/manager.rs`).

### Lua config engine (`crates/config`)
- Full `srd` API matching `docs/DEFAULTS.md`'s documented (not the legacy
  C++'s actually-implemented) surface: `srd.set/get/reset/reset_all/reset_category`,
  `srd.window.{focused,close,minimize,maximize,focus,set_decorations,
  set_border_color,set_border_width,set_floating,toggle_floating,is_floating}`,
  `srd.layout.{set,configure}`, `srd.workspace.{next,prev,switch,move_window}`,
  `srd.theme.{set_colors,set_decorations}`, `srd.bind`, `srd.load`,
  `srd.spawn`, `srd.notify`, `srd.quit`, `srd.rule`, `srd.validate_config`,
  `srd.debug.{config_status,validate_config,show_settings,profile_start,profile_stop}`.
- `srd.bind()` stores the actual Lua closure via `mlua`'s registry and
  invokes it on dispatch (the legacy engine stored only the key-combo
  string - keybindings could never fire).
- `srd.rule(matcher, actions)` matches windows by title (substring) or
  class/app_id (exact) and applies floating/maximized/workspace/geometry/
  decoration/border actions once, when a matching window is first created
  (`crates/core/src/rules.rs`, applied from `WindowManager::add_window`).
  `config/srd/rules.lua` documents the API instead of being a no-op
  placeholder.
- `srd.validate_config()` (and `srd.debug.validate_config()`) actually
  check the numeric ranges, layout-name references, and hex-color formats
  documented in `docs/DEFAULTS.md`'s "Validation Rules" section, returning
  `(ok, errors)` - not a trivial always-true. `srd.debug.config_status()`/
  `show_settings()`/`profile_start()`/`profile_stop()` are real too.
- `local srd = require("srd")` works (registered via `package.preload`, not
  just as a global) - every shipped example config opens with this line,
  and it would have failed against a naive "global-only" registration; this
  was caught and fixed during the smoke test.
- 15 unit tests, including one that reproduces the exact legacy bug
  (`window:close()` on a table with no methods) and shows it now works.

### X11 backend (`crates/x11`)
- Real reparenting WM: frame windows sized to actual client geometry (not
  the legacy's hardcoded 800px titlebar), drawn title bar with
  close/maximize/minimize buttons, drag-to-move, edge/corner resize --
  all driven by the same `ResizeEdge::hit_test` the Wayland backend uses.
- Correct other-WM detection via a *checked* `SUBSTRUCTURE_REDIRECT`
  request (the legacy version's error handler discarded errors and always
  reported success).
- RandR monitor enumeration using CRTC pixel mode, not output physical
  millimeters (the legacy version conflated the two).
- `WM_DELETE_WINDOW`-aware close, click-to-focus via a passive button grab
  + replay (the standard dwm/openbox pattern), global keybinding grabs
  translated from Lua combo strings via a hand-maintained keysym table
  (`crates/x11/src/keysyms.rs`, letters/digits/navigation/F-keys/media keys;
  not a full xkbcommon keymap).
- **Verified live**: run under Xephyr with the shipped example config, an
  `xterm` client was correctly reparented (frame at the exact
  `SmartPlacement`-computed position, client offset by exactly
  `TITLEBAR_HEIGHT`), and the drawn title bar (background, title text,
  minimize/maximize/close glyphs) was confirmed via screenshot.
- **Re-verified in an isolated QEMU VM** (see "QEMU VM verification" below):
  two `xterm` clients reparented and placed by `SmartPlacement`, each with
  a drawn titlebar showing real title text and close/maximize/minimize
  glyphs, screenshotted via QEMU's `screendump`.

### Windows and macOS backends (`crates/windows`, `crates/macos`)
- Structured as honest stubs: real-looking `windows-rs`/Core Graphics calls
  behind `cfg(windows)` / `cfg(target_os = "macos")`, but **never built or
  run** - this sandbox only has the `x86_64-unknown-linux-gnu` target
  installed. On any other target the same methods return
  `PlatformError::Unsupported` rather than pretending to work.
- Design intent (informed by komorebi/glazewm for Windows, yabai/AeroSpace
  for macOS - see `docs/PRIOR_ART.md`) is documented in each crate's module
  doc comment: keep DWM's native frame on Windows rather than fight it;
  use the public Accessibility API plus an overlay window for decorations
  on macOS, not private APIs.

## 🔄 Wayland backend (`crates/wayland`) - real, more limited scope than X11

This is the one piece with essentially no working prior art to port (see
`docs/PRIOR_ART.md`): the legacy C++ never wired a single event listener.
What's here is a genuine from-scratch `smithay`-based compositor, not a stub:

- ✅ Runs via smithay's winit backend (nested window), initializes EGL/GLES,
  advertises a real Wayland socket, and was verified to start, initialize
  rendering, and run its event loop without crashing (log-verified; a
  full visual confirmation the way X11 got one was skipped deliberately --
  see below).
- ✅ xdg-shell toplevels are tracked through the *same*
  `srdwm_core::WindowManager` the X11 backend uses - new windows get a
  real `WindowId`, go through `SmartPlacement`/`MasterStackLayout` exactly
  like X11 windows do.
- ✅ xdg-decoration is negotiated to server-side mode.
- ✅ Pointer click/drag/resize on the decoration band uses the identical
  `hit_test` code path as X11.
- ✅ Decorations render actual title text (`crates/wayland/src/decoration.rs`):
  glyphs rasterized via `fontdue` against whatever monospace font is found
  under `/usr/share/fonts` etc. (falls back to solid-color-only, same as
  before, if none is found), uploaded per-frame through smithay's
  `MemoryRenderBuffer`. Pure `(width, height, text) -> Vec<u8>` function,
  unit-tested without any GL/display context.
- ✅ Global keybindings are matched precisely: `WaylandPlatform::connect`
  takes the config's actual bound-key combo strings (same format/shared
  `srdwm_core::keysyms` table the X11 backend's `XGrabKey` calls use) and
  only a matching keypress is withheld from the focused client - no more
  "any Super-held key is ours" heuristic.
- ✅ DRM/udev backend (`crates/wayland/src/udev.rs`): runs as the real
  compositor on a bare TTY, no host session to nest under. Single primary
  GPU, first connected connector, its first-listed mode, real `libseat`
  session/seat handling (VT-switch pause/resume, no raw root-only
  `/dev/dri` open), real `libinput` input sharing the exact same
  keybinding/hit-test code the winit backend uses. Rendering is
  **software** (smithay's `PixmanRenderer` into plain KMS dumb buffers via
  the legacy, non-atomic `set_crtc`/`page_flip` API) rather than
  GBM/EGL/`DrmCompositor`-based hardware acceleration: that path needs a
  GPU with working KMS+3D driver support that a low-spec machine's VM isn't
  guaranteed to have, while dumb buffers work on essentially any DRM
  driver. `WaylandPlatform::connect` (winit) picks this backend
  automatically when no `WAYLAND_DISPLAY`/`DISPLAY` is set, falling back to
  nested winit if udev init fails for any reason.
  **Verified live in an isolated QEMU VM** (see below): started on a bare
  virtual TTY with no `DISPLAY`/`WAYLAND_DISPLAY`, opened `/dev/dri/card1`
  via a real libseat session, initialized libinput, advertised a Wayland
  socket, and rendered a frame that scanned out correctly via KMS
  page-flip - confirmed by screendumping the guest's virtual framebuffer
  and matching the exact clear color (`[0.05, 0.05, 0.08]`) the compositor
  renders. No client-side visual check yet (the VM has no Wayland-native
  client installed to test against, only X11 ones - see below). No
  hotplug (connectors or GPUs) after startup.
- 🔄 XWayland integration (`crates/wayland/src/xwayland.rs`), udev/DRM
  backend only (the winit backend would need its own `calloop::EventLoop`
  added first - see the module's doc comment): spawns XWayland, starts
  `X11Wm`, and implements `XwmHandler`/`XWaylandShellHandler` to bridge
  X11-only clients into the same `WindowManager`/`Space` pipeline
  xdg-shell windows use (`CreateNotify`/`MapRequest` create a real
  `srdwm_core::Window`, matched by rules via `class()`; unmap/destroy
  clean up the same way). **Verified working up through window creation
  and event routing, then found a real architectural blocker**: XWayland
  tries `glamor` (GBM-based rendering) first; since this compositor is
  deliberately software-only (no GBM/DMA-BUF support - the whole point of
  the dumb-buffer approach above), glamor fails, and XWayland's
  post-failure fallback path skips the `xwayland_shell_v1` protocol
  entirely, so `X11Surface::wl_surface()` never resolves and windows never
  render. Confirmed by tracing the actual Wayland protocol exchange
  (`WAYLAND_DEBUG=1` on the spawned XWayland process): it binds
  `xwayland_shell_v1` at startup, then a second, window-creation-time
  registry pass sees the global but never binds it, and
  `get_xwayland_surface`/`set_serial` never appear at all. `Xwayland
  -help` confirms a `-shm` flag exists that forces shared-memory buffers
  from the start (matching this compositor's `wl_shm`/`ImportMem`-only
  renderer) instead of trying and falling back from glamor - but
  `smithay::xwayland::XWayland::spawn` builds its `Xwayland` command line
  internally with a fixed argument list and has no way to add `-shm`.
  Fixing this for real means either bypassing `XWayland::spawn` with a
  custom implementation (reimplementing its X11 lock-file/socket-pair/
  readiness-detection logic, which is intentionally private to smithay --
  `mod x11_sockets;`, not `pub mod`) or giving the compositor real
  GBM/DMA-BUF import support, undoing the earlier deliberate low-spec/
  no-GPU-required design. Left as a documented gap rather than rushing a
  low-level reimplementation with no cheap way to iterate on it.

**Why the visual verification stopped short of a screenshot**: the winit
window opens on the *host* compositor, and the only available display in
this sandbox was the user's live desktop session (not an isolated nested
server the way Xephyr was for X11). A screenshot of that would have
captured the user's actual desktop/other work, which isn't appropriate to
casually paste into a build log. The X11 backend's Xephyr-based
verification is the same class of test, done on an isolated, disposable
display instead.

## QEMU VM verification

Both the X11 backend and the Wayland/DRM-udev backend were re-verified from
scratch in an isolated QEMU VM (not the sandbox they were originally built
in), to check they work somewhere other than the exact environment that
built them:

- **VM**: minimal Arch Linux rootfs (base, linux, xorg-server, xterm, mesa,
  seatd, libinput, libxkbcommon, xf86-input-libinput - built by copying the
  host's own already-installed files for these packages plus their full
  dependency closure, rather than a fresh `pacstrap`, since this sandbox's
  network throughput made a real package download impractical). Booted via
  direct kernel+initramfs (no bootloader), `virtio-gpu`/`virtio-keyboard`/
  `virtio-mouse`/`virtio-net`, autologin on both the serial console and
  `tty1`, `-display none` with QMP `screendump` for visual verification
  (no interactive GUI needed on the host side).
- **X11 backend**: `run-x11.sh` starts Xorg on `vt1` then execs `srdwm` as
  an X11 client (the standard way it becomes the WM). Two `xterm`s spawned
  via the config's `startup.lua` were reparented, tiled/placed by
  `SmartPlacement`, and both show a drawn titlebar with real "xterm" title
  text and close/maximize/minimize glyphs - screenshotted and visually
  confirmed.
- **Wayland/DRM-udev backend**: run directly on the bare console (no `-x`
  script needed - no `DISPLAY`/`WAYLAND_DISPLAY` set at all). Opened
  `/dev/dri/card1` via `libseat`, initialized `libinput`, advertised a real
  Wayland socket, and rendered/page-flipped a frame - confirmed by
  screendumping the guest's virtual display and matching the exact clear
  color the compositor renders. This required a real bug fix, found by
  this exact test: `srdwm_platform::detect()` previously defaulted to X11
  whenever neither `WAYLAND_DISPLAY` nor `XDG_SESSION_TYPE=wayland` was
  set, *regardless of whether `DISPLAY` was set either* - meaning on a
  genuinely bare TTY it picked X11, a backend that can never work there
  (`srdwm_x11::X11Platform` only ever connects to an already-running X
  server; it doesn't start one). `detect()` now only picks X11 when
  `DISPLAY` is set without Wayland evidence; every other case, including a
  bare TTY, resolves to Wayland, which is the only backend able to run
  standalone there.
- **Not (yet) verified**: nested Wayland (the `backend_winit` path) running
  as an X11 client under this VM's Xorg - `smithay`'s `winit` backend
  failed with `Failed to initialize an event loop`, which is an error
  surfaced from inside the `winit` crate's own X11 initialization, not
  `srdwm`'s code; most likely this minimal VM's software-only Xorg is
  missing a GLX/DRI3 piece `winit`'s EGL context creation wants. The nested
  path was already verified once before (Xephyr-equivalent, log-verified
  per the section above); this is a gap in re-verifying it in this
  specific minimal VM, not a known-broken code path.
- No Wayland-native client was available in this minimal VM to visually
  confirm client-side rendering under either Wayland backend (only
  `xterm`, which is X11-only) - the compositor/socket/render-pipeline
  side is confirmed, but no real Wayland app has been shown on-screen yet.
- **XWayland**: `xterm` launched with `DISPLAY` pointed at the udev
  backend's spawned XWayland connected successfully and stayed alive
  (`CreateNotify`/`MapRequest` both reached `XwmHandler`, logged and
  handled with no crash), but never rendered - this is the
  glamor/`-shm` blocker documented above, root-caused via
  `WAYLAND_DEBUG=1` protocol tracing on the XWayland process rather than
  guessed at.

## Not implemented anywhere yet

- XWayland actually rendering a window (blocked on the glamor/`-shm`
  issue above; the spawn/protocol/window-tracking plumbing is real).
- Animations (`general.animations`/`animation_duration` config keys exist
  and are read into defaults, but nothing consumes them yet).
- A native GUI settings app (the legacy project's `GUI_SETTINGS.md` was
  pure design doc even in C++; not revisited here).
