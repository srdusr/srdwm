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
  `srd.spawn`, `srd.notify`, `srd.quit`.
- `srd.bind()` stores the actual Lua closure via `mlua`'s registry and
  invokes it on dispatch (the legacy engine stored only the key-combo
  string - keybindings could never fire).
- `local srd = require("srd")` works (registered via `package.preload`, not
  just as a global) - every shipped example config opens with this line,
  and it would have failed against a naive "global-only" registration; this
  was caught and fixed during the smoke test.
- 10 unit tests, including one that reproduces the exact legacy bug
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
- ⚠️ Decorations are a solid-color titlebar band with **no text** - font
  rasterization (glyph atlas, text shaping) is a substantial independent
  piece of work, not something to fake with a placeholder.
- ⚠️ Global keybindings use a coarse heuristic: any keypress with Super/Mod4
  held is treated as WM-exclusive and not forwarded to the client; anything
  else is forwarded. A precise design would thread the config's actual
  bound-key set into the platform layer (X11 does this correctly via
  per-combo `XGrabKey`); Wayland's compositor-sees-everything-first model
  makes the equivalent design more involved and was left as a TODO rather
  than rushed.
- ❌ No DRM/udev backend (i.e. cannot run as the actual system compositor on
  a bare TTY, only nested under an existing session) - winit backend only.
- ❌ No XWayland integration.

**Why the visual verification stopped short of a screenshot**: the winit
window opens on the *host* compositor, and the only available display in
this sandbox was the user's live desktop session (not an isolated nested
server the way Xephyr was for X11). A screenshot of that would have
captured the user's actual desktop/other work, which isn't appropriate to
casually paste into a build log. The X11 backend's Xephyr-based
verification is the same class of test, done on an isolated, disposable
display instead.

## Not implemented anywhere yet

- Window rules (match-by-title/class -> action). `config/srd/rules.lua` is
  a documented placeholder.
- `srd.debug.*` namespace, `srd.validate_config()` beyond a trivial always-true.
- Animations (`general.animations`/`animation_duration` config keys exist
  and are read into defaults, but nothing consumes them yet).
- A native GUI settings app (the legacy project's `GUI_SETTINGS.md` was
  pure design doc even in C++; not revisited here).
