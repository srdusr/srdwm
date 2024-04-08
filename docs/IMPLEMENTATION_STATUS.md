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

**Module layout.** `lib.rs` had grown to ~1260 lines holding state, every
protocol handler, input routing and rendering; it is now a 78-line shell
(module declarations plus `connect()`), with the rest split by
responsibility:

| module | responsibility |
| --- | --- |
| `state.rs` | `CompState` (the `Display<D>` state everything hangs off), outputs, window bookkeeping |
| `protocols.rs` | smithay `*Handler` impls + `delegate_*!` macros - deliberately thin |
| `input.rs` | keyboard/pointer routing and what "focus" means |
| `lock.rs` | session lock as one feature: state, handler *and* its render helpers |
| `screencopy.rs` | hand-written `wlr-screencopy` (no smithay helper exists) |
| `winit.rs` / `udev.rs` | the two backends - all they differ in is how a frame reaches a screen |
| `decoration.rs` / `xwayland.rs` | titlebar rasterisation; XWayland bridge |

`lock.rs` is grouped by *feature* rather than by kind on purpose: the
security-relevant invariant spans state, protocol handling and rendering at
once, so splitting it across three files would have hidden it.

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
- ✅ XWayland integration (`crates/wayland/src/xwayland.rs`), udev/DRM
  backend only (the winit backend would need its own `calloop::EventLoop`
  added first - see the module's doc comment): spawns XWayland, starts
  `X11Wm`, and implements `XwmHandler`/`XWaylandShellHandler` to bridge
  X11-only clients into the same `WindowManager`/`Space` pipeline
  xdg-shell windows use (`CreateNotify`/`MapRequest` create a real
  `srdwm_core::Window`, matched by rules via `class()`; unmap/destroy
  clean up the same way). **Verified live end-to-end**: an `xterm`
  launched against the spawned XWayland renders as a correctly-sized,
  server-managed window, and typing at it (via a real synthetic
  QEMU-level keyboard, not a shortcut) reaches the shell inside it --
  `ls` produced a new prompt line. Getting there surfaced three real bugs,
  each root-caused with evidence rather than guessed at:
  - XWayland tries `glamor` (GBM-based rendering) first; since this
    compositor is deliberately software-only, glamor fails and previously
    left XWayland on a rendering path that never used the
    `xwayland_shell_v1` protocol at all (confirmed via `WAYLAND_DEBUG=1`
    tracing: the global was bound but `get_xwayland_surface`/`set_serial`
    were never called). Fixed by shadowing `Xwayland` on `PATH` with a
    tiny wrapper script that always re-execs the real binary with `-shm`
    - `smithay::xwayland::XWayland::spawn` builds its own fixed argument
    list with no way to pass this directly, and its `XWaylandClientData`
    has private fields so the spawn call itself can't be bypassed either.
  - Even with `-shm`, `set_mapped(true)` was only ever called from inside
    `finish_x11_window_setup`, itself gated on `X11Surface::wl_surface()`
    already resolving - but XWayland doesn't appear to advance a window
    past surface creation (no buffer attach, no further protocol traffic
    at all) until the map is granted. A real deadlock, found by tracing
    the *same* `WAYLAND_DEBUG=1` output before and after the `-shm` fix
    and seeing identical behavior either way. Fixed by calling
    `set_mapped(true)` unconditionally in `map_window_request`, before
    checking whether `wl_surface()` is available.
  - The window then rendered as a ~1px sliver: `map_window_request` seeded
    the initial `srdwm_core::Window` geometry from
    `X11Surface::geometry()`, which at `MapRequest` time can still be
    whatever tiny default the X11 window was *created* with (our own
    `configure_request` handler is deliberately a no-op - this compositor
    owns layout for managed windows). Fixed by using the same fixed
    800x600 default `new_managed_window`'s xdg-shell path already uses,
    instead of trusting the client's initial size.
  - Typing didn't reach the window at all until a fourth, broader bug was
    found and fixed *outside* the XWayland code: nothing in the whole
    Wayland backend ever called `KeyboardHandle::set_focus` - clicking a
    window only updated `srdwm_core::WindowManager`'s own focus tracking,
    never Wayland/X11 keyboard focus. This affected xdg-shell windows too,
    not just XWayland ones. Fixed in `lib.rs`'s `handle_pointer_button`
    (both the decoration-click and click-through-to-content-area paths,
    the latter of which also never focused a window at all, only raised
    it). `TitlebarHit::Close` was also X11-surface-blind (only called
    `ToplevelSurface::send_close()`), fixed alongside.
  - No font is installed in the test VM at all (a gap in the VM's package
    set, not the code), so the titlebar band renders with no title text in
    this environment - `decoration.rs`'s font-search fallback is working
    exactly as designed; see its own section above for where actual text
    rendering was verified.
  - Not implemented: selections/clipboard, XSETTINGS, RandR
    primary-output sync, override-redirect window geometry beyond initial
    placement (all have harmless no-op default `XwmHandler` methods).
- ✅ **`wlr-layer-shell-unstable-v1`** (`WlrLayerShellHandler`, `delegate_layer_shell!`
  in `lib.rs`): layer surfaces are mapped into the output's
  `smithay::desktop::LayerMap` (`layer_map_for_output`), which `render_output`
  renders automatically - no rendering-path changes were needed, only state
  wiring, initial-configure-on-commit, and pointer/keyboard routing
  (`layer_surface_under` in `lib.rs`, checked ahead of our own decorations and
  xdg-shell windows so bars/launchers/notifications/lock UIs sit properly on
  top; `Exclusive`-interactivity surfaces grab keyboard focus on commit,
  `OnDemand` ones on click). Background/bottom-layer pointer routing (e.g. a
  wallpaper daemon wanting clicks) is out of scope - nothing needed for the
  daily-driver gate requires it.
  **Verified live** against two real, unmodified clients (waybar 0.x, wofi
  1.5.3) run as actual Wayland clients of a running `srdwm` (winit backend,
  `WAYLAND_DEBUG=1` protocol tracing): waybar's Top-layer bar configured
  correctly ("Bar configured (width: 934, height: 45) for output:
  srdwm-wayland"); wofi's Exclusive-interactivity launcher surface was
  created, sized, and configured with no crash. Getting there surfaced and
  fixed three real bugs:
  - `Output::change_current_state` was called unconditionally every render
    frame (60/s), which - harmless with zero Wayland-native clients ever
    connected before this - floods any client actually bound to `wl_output`
    with duplicate `mode`/`done` events forever. Fixed by only calling it
    (and re-`arrange()`ing the layer map) when the output size actually
    changed.
  - The very first `configure` sent to a newly-mapped layer surface used
    stale geometry: `map_layer`'s own `arrange()` runs before the client's
    `set_size`/`set_anchor`/etc. requests (and the commit applying them) have
    even arrived, so the initial `send_configure()` was re-sending that
    stale pre-request computation instead of recomputing from what the
    client actually asked for (caught live: wofi's `set_size(420, 550)` was
    silently ignored, and it configured stuck at the output/2 fallback
    instead). Fixed by re-`arrange()`ing on every layer-surface commit
    (`LayerMap::arrange` only ever sends a configure when something actually
    changed, so this is a no-op on unrelated commits).
  - The missing `zxdg_output_manager_v1` (xdg-output) global - a separate,
    real gap of its own, see below - made wofi's own layer-shell setup code
    call a Wayland request on a proxy that was never bound (it doesn't
    null-check), **segfaulting the client**, not just failing gracefully.
    Root-caused with `gdb` (crash was `wl_proxy_marshal_constructor(proxy=0x0,
    opcode=1, ...)`, matching `zxdg_output_manager_v1.get_xdg_output`) and
    confirmed by comparing a `WAYLAND_DEBUG=1` trace of the same `wofi`
    binary against the user's real Hyprland session (which advertises
    xdg-output and doesn't crash it) side by side with the trace against
    `srdwm`.
- ✅ **xdg-output (`zxdg_output_manager_v1`)**: added via smithay's
  `OutputManagerState::new_with_xdg_output`, piggybacking on the existing
  `delegate_output!`/`OutputHandler` wiring (no new handler trait needed).
  Not itself in the original "biggest blocker" list, but found to be a hard
  requirement in practice while fixing layer-shell above - see the wofi
  segfault account.
- ✅ **Clipboard**: `wl_data_device_manager`, `zwp_primary_selection_v1`,
  and `zwlr_data_control_manager_v1`, all three sharing smithay's single
  `SelectionHandler`. Data-control is the one that matters most for this
  user's session: `wl-paste --watch cliphist store` (in their Hyprland
  autostart) needs to read the selection *without* holding keyboard focus,
  which the core data-device protocol cannot do.
  The non-obvious wiring is that selection focus must follow keyboard focus
  - `set_keyboard_focus` now also calls `set_data_device_focus` and
  `set_primary_focus`, because the data-device protocols only offer the
  selection to, and accept `set_selection` from, the focus-holding client.
  **Verified live** against the user's own tools: `wl-copy`/`wl-paste`
  round-tripped both clipboard and primary; `wl-paste --watch cliphist
  store` captured three successive copies; and a real `wezterm` toplevel
  was observed receiving `wl_data_device.data_offer` + `selection` over
  `WAYLAND_DEBUG=1`, i.e. the core (non-data-control) path works too.
  Drag-and-drop uses smithay's default `ClientDndGrabHandler`/
  `ServerDndGrabHandler` behaviour and has *not* been separately tested.
  This surfaced a real pre-existing bug, fixed here: nothing ever gave a
  **newly-created** window Wayland focus. `WindowManager::add_window` sets
  its own `focused` field, but no code path turned that into a
  `KeyboardHandle::set_focus`, so a freshly-opened app received no
  keystrokes and could not paste until it was clicked. (Same class as the
  click-to-focus bug fixed in the XWayland pass; this was the creation
  path.)
- ✅ **`ext-session-lock-v1`** (screen locking): `SessionLockHandler` with
  per-output lock surfaces. `locked` gates both rendering (only the lock
  surface, over an opaque black clear - no windows, decorations, or layer
  surfaces) and input (all keys go to the lock surface, and **no key is
  treated as a WM binding**, since the shipped config binds
  `Mod4+Return` to spawn a terminal and honouring that at a locked screen
  would defeat the lock entirely). The lock is confirmed only *after* a
  client-content-free frame has actually been presented, never at request
  time, so the locker is never told "the screen is safe" while the user's
  windows are still on screen.
  **Verified live** with a purpose-written minimal `ext-session-lock`
  client (no locker - hyprlock/swaylock/etc. - is installed on this
  machine, so there was nothing else to test against; the user's
  `~/.scripts/lock` currently falls through to `loginctl lock-session`):
  lock → cleared frame → `locked` confirmation → lock surface configured to
  the real output size → `unlock_and_destroy` → normal operation restored.
  Three properties were checked by counting protocol events delivered to a
  real `wezterm` launched at each point:
  - unlocked: 1 `wl_keyboard.enter` (control);
  - locked: 0 `wl_keyboard.enter`, 0 `wl_pointer.enter`;
  - locker killed *without* unlocking: still 0 - the session correctly
    stays locked when the screen locker crashes, as the protocol requires.
  The locked-case count was **1, not 0, before a bug was found and fixed by
  this exact test**: `new_managed_window` called `set_keyboard_focus`
  unconditionally, so merely opening a window at a locked screen handed it
  keyboard focus. The guard now lives in `set_keyboard_focus` itself, as
  the single chokepoint every focus path goes through.
- ✅ **`wlr-screencopy-unstable-v1`** (`crates/wayland/src/screencopy.rs`):
  what `grim` uses, and therefore what the user's `Print` / `Alt+Print`
  binds (`grim`, `slurp | grim -g -`) and `wf-recorder` need. smithay 0.7
  ships **no** helper for this protocol, so the `GlobalDispatch`/`Dispatch`
  plumbing is written out by hand against the raw `wayland-protocols-wlr`
  server bindings (a new direct dependency, pinned to the version smithay
  already uses so both see one set of types). Capture is deferred: a `copy`
  request only queues the frame, and pixels are read back during the render
  pass via `ExportMem::copy_framebuffer`.
  **Verified live with real `grim`**: full-output capture, region capture
  (`-g "0,0 420x110"`, confirmed by screenshotting a window and reading the
  PNG back - correct offset, size, colours, and orientation, which is also
  what establishes that no `y_invert` flag is needed), and an
  out-of-bounds region (clamped, no crash). While the session is locked,
  queued captures are rejected outright rather than served or left
  queued - confirmed: `grim` fails fast with "failed to copy output" and
  writes nothing.
  One real bug was found and fixed by this testing: reading back the winit
  backend's **EGL window surface** destroyed the GL context on the first
  capture (`eglSwapBuffers: BAD_SURFACE` → `BAD_ALLOC` → "context has been
  lost", taking the whole compositor down), root-caused by A/B-ing the
  identical build with only the readback call removed. The winit path now
  renders a second pass into an offscreen `GlesRenderbuffer` and reads
  *that*, costing an extra scene render only on frames where a capture was
  actually requested. The udev/pixman path is unaffected - its render
  target is already a plain memory image, so reading it directly is safe.
  Not implemented: `linux_dmabuf` capture (the manager is capped at
  protocol version 2 for that reason) and cursor overlay
  (`overlay_cursor` is accepted and ignored - this backend draws no
  cursor of its own yet).
- ✅ **Multi-monitor** (udev/DRM backend). Every connected connector becomes
  a `UdevHead` with its **own** scanout buffers, damage tracker and
  page-flip state, laid out left-to-right in a shared global coordinate
  space; the `PixmanRenderer` is shared, since they are one GPU. A head
  whose flip is still in flight is skipped for that pass and resumes when
  its own page-flip event arrives (matched by CRTC), so monitors at
  different refresh rates each run at their own pace instead of the slowest
  gating the rest.
  Connector→CRTC assignment never reuses a CRTC, so a machine with more
  monitors than CRTCs drives as many as the hardware allows and logs the
  rest. Modes are chosen by the `PREFERRED` flag rather than list order.
  The rest of the compositor reaches outputs through
  `CompState::{primary_output, output_at, output_for_wl}` rather than a
  single field, which is what kept the change small: layer surfaces map to
  the output the client names, session lock creates **one lock surface per
  output** (and only confirms the lock once *every* output has both a
  surface and a presented frame - otherwise a second monitor could still
  be showing the desktop when the locker is told the session is safe),
  screencopy captures the output the client names, and the pointer is
  clamped to the union of all heads so it can cross between them.
  **Verified live in the QEMU VM** with a two-output `virtio-gpu`
  (`max_outputs=2`, second connector forced on with `video=Virtual-2:...e`,
  default VGA removed so the GPU choice is unambiguous):
  - srdwm logged `2 connected output(s)` and built both heads
    (`Virtual-1 1280x800 at x=0`, `Virtual-2 ... at x=1280`), and core saw
    `2 monitor(s)`;
  - QMP `screendump` of **both** heads returned each one's own resolution,
    both filled with srdwm's exact clear colour `rgb(12,12,20)`
    (= `[0.05, 0.05, 0.08]`) - i.e. both are really being rendered and
    scanned out, not just enumerated;
  - a window forced by `srd.rule` to **global** x=1500 appeared on head 1 at
    head-local x=**220** (= 1500 − 1280, the exact translation) with its
    srdwm titlebar, while head 0 stayed completely empty (0 of 64000 sampled
    pixels differed from the clear colour).
  The nested winit backend remains single-output by construction (it is one
  window on a host compositor).

**Known limitation of the nested (winit) backend**: it renders through the
host compositor's frame callbacks, so if the srdwm window is occluded or on
another workspace, the host stops scheduling it, `eglSwapBuffers` blocks,
and srdwm's whole main loop stalls - it stays alive but stops serving
clients until the window is visible again. Observed repeatedly while
testing. This affects only the nested development path; the udev/DRM
backend drives its own page flips and is unaffected.

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
  backend's spawned XWayland renders as a correctly-sized, decorated
  (background-only in this VM - no font installed) window, and receives
  real keyboard input end-to-end: a synthetic QEMU-level keypress sequence
  (`ls` + Enter) executed inside the shell and produced a new prompt line,
  screendump-confirmed. Getting there took four rounds of root-causing via
  `WAYLAND_DEBUG=1` protocol tracing and fixing real bugs - see the
  Wayland backend section above for the full account.

## Not implemented anywhere yet

All three protocols originally identified as blocking srdwm-wayland from
being a real daily-driver session (bars/launchers/notifications/lock UIs,
clipboard, screen locking) are now implemented and verified - see the
Wayland backend section above. What is left:

- **Connector hotplug** - connectors are probed once at startup, so
  plugging a monitor in (or unplugging one) while srdwm is running is not
  noticed. Needs a udev event source, which this backend does not register
  yet. Multi-monitor itself *is* implemented (see above); only hotplug is
  missing.
- **Multi-GPU** - only the primary GPU's connectors are driven.
- Animations (`general.animations`/`animation_duration` config keys exist
  and are read into defaults, but nothing consumes them yet).
- A native GUI settings app (the legacy project's `GUI_SETTINGS.md` was
  pure design doc even in C++; not revisited here).
