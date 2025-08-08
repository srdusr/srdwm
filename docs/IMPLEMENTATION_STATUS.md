# Implementation status

This mirrors the style of the legacy C++ project's own status doc (now at
`legacy-cpp/docs/IMPLEMENTATION_STATUS.md`), but for the Rust rewrite.
"Verified" means: built with `cargo test --workspace` (0 warnings under
`cargo clippy --workspace`) and, where applicable, actually run and observed
doing the thing described - not just "the code compiles and looks right."

## Complete and verified

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

## Wayland backend (`crates/wayland`) - real, more limited scope than X11

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

- Runs via smithay's winit backend (nested window), initializes EGL/GLES,
  advertises a real Wayland socket, and was verified to start, initialize
  rendering, and run its event loop without crashing (log-verified; a
  full visual confirmation the way X11 got one was skipped deliberately --
  see below).
- xdg-shell toplevels are tracked through the *same*
  `srdwm_core::WindowManager` the X11 backend uses - new windows get a
  real `WindowId`, go through `SmartPlacement`/`MasterStackLayout` exactly
  like X11 windows do.
- xdg-decoration is negotiated to server-side mode.
- Pointer click/drag/resize on the decoration band uses the identical
  `hit_test` code path as X11.
- Decorations render actual title text (`crates/wayland/src/decoration.rs`):
  glyphs rasterized via `fontdue` against whatever monospace font is found
  under `/usr/share/fonts` etc. (falls back to solid-color-only, same as
  before, if none is found), uploaded per-frame through smithay's
  `MemoryRenderBuffer`. Pure `(width, height, text) -> Vec<u8>` function,
  unit-tested without any GL/display context.
- Global keybindings are matched precisely: `WaylandPlatform::connect`
  takes the config's actual bound-key combo strings (same format/shared
  `srdwm_core::keysyms` table the X11 backend's `XGrabKey` calls use) and
  only a matching keypress is withheld from the focused client - no more
  "any Super-held key is ours" heuristic.
- DRM/udev backend (`crates/wayland/src/udev.rs`): runs as the real
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
  driver. A real GBM+EGL+`DrmCompositor` GPU path now exists
  (`crates/wayland/src/udev/gpu.rs`), opt-in via `general.gpu` in config
  or the lower-level `SRDWM_GPU=1` env var (both `false`/unset by
  default - this backend stays 100% software unless explicitly asked
  otherwise), falling back to the software path unchanged on any
  failure at any step. Every connected head it successfully initializes
  gets driven through it, VT-switch pause/activate is wired, and the
  real cursor and real window content (plain, square-cornered, no
  border/titlebar yet) both render on top of its own clear color --
  decorations are the remaining gap. Untested on real GPU-enabled
  hardware as of this writing - builds and passes the full test suite,
  but `SRDWM_GPU`/`general.gpu` were both unset on the machine this was
  built on.
  `WaylandPlatform::connect` (winit) picks this backend
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
- XWayland integration (`crates/wayland/src/xwayland.rs`), udev/DRM
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
- **`wlr-layer-shell-unstable-v1`** (`WlrLayerShellHandler`, `delegate_layer_shell!`
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
- **xdg-output (`zxdg_output_manager_v1`)**: added via smithay's
  `OutputManagerState::new_with_xdg_output`, piggybacking on the existing
  `delegate_output!`/`OutputHandler` wiring (no new handler trait needed).
  Not itself in the original "biggest blocker" list, but found to be a hard
  requirement in practice while fixing layer-shell above - see the wofi
  segfault account.
- **Clipboard**: `wl_data_device_manager`, `zwp_primary_selection_v1`,
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
- **`ext-session-lock-v1`** (screen locking): `SessionLockHandler` with
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
  lock cleared frame `locked` confirmation lock surface configured to
  the real output size `unlock_and_destroy` normal operation restored.
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
- **`wlr-screencopy-unstable-v1`** (`crates/wayland/src/screencopy.rs`):
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
  capture (`eglSwapBuffers: BAD_SURFACE` `BAD_ALLOC` "context has been
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
- **Multi-monitor** (udev/DRM backend). Every connected connector becomes
  a `UdevHead` with its **own** scanout buffers, damage tracker and
  page-flip state, laid out left-to-right in a shared global coordinate
  space; the `PixmanRenderer` is shared, since they are one GPU. A head
  whose flip is still in flight is skipped for that pass and resumes when
  its own page-flip event arrives (matched by CRTC), so monitors at
  different refresh rates each run at their own pace instead of the slowest
  gating the rest.
  ConnectorCRTC assignment never reuses a CRTC, so a machine with more
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
    head-local x=**220** (= 1500 1280, the exact translation) with its
    srdwm titlebar, while head 0 stayed completely empty (0 of 64000 sampled
    pixels differed from the clear colour).
  The nested winit backend remains single-output by construction (it is one
  window on a host compositor).
- **Connector hotplug**. A `UdevBackend` event source watches for the
  kernel's `change` uevent on the DRM device; `CompState::reprobe_outputs`
  then re-probes connectors (forcing a fresh probe - on a hotplug the
  cached status is exactly what has gone stale) and reconciles the head
  list. Vanished connectors have their head torn down: `wl_output` global
  removed, output unmapped from the `Space`, DRM framebuffers and dumb
  buffers explicitly freed (dropping the Rust structs alone leaks the
  kernel-side objects, which matters when a cable is plugged repeatedly),
  and any lock surface for that output dropped - otherwise
  `confirm_lock_if_presented` would wait forever for a monitor that no
  longer exists. New connectors are brought up through the same
  `bring_up_head` path used at startup. Every head is then repositioned
  left-to-right, since removing one shifts the rest, and the layer maps are
  re-arranged so bars follow their moved output.
  `WindowManager::set_monitors` rehomes windows left stranded, and
  `main.rs` re-queries the whole monitor list on `MonitorAdded`/
  `MonitorRemoved` rather than applying the single monitor in the event
  (positions of the others change too).
  **Verified live in the QEMU VM**, booting with one connector and toggling
  the second at runtime:
  - plug in `hotplug - 0 output(s) removed, 1 added`,
    `output Virtual-2 connected (1024x768)`, `monitor layout changed:
    2 monitor(s)`, and a screendump of the new head showed it really
    rendering at its own resolution;
  - unplug `1 output(s) removed, 0 added`, back to 1 monitor, compositor
    healthy;
  - **window rescue**: an xterm placed by rule at global x=1500 (on the
    second monitor) was still visible after that monitor was unplugged --
    it reappeared on the remaining head at x=680, exactly
    `min(1500, 1280-600)`, keeping its 600x400 size.
  Caveat on method: writing to `/sys/class/drm/<connector>/status` changes
  the connector but emits **no uevent** on this kernel, so the uevent the
  kernel would send on real hardware is synthesized with `udevadm trigger
  --subsystem-match=drm --action=change`. The reaction path - re-probe,
  diff, bring up/tear down, re-layout, rehome - is genuinely exercised;
  only the initial signal is injected.

  This turned up a real bug that the unit tests had **missed**: rehoming
  originally keyed off `Window::monitor`, but that field records the
  monitor a window was *assigned* at creation, not where it actually is --
  `add_window` always sets it from the primary monitor, so a window placed
  on the second monitor by a rule (or dragged there) still reads
  `monitor == 0`. The field-only check saw a valid id, skipped the window,
  and left it at coordinates that no longer existed: invisible and
  unreachable. Caught by unplugging a monitor out from under a real xterm
  and watching it vanish from both heads. `set_monitors` now keys off
  geometry, with a regression test that fails against the old logic.

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

- **Mouse cursor** (`crates/wayland/src/cursor.rs`). Previously **nothing
  drew a pointer at all** - invisible mouse on a bare TTY. It hid because
  the nested backend runs inside another compositor, which draws a cursor
  over srdwm's window; only the DRM backend is affected, and only when run
  as a real session. A built-in arrow (a reviewable ASCII bitmap, no XCursor
  theme dependency, same reasoning as `decoration.rs`'s font fallback) is
  now composited above everything on the output the pointer is on.
  `CursorImageStatus::Hidden` is honoured. **Not** yet done: rendering a
  client's own cursor surface or a named shape, so an app asking for an
  I-beam still gets the arrow.
  **Verified in the QEMU VM**: screendump of a bare-TTY session shows a
  recognisable arrow at the pointer position - 113 white fill + 58 black
  outline pixels at screen centre, where the pointer starts.
- **Lid switch**: libinput switch events are handled and surfaced to
  config as `srd.on("lid_closed"/"lid_open", fn)`, so a session can lock and
  suspend on lid close. Previously there was no switch handling at all.
- **Fullscreen** (`srd.window.fullscreen()`), **directional window move**
  (`srd.window.move("left")`, swaps with the neighbour), **focus cycling**
  (`srd.window.next()`/`prev()`), **modifier+drag move/resize anywhere in a
  window**, **modifier+scroll workspace switching**, and 8 more `XF86`
  media/power keysyms. All were needed to port a real Hyprland config and
  none existed before.

- **Cursor shapes**: a client's own cursor surface is now rendered with
  its declared hotspot, so an I-beam over text or a hand over a link shows
  the app's image rather than srdwm's arrow. The built-in arrow remains the
  fallback for when no client has set one (over decorations and the
  desktop). Named shapes (`CursorIcon::Named`, e.g. what a client requests
  via `wp_cursor_shape_v1` instead of uploading a surface) now render as
  real per-shape bitmaps too - text (I-beam), and the four resize
  directions (`ew`/`ns`/`nesw`/`nwse`) - rather than falling back to the
  arrow; everything else still uses the arrow. The WM itself also drives
  this: hovering a resize edge or actively resizing sets the matching
  shape even for clients that never touch the cursor protocol themselves
  (`crates/wayland/src/input.rs`'s `update_cursor_shape`).
- **Key repeat for bindings** (`srd.bind_repeat`, Hyprland's `binde`).
  Held volume/brightness keys and switcher cycling now repeat at the seat's
  own rate (200ms delay, 25/s). Driven from the poll loop rather than a
  timer source, because the winit backend has no `calloop` loop of its own
  and `poll_events` already runs continuously in both.
- **Always-on-top / pin** (`srd.window.toggle_pin`, and `pinned = true`
  as a window rule - their picture-in-picture and HUD rules use it).
  `Window::always_on_top` was another declared-but-never-read field.
  Enforced in `WindowManager`'s stacking order rather than at render time,
  so every consumer of `stacking_order` gets it and none can forget it.
- **Mouse-only window management** works without touching the keyboard:
  drag the titlebar to move, drag any edge or corner to resize, the
  titlebar buttons to close/maximise/minimise, click to focus, drag to a
  screen edge to snap (half/quarter/maximise), and **double-click the
  titlebar to maximise**. The resize grab band was widened from 6px to
  10px - a hairline border is genuinely hard to hit with a mouse, which is
  why Hyprland ships `extend_border_grab_area`.
- **Config path** is now `~/.config/srd` (or `$XDG_CONFIG_HOME/srd`),
  not `~/.config/srdwm/srd` - the extra level said the same thing twice.
- **`wl_pointer.frame` was never sent.** `input.rs`'s motion/button handlers
  called `PointerHandle::motion`/`button` correctly, but never followed up
  with `PointerHandle::frame` - confirmed by reading smithay's own
  `DefaultGrab`: `motion`/`button` there just forward to the handle, they
  never call `frame` themselves, so it's entirely on the compositor to send
  it. Per protocol (required since `wl_pointer` v5; this compositor
  advertises v9) `frame` is what tells a client "the events since the last
  one are a single atomic update, act on them now" - without it, any
  client that correctly waits for it (which is most real ones, including
  Firefox and wezterm, confirmed live: neither registered a click or a
  drag-selection with the cursor squarely on the target) never actually
  processes motion or button state it was sent. The one place `frame` was
  already being sent - the scroll/axis path - worked the whole time,
  which is why this went unnoticed for so long: motion and clicks looked
  fine from the compositor's own side (cursor tracked correctly, our own
  decoration hit-testing and window management never touch this path at
  all, since a decoration click is intercepted before ever reaching a
  client), and every mouse-only window-management item in the list above
  still worked perfectly, since none of it depends on a client ever
  processing anything. Only clicks/drags that needed to reach a client's
  *content* were silently inert. This is very likely the real root cause
  behind most of a night's worth of "clicking/scrolling doesn't work"
  reports that survived several other real, necessary fixes (subsurface
  routing, decoration geometry, `app_id` never being set for native
  Wayland windows) without going away.
- **Undecorated windows had a phantom titlebar.** `ResizeEdge::hit_test`
  applied its top-`TITLEBAR_HEIGHT`-band/button logic unconditionally,
  regardless of `Window.decorated` - a window's allocated geometry always
  reserves that space (placement doesn't shrink it just because decoration
  later gets turned off), so for an undecorated window, hit-testing still
  swallowed clicks in that band as a phantom drag/close/maximize/minimize
  hit instead of ever reaching the client. Only became visible once
  `decorated = false` rules could actually apply to anything (see the
  `app_id` fix above) - Firefox's own tab strip/URL bar live exactly in
  that band. Fixed by gating the band/button check on `decorated`; plain
  resize-from-edge still applies either way. Exposed a second, previously
  unreachable gap while fixing it: `resize_edge_at`'s match arms had no
  case for a plain top edge (only the two top corners), so a window with no
  titlebar to intercept top-area clicks at all still couldn't resize from
  a plain top-edge drag - added.
- **Cursor movement alone triggers a full frame-callback broadcast to every
  mapped window on an output, not just ones near the cursor.** Found live:
  `wezterm-gui --class scratchpad` confirmed via repeated `ps` sampling to
  be sustaining ~140% CPU continuously on an otherwise-idle session, on a
  machine already under real memory pressure (1.3GB swap in use of 3.7GB
  total). Root cause: the has-damage gate added earlier (frame callbacks
  only sent `if has_damage`, replacing a prior unconditional-every-frame
  send) operates on a single output-wide boolean. Moving the pointer
  legitimately damages the small region around it (old position needs
  redrawing without the cursor, new position with it - this part of
  smithay's damage tracking is correct and not a bug), but that alone
  currently marks the whole output "damaged" and every mapped window gets
  told to redraw, whether or not the cursor is anywhere near its content.
  Ruled out one candidate mechanism directly from
  `smithay-0.7.0/src/backend/renderer/element/memory.rs`:
  `MemoryRenderBufferRenderElement::from_buffer()`'s `Id` is cloned from
  the underlying `MemoryRenderBuffer`'s own stable id, not freshly
  generated per call, so cached buffers (cursor bitmaps, per-window
  decoration/border buffers) do have stable identity across frames --
  the damage is real, not an artifact of identity churn. **Fixed** with the
  per-window damage-region intersection this entry originally called for,
  turns out `smithay::backend::renderer::damage`'s own `RenderOutputResult`
  already hands back the exact physical-space damage rectangles it drew
  from (`.damage: Option<&Vec<Rectangle<i32, Physical>>>`) - no second
  damage tracker or double render needed. `elements.rs`'s
  `windows_touched_by_damage` filters `space.elements()` down to windows
  whose `Space::element_geometry` (converted to physical space via the
  output's own scale) overlaps at least one of those rects, and both
  backends' frame-callback dispatch now iterates that instead of every
  mapped window. udev.rs's per-head loop had to carry the damage rects
  alongside each presented `Output` into `presented` (previously just
  `Vec<Output>`), since the frame-callback loop runs after `udev`'s mutable
  borrow ends and needs that frame's damage by then. **Verified live**: the
  exact reproducer from the original finding (`wezterm-gui --class
  scratchpad`, idle) measured at 4-6% CPU on the real session after this
  build went live, down from the ~140% recorded before the fix.
  **Follow-up regression, found and fixed the same day**: narrowing which
  windows get a callback to damage-overlap alone can *starve* a window
  instead of merely under-notifying it. `send_frame` only answers a
  *pending* `wl_surface.frame` request; GTK's frame-clock model (Firefox's
  Wayland vsync source included) paces every repaint through that
  callback, even the very first one after being idle - there is no
  "just commit immediately" fallback path. If a window's *own* new content
  is what would produce the next frame's damage, but producing it needs a
  callback this filter is withholding because the *previous* frame's
  damage didn't overlap it, nothing ever arrives to unstick it - reported
  live as clicks in Firefox intermittently doing nothing until the cursor
  was moved again (moving the cursor across the window is what
  incidentally supplied overlapping damage). Fixed by adding
  `always_notify` to `windows_touched_by_damage`: the focused window and
  whatever window is currently under the pointer always get a callback,
  damage-overlap or not - a fixed, small cost (at most two windows)
  covering exactly the two cases user input targets.
  **Second follow-up, same day**: that first version still didn't work --
  `always_notify` was folded into the loop over `presented` (the outputs
  that actually had damage this tick), so it only ever ran on a tick that
  *already* had damage from something else. The one tick it needed to run
  on - the output has *no* damage at all, cursor stationary, nothing else
  happening - is exactly the tick that loop never executes for. Reported
  live as clicks in Firefox still doing nothing at all, not intermittently.
  Fixed by moving the `always_notify` frame-send into its own pass, after
  and independent of the `presented` loop, run unconditionally (only
  gated on `!locked`) every tick regardless of whether anything presented.
  `windows_touched_by_damage` itself went back to a pure damage-overlap
  filter with no `always_notify` parameter, now that the two mechanisms
  are fully independent - see its doc comment.
- **Two precise, single-function omissions, found in the same investigation
  (a codebase-wide audit prompted by "clicking still doesn't work" and
  "window bars still look detached" both persisting after everything
  above), that together explain both symptoms for the two cases they cover
  (native Wayland windows; XWayland windows) far more completely than the
  frame-callback fixes above did on their own:**
  - **Drag/resize end never told the compositor about the final snap.**
    `handle_pointer_button`'s button-release branch called
    `WindowManager::end_drag`/`end_resize` and stopped - it never called
    `sync_geometry` afterward. `end_drag` can snap the geometry one more
    time after the last `update_drag` already moved the window
    (`SmartPlacement::snap_zone`: dragging to a screen edge or the top,
    exactly the ordinary "tile left/right" and "drag-to-maximize"
    gestures, not a rare corner case). The border and titlebar redraw
    fresh from live `Window.geometry` every single frame, so they jumped
    to the snapped rect immediately; the client's actual mapped surface,
    driven only by `sync_geometry`'s `space.map_element`/
    `xdg_toplevel.configure`, stayed wherever the drag physically
    stopped - decoration visibly detached from its own window's content,
    persisting until something unrelated (any keypress, a new window)
    happened to trigger a `dirty`-driven resync. Click routing desynced
    the same way in the meantime: `hit_test`/`window_at` read the
    now-snapped geometry while `space.element_under` still read the stale
    pre-snap position, so clicks in the visually-snapped zone resolved
    against the wrong rect. The X11 backend already did this correctly
    (`crates/x11/src/lib.rs`'s `ButtonRelease` handler calls
    `sync_geometry` right after `end_drag`/`end_resize`) - `input.rs` is
    explicitly the module shared by both backends for exactly this kind of
    logic, and this one call site never got the same fix ported over.
    Fixed by capturing `wm.focused_id()` before ending the drag/resize
    (reliable: `start_drag`/`start_resize` both focus the window they
    grab, and nothing else can change focus while a grab has the pointer
    captured) and calling `state.sync_geometry(id)` after, mirroring the
    X11 pattern exactly.
  - **XWayland windows were never reconfigured past their initial map.**
    `sync_geometry` only had a branch for `w.toplevel()` (native
    xdg-shell) - there was no `w.x11_surface()` branch calling
    `X11Surface::configure()` at all. `space.map_element` still moved
    smithay's own tracked position (hit-testing/stacking stayed nominally
    consistent) and the border/titlebar still redrew at the new
    `Window.geometry` (both read it fresh every frame), but the real X11
    client window was never told to move or resize - confirmed by
    grepping the whole crate: `X11Surface::configure()` was called exactly
    once anywhere, at initial mapping (`xwayland.rs`'s
    `finish_x11_window_setup`), never again after. Every drag, resize,
    maximize, edge-snap, or tiling re-layout of an XWayland-backed app
    (any X11-only client - xterm, many GTK3/Qt5/Java apps, anything not
    forced into native-Wayland mode) left its actual content frozen at its
    original mapped size and position *forever*, while srdwm's own border
    and titlebar moved freely around it - a second, independent, and
    steady-state-permanent (not just a momentary post-drag glitch) cause
    of "decoration doesn't look connected to the window." Fixed by adding
    the missing branch, calling `x11.configure()` with the current
    geometry. Unlike the xdg-shell branch (gated on `size_changed`, since
    xdg-shell position is a purely compositor-side concept never
    communicated to the client), the X11 branch reconfigures on *every*
    `sync_geometry` call regardless of whether size changed - an X11
    client's on-screen position is real window state it has to be told
    about on every move, the same way a real X11 window manager sends
    continuous `ConfigureNotify` during an interactive drag.
  Neither fix is a refactor: both are a handful of lines in one function
  each, and both had an already-correct reference pattern sitting
  elsewhere in the same codebase to copy (X11's own `ButtonRelease`
  handler; XWayland's own initial-map `configure()` call) rather than a
  new mechanism to invent. The underlying process gap worth noting: there
  is no invariant (test or type-level) forcing every geometry-mutating
  call site to route through `sync_geometry` - that is exactly the class
  of bug that let both of these happen and both survive `manager.rs`'s
  own unit tests (which verify the snap/placement math in isolation and
  never cross the boundary into "did the backend actually get told").
- **`xdg_popup` was entirely unimplemented - not a missing feature so much
  as a client-hanging bug.** `new_popup` was a bare no-op: no
  `send_configure`, no tracking, no rendering. Per xdg-shell, a popup's
  first `wl_surface.commit()` cannot proceed without a prior
  `xdg_surface.configure`; GTK4's Wayland backend (and most real toolkits)
  blocks that commit in a synchronous roundtrip waiting for it, so every
  popup hung its client forever, not intermittently. GTK4 implements both
  tooltips and `Gtk.Popover` as `xdg_popup` - a peer session's gdb
  backtrace (blocked in `wl_display_dispatch_queue` under
  `gtk_widget_show`) traced this to exactly that path, and the AGS shell
  alone has 74+ tooltip/popover usages, so this was hit constantly, just
  never attributed (hovering a bar icon is not a memorable action).
  Fixed: `new_popup` now sets pending geometry from the positioner
  (`PositionerState::get_geometry()`, unconstrained - see below) and
  configures; `reposition_request` re-geometries and
  `send_repositioned`s; `commit()` advances `PopupManager`'s
  unmapped-to-mapped tracking and prunes dead ones. Rendered as ordinary
  surface-tree elements (`render_elements_from_surface_tree`, same
  mechanism a client-set cursor image already used) positioned at their
  parent toplevel's on-screen location plus `PopupManager`'s tracked
  offset, added to `custom_elements` alongside the cursor/borders/
  decorations - popups are never `space.map_element`'d, so without this
  `render_output`'s automatic per-space-element rendering would never see
  them even once configured. One thing this does NOT do: geometry is not
  clamped to the output (`PositionerState::get_unconstrained_geometry`
  needs a target rect in the parent's surface-local space, a real
  follow-up); a popup positioned very close to a screen edge may render
  partly off it. Cosmetic, not a hang.
  **Follow-up, same investigation**: implicit grab + dismiss-on-outside-
  click (`grab()`) was left as a no-op believing `PopupManager::grab_popup`
  needed `CompState`'s `SeatHandler::KeyboardFocus` to implement
  `WaylandFocus + From<PopupKind>`, which it supposedly didn't. Rechecked
  while implementing `move_request`/`resize_request` below (same trait,
  adjacent methods) - it already did: `KeyboardFocus`/`PointerFocus` are
  both plain `WlSurface`, smithay provides `impl From<PopupKind> for
  WlSurface` itself, and `WlSurface: From<WlSurface>` trivially. No
  blocker ever existed by the time this got rechecked; the earlier note
  just hadn't been revisited. Fixed: `grab()` now calls `grab_popup`,
  installs the returned `PopupGrab`'s default `PopupKeyboardGrab`/
  `PopupPointerGrab` on the seat, and lets smithay's own default grab
  implementations handle the dismiss-on-outside-click behavior (their own
  documented purpose).
- **`zwlr_foreign_toplevel_handle_v1.app_id`/`.title` were sent as empty
  strings on every window, always** - see `PANEL_SUPPORT_TODO.md`'s P1
  section for the root cause and fix (re-read on every `commit()` rather
  than once at role-assignment time). Both `Window.app_id`/`.title` and
  everything downstream of them (`srd.rule({ class = ... })`, the
  foreign-toplevel protocol) were affected identically, since both read
  the same fields populated the same way.
- **Titlebar button icons were drawn one full button-width left of where
  clicking them actually registered.** `decoration.rs`'s three
  `draw_*_icon` calls passed `right_offset` values of `height`, `height*2`,
  `height*3` for close/maximize/minimize; `button_box`'s formula
  (`right = width - right_offset`) means those land the icons in the
  *second*, *third*, and *fourth* button-width squares from the right
  edge, not the first, second, and third squares `ResizeEdge::hit_test` in
  `crates/core/src/window.rs` actually assigns to Close/Maximize/Minimize.
  Concretely: the true Close hit-zone (the rightmost `TITLEBAR_HEIGHT`
  pixels) was visually blank; the drawn "X" icon sat in the square that
  hit_test treats as Maximize; the drawn square icon sat in the square
  hit_test treats as Minimize; the drawn minimize line sat past all three
  button bands, in plain drag territory. Every titlebar button was
  therefore one click-target to the right of its own icon. Fixed by
  changing the three offsets to `0`, `height`, `height*2` respectively, so
  each icon lands in the same square hit_test assigns it. A regression
  test (`button_icons_are_drawn_in_the_squares_hit_test_assigns_them`)
  renders a titlebar, finds each icon's drawn square, and asserts
  `hit_test` at that square's centre reports the matching button --
  confirmed it fails on the pre-fix offsets and passes on the corrected
  ones.
- **A bordered window's titlebar rounded its own top corners while the
  border frame around it stayed square**, leaving a small gap at exactly
  those two corners where the rounded cutout exposed whatever was behind
  the window instead of the border - undermining `border_strips`, whose
  whole purpose was making the titlebar read as part of the window rather
  than a strip bolted on top of it (reported live as "bars/decorations
  don't feel part of the window"). `render_titlebar` now takes a
  `round_corners: bool`; `redraw_decoration_buffer` passes
  `w.border_width == 0`, so a bordered window's titlebar stays flush-square
  with its border (no gap, fully continuous frame) and a borderless window
  keeps the rounded top corners it had no square frame to clash with.
- **Window borders never actually rendered on the real (udev) backend --
  every window looked like a bare titlebar floating over content, with no
  frame at all.** Confirmed by pixel-sampling a live screenshot: the
  transition from desktop background straight to the titlebar's own pixels
  had zero border-coloured pixels in between, not merely a faint or
  dim border - literally none. Root cause: `border_strips` rendered each
  strip as a cached 1x1 solid-colour `MemoryRenderBuffer`, stretched to the
  strip's real size via `MemoryRenderBufferRenderElement::from_buffer`'s
  `size` override (upscaling a tiny buffer, the same trick the cursor
  bitmaps use). Smithay's `PixmanRenderer` - the udev backend's software
  renderer - hardcodes `src_image.set_repeat(Repeat::None)` on every
  imported texture with no per-call override in 0.7.0. Combined with
  bilinear upscale filtering, sampling a 1x1 image stretched across a much
  larger destination has no valid neighbouring texels under `Repeat::None`
  to blend against, so it rendered fully transparent. Decorations (the
  titlebar bitmap) never hit this because they pass `size: None` - a
  real, non-stretched, buffer-native-size bitmap, `scale == 1.0`, so
  pixman's transform-and-sample path is skipped entirely. The winit (GPU/
  GLES) backend was never affected: OpenGL's texture sampler returns a 1x1
  texture's single texel regardless of wrap mode, so the exact same code
  "worked" there by accident - meaning this bug was invisible in every
  nested/dev-session test and only ever showed up on real hardware, the
  one backend that actually matters for daily driving. Fixed by rendering
  borders as `SolidColorRenderElement` instead (`smithay::backend::renderer::element::solid`,
  new `Solid` variant on `OverlayElement`) - a native `Frame::draw_solid`
  fill with no texture import or sampling involved at all, so this backend
  difference cannot affect it. Also removed the now-dead 1x1-buffer
  machinery it replaced (`CompState::border_buffers`, `border_buffer_for`,
  `decoration::solid_pixel`). Not yet re-verified live on the real udev
  backend - needs a restart.
- **`zwp_linux_dmabuf_v1`** - no client could ever hand over a GPU buffer;
  GTK4 in particular tried to open a DRM render node to allocate one
  anyway, found no dmabuf global to negotiate through, and crashed instead
  of falling back gracefully, forcing `GSK_RENDERER=cairo` (full software
  rendering) on every GTK4 client just to survive. Full account, including
  why this works at all given the udev backend's `PixmanRenderer` has no
  GPU pipeline, in `docs/PANEL_SUPPORT_TODO.md`'s P0.3. Live-verified in an
  isolated nested instance that the global is now advertised and a real
  client connects/maps normally; full GPU-allocated-buffer round trip
  needs a retest on real hardware, this sandbox has no working DRM render
  node to allocate one against.
- **`xdg_activation_v1`** - a launcher's freshly-spawned app had no way to
  raise itself once its window mapped; it just opened unfocused behind
  everything. `request_activation` reuses the same `focus_window` path a
  dock's foreign-toplevel "activate" already goes through. Full account in
  `docs/PANEL_SUPPORT_TODO.md`'s P1. Live-verified the same way as dmabuf
  above: the global advertises correctly in an isolated nested instance,
  a real client connects/maps normally.
- **The entire compositor is one thread: input dispatch, client protocol
  dispatch, and rendering all run serially inside `UdevPlatform::poll_events`
  (`crates/wayland/src/udev.rs`, `~line 1149`) every tick.** Found while
  investigating a fresh "clicking still doesn't work" report by checking
  `~/.local/state/wm-session-*.log` for anything libinput itself had to say:
  it did --
  ```
  libinput error: event10 - Logitech USB Optical Mouse: client bug: event
  processing lagging behind by 831ms, your system is too slow
  libinput error: client bug: timer button-debounce-...: scheduled expiry
  is in the past (-216ms), your system is too slow
  libinput error: WARNING: log rate limit exceeded (5 msgs per 3600000ms).
  Discarding future messages.
  ```
  i.e. libinput's own watchdog saw the compositor fail to call back into it
  for most of a second, on the *mouse and keyboard event sources both* --
  and then it rate-limited itself to 5 messages/hour, so the *absence* of
  further such lines in the log is not evidence this stopped happening,
  only that libinput stopped reporting it. Correlated with timestamps of
  `libEGL warning: failed to get driver name` and
  `[WARN audioipc2_server::server] Promotion of content process thread to
  real-time` right alongside it - `audioipc2` is Firefox's own audio IPC
  subsystem, meaning this specific incident lines up with Firefox's
  multi-process startup (GPU/audio/content processes all spawning at once)
  putting the whole system under enough transient CPU/memory pressure that
  even the compositor's own event loop missed its scheduling window --
  consistent with this being a genuinely resource-constrained machine (see
  the earlier-documented 1.3GB-swap finding). A direct IPC round-trip check
  (`srd clients`, timed) immediately after finding this came back at a
  steady 10-20ms, so the loop is not *currently* stalling - this specific
  831ms incident is not, by itself, an explanation for an ongoing "clicking
  doesn't work" complaint reported well after it happened. It is still a
  real, reproducible architectural weak point: because `poll_events` calls
  `event_loop.dispatch(16ms timeout)` *then* runs `render_udev_frame()`
  synchronously before looping back, any single slow render pass (a burst
  of new windows/layer-surfaces all redrawing at once, say) directly adds
  to input latency, with nothing to prioritize input processing over
  rendering when both are contending for the same thread. Not fixed --
  the real fix (moving libinput's event source dispatch to its own thread,
  independent of the render cadence) is a genuine, non-trivial
  restructuring of the event loop, not a targeted patch, and is exactly
  the kind of thing worth scoping as a deliberate follow-up rather than
  rushing into the same pass that found it.
- **`toggle_fullscreen`'s exit path hardcoded `Window.decorated = true`
  unconditionally - the actual root cause behind persistent "clicking
  doesn't work" and "decoration looks detached" reports on Firefox, found
  by adding temporary diagnostic logging to the click-routing path and
  correlating a live "clicked the back button, it minimized the window
  instead" report against it.** The log showed `hit_test` returning
  `Some((firefox_id, TitlebarHit::Drag))` for a click on Firefox's own
  toolbar - impossible if `Window.decorated` were actually `false`, which
  is what `srd.rule({ class = "firefox" }, { decorated = false })` sets it
  to (Firefox draws its own close/minimize/maximize row and negotiates
  client-side decoration once, at startup, via `zxdg_toplevel_decoration_v1`
  - see the `XdgDecorationHandler` doc comment). Root cause: entering
  fullscreen correctly saved nothing and set `decorated = false`
  (fullscreen has no titlebar, always), but *exiting* fullscreen hardcoded
  it back to `true` regardless of what it was before entering - correct
  for a normally-decorated window, wrong for anything a rule set to
  `false`. Since Firefox never renegotiates its decoration mode again
  after the initial handshake, nothing would ever set it back. `Super+f`/
  `Super+m`/`Super+z` are all bound to `srd.window.fullscreen()` in the
  shipped config - an easy, plausible accidental press during any long
  session, not a rare edge case. Once `decorated` was wrongly `true`,
  every click in what srdwm now (incorrectly) treated as the top
  `TITLEBAR_HEIGHT` band of Firefox's window got swallowed as a fake
  drag/button hit instead of ever reaching the client - matching both
  "clicking doesn't work" (band clicks silently absorbed) and "I have to
  click somewhere else for it to register" (only clicks *below* the band
  ever reached Firefox) exactly. Fixed with a new `Window.restore_decorated:
  Option<bool>` field, following the exact same save/restore pattern
  `restore_geometry` already uses for `toggle_maximize`/`toggle_fullscreen`'s
  geometry: entering fullscreen now saves the current `decorated` value
  before forcing `false`, exiting restores it instead of hardcoding `true`.
  Regression test:
  `fullscreen_round_trip_restores_a_client_side_decorated_window_to_undecorated`.
  Diagnostic logging (temporary, since removed - see below) confirmed no
  client-side keyboard double-dispatch at the point srdwm receives events
  from libinput (every key logged shows a clean, single Pressed/Released
  pair) - the separately-reported "typing feels sensitive, letters
  double" bug is not the same mechanism as this one. It was tracked down
  later in this same pass to a too-short repeat delay, not server-side
  duplication; see the `REPEAT_DELAY`/`add_keyboard` entry further down.
- **`maximize_request`/`unmaximize_request`/`fullscreen_request`/
  `unfullscreen_request`/`minimize_request` were all still smithay's
  default no-op (or configure-only) implementations - found immediately
  after the `toggle_fullscreen` fix above, checking what else in
  `XdgShellHandler` might share its blast radius.** These are the
  *client-initiated* equivalent of the pointer-driven titlebar-button
  handlers already in `input.rs`: a client's own window-menu "Maximize",
  pressing F11, an HTML5 video going fullscreen, or (for a client that
  negotiated client-side decoration and draws its own titlebar, like
  Firefox) that titlebar's own maximize button - all ask the compositor
  to actually perform the state change via these requests rather than the
  compositor noticing on its own. Left unimplemented, every one of them
  was a silent no-op, indistinguishable from a client bug: the button
  visibly existed and could be clicked, nothing happened, no error. Fixed
  by wiring all five to the same `WindowManager` calls (`toggle_maximize`,
  `toggle_fullscreen`, `minimize_window`) the pointer path already uses,
  including the same `redraw_decoration_buffer`-before-`sync_geometry`
  ordering `set_decorated_from_mode` established for anything that flips
  `Window.decorated` (fullscreen does; maximize/minimize don't).
- **`move_request`/`resize_request` were *also* still smithay's default
  no-op implementations - the single biggest gap found in this whole
  investigation.** This is how a client-side-decorated window gets dragged
  or resized by its own titlebar/edges at all. A window srdwm draws its
  own decoration for never needed this (`TitlebarHit::Drag`/`Resize` in
  `input.rs` detect the click directly, since srdwm owns those pixels),
  but a window that negotiated client-side decoration and draws its own
  titlebar - Firefox, and most GTK4 apps by default - handles the click
  itself and then asks the compositor to actually perform the move/resize
  via exactly these two requests. Left unimplemented, dragging or
  resizing *any* such window by its own chrome did nothing at all - the
  only way to reposition one was the modifier+drag-anywhere gesture
  (`bindm`), which most users have no reason to know exists and doesn't
  cover resize-from-a-specific-edge at all. Fixed by reusing the exact
  same `WindowManager::start_drag`/`start_resize` the pointer-driven
  titlebar handlers call: `handle_pointer_position`/`handle_pointer_button`
  already drive any in-progress drag/resize to completion on subsequent
  motion/release regardless of what started it, so no smithay pointer
  grab was needed here at all, just the same start call from a different
  trigger. `xdg_toplevel::ResizeEdge`'s 8 real edge values map directly
  onto `srdwm_core::ResizeEdge`; the protocol's `None` value is a no-op
  (nothing sensible to default it to that wouldn't be a guess).
- **`PopupManager::grab_popup` was believed blocked on `CompState`'s
  `SeatHandler` associated types not satisfying its `WaylandFocus +
  From<PopupKind>` bound, and `grab()` was left a no-op on that basis --
  rechecked while implementing `move_request`/`resize_request` above
  (same trait, adjacent methods) and the bound was already met**:
  `KeyboardFocus`/`PointerFocus` are both plain `WlSurface`, smithay
  itself provides `impl From<PopupKind> for WlSurface`, and
  `WlSurface: From<WlSurface>` trivially. No blocker ever existed by the
  time of this pass; the earlier note just hadn't been revisited since it
  was written. Fixed: `grab()` now calls `grab_popup`, installs the
  returned `PopupGrab`'s default `PopupKeyboardGrab`/`PopupPointerGrab` on
  the seat, and lets smithay's own default grab implementations handle
  dismiss-on-outside-click (their documented purpose) - a popup (tooltip,
  dropdown, context menu) now closes when you click outside it, instead
  of only when its own client decides to close it.
- **`popup_targets` (the render path a mapped `xdg_popup` needs to find
  its on-screen position) only ever scanned toplevel windows for a popup
  parent, never layer-shell surfaces - so any popup parented to a bar or
  launcher's own layer surface was fully functional and completely
  invisible.** `zwlr_layer_surface_v1.get_popup` (what a bar's own
  dropdown/context menu uses to parent a popup to itself) is a wholly
  separate request from `xdg_surface.get_popup`, but both funnel into the
  same `XdgShellHandler::new_popup`/`commit()` tracking path regardless of
  which kind of surface ends up as the parent - so the popup was always
  correctly configured and tracked (a bar's dropdown would open and
  accept clicks), it just never had a `PopupTarget` to render relative to.
  Fixed by also gathering every mapped layer-shell surface, across every
  output, as a candidate popup parent alongside toplevel windows.
- **The same six no-op requests found missing from `XdgShellHandler`
  (`maximize_request`/`unmaximize_request`/`fullscreen_request`/
  `unfullscreen_request`/`minimize_request`, plus `unminimize_request`,
  which has no native-Wayland equivalent) were equally unimplemented for
  XWayland's `XwmHandler` - the EWMH/ICCCM equivalent
  (`_NET_WM_STATE_MAXIMIZED_VERT`/`_HORZ`, `_NET_WM_STATE_FULLSCREEN`,
  `_NET_WM_STATE_HIDDEN`).** `move_request`/`resize_request` were
  *already* correctly implemented for XWayland (`crates/wayland/src/
  xwayland.rs`), which is what made the state-toggle half of this gap easy
  to miss - the drag/resize half already had parity with native Wayland;
  the maximize/fullscreen/minimize half didn't. Any XWayland app's own
  window-menu maximize/minimize/fullscreen action (older GTK3/Qt5/Java
  apps, anything not forced into native-Wayland mode) was a silent no-op.
  Fixed the same way as the native-Wayland five: wired to the same
  `WindowManager` calls, with the same `redraw_decoration_buffer`-before-
  `sync_geometry` ordering for fullscreen specifically (it flips
  `Window.decorated` for an XWayland window's `Window` entry exactly the
  same way it does for a native one).
- **`zwlr_foreign_toplevel_management_v1` wasn't re-broadcasting
  maximize/fullscreen/minimize state changes that didn't originate from
  the protocol itself.** `foreign_toplevel::send_state` was already called
  from `set_maximized`/`set_minimized`/`announce`/`update_activated`, but
  never from the pointer-driven titlebar handlers in `input.rs`
  (`TitlebarHit::Maximize`/`Minimize`, and double-click-to-maximize on
  `TitlebarHit::Drag`), nor from any of the client-initiated
  `XdgShellHandler`/`XwmHandler` requests just fixed above - so a dock or
  panel's own maximized/minimized indicator went stale the moment a user
  clicked a titlebar button directly, or a client asked for the state
  change itself, instead of driving it through the dock. Fixed by adding
  the same `send_state` call to all eleven sites (3 in `input.rs`, 5 in
  `protocols.rs`, 6 in `xwayland.rs` - `unminimize_request` included,
  even though it calls `restore_window` rather than `minimize_window`).
  One trigger remains unfixed: a *compositor keybinding*
  (`srd.window.maximize()`/`.fullscreen()`/`.minimize()`) still doesn't
  re-broadcast, because `crates/config` (the Lua engine, shared with the
  X11/macOS backends) only ever holds a `WindowManager` reference, never
  `CompState` - it has no way to reach a Wayland-protocol-specific
  function that lives one layer up, in the crate that depends on it. See
  `docs/PANEL_SUPPORT_TODO.md`'s P1 section for the full note; closing it
  needs a small design decision (a dirty-window callback, or a periodic
  diff/broadcast pass in `CompState`'s own tick), not a one-line patch.
- **`zwlr_foreign_toplevel_handle_v1.set_fullscreen`/`.unset_fullscreen`
  were silent no-ops**, and `Fullscreen` was missing from the state bytes
  the handle ever sent - so a dock had no way to ask a window to
  fullscreen, and couldn't tell if one already was, even though
  `toggle_fullscreen` was exactly as reachable from `foreign_toplevel.rs`
  as `toggle_maximize` already was (both are plain `WindowManager`
  methods). Left over from before `move_request`/`resize_request` and the
  five `XdgShellHandler` fullscreen/maximize/minimize requests were fixed
  earlier in this pass - at the time this module was originally written,
  the doc comment's given reason ("no `srd.window.fullscreen()`-equivalent
  entry point reachable from here") was accurate; it stopped being true
  once those fixes landed, and nothing had gone back to revisit it. Fixed
  by adding `set_fullscreen`, mirroring `set_maximized`'s shape (including
  the same `redraw_decoration_buffer`-before-`sync_geometry` ordering
  fullscreen always needs), and adding `State::Fullscreen` to
  `state_flags`/`send_state_to`. The request's `output` argument is
  ignored as a hint only, matching `fullscreen_request`'s `_output` in
  `protocols.rs`.
- **`update_cursor_shape` left the resize cursor stuck once the pointer
  moved from one of srdwm's own decoration edges straight onto the bare
  desktop.** Its early-return for "over a client surface, let the client
  drive its own cursor" didn't distinguish that case from "over nothing at
  all" - on the empty desktop there is no client to ever call
  `set_cursor` and reset it, so whatever named icon was showing (a resize
  arrow, most noticeably) stayed forever until the pointer happened to
  cross back onto some client's content. Fixed by threading through
  whether any window is actually under the pointer (`over_content`,
  computed once in `handle_pointer_position` from the same `under` lookup
  already used for click routing) and falling back to `CursorIcon::Default`
  instead of returning early when it's `false`.
- **`cursor.rs` only had dedicated art for two shapes (text entry, the four
  resize directions) - every other named shape a client can request,
  including `CursorIcon::Pointer` (the hand shown over every hyperlink and
  most other clickable non-form controls, by far the most common named
  shape after the plain arrow), silently fell back to the same arrow shown
  everywhere else.** Added three more built-in bitmaps following the same
  solid-black-plus-white-halo style the resize/text shapes already use:
  `crosshair_bitmap` (a plain centered `+`), `move_bitmap` (a four-way
  arrow, built the same way `diagonal_resize_bitmap`'s arrowheads are, just
  aimed at all four cardinal directions), and `pointer_bitmap` (a blocky
  pointing hand - an upright finger over a wider palm block, hotspot at
  the fingertip via a new `POINTER_HOTSPOT` constant rather than the
  centered hotspot every other non-arrow shape uses). Everything else
  (grab, wait, help, and the dozen or so rarer named shapes) still falls
  back to the arrow - deliberately scoped to the shapes common enough to
  be immediately noticeable when wrong, same reasoning the module's own doc
  comment already gives for why it isn't a full XCursor theme.
- **`ext_workspace_manager_v1` (`crates/wayland/src/workspace.rs`) turned
  out to already be fully implemented and wired into both backends'
  globals - `docs/PANEL_SUPPORT_TODO.md`'s P1 list had it marked as not
  done, which was simply stale.** Auditing every `WindowManager::
  switch_workspace` call site (the same technique that found `foreign_
  toplevel`'s broadcast gap) turned up a real, smaller gap instead: the
  `SUPER+scroll` workspace-cycle gesture in `input.rs` called
  `switch_workspace` directly without calling `workspace::
  broadcast_active_workspace` afterward, so a dock's workspace pill only
  ever tracked switches driven through this protocol's own `activate`
  request - identical shape to the maximize/fullscreen/minimize gap fixed
  above, just for workspaces instead of window state. Fixed by making
  `broadcast_active_workspace` `pub(crate)` and calling it from the scroll
  gesture. The `srd.workspace.next()`/`.prev()`/`.switch()` Lua API has the
  identical gap for the identical reason (`crates/config` only ever holds a
  `WindowManager` reference, never `CompState`) and remains open, same as
  the Lua-keybinding gap already noted for maximize/fullscreen/minimize.
- **The keyboard repeat delay was hardcoded to 200ms - likely the real
  explanation behind the separately-reported "typing feels sensitive,
  letters double" symptom, which the earlier investigation above confirmed
  was not server-side event duplication.** Found by comparing against
  Hyprland's default (`repeat_delay = 600`), after a live report that
  typing felt noticeably more sensitive under srdwm than under other
  compositors on the same hardware - the machine already had a Hyprland
  config on disk (`~/.config/hypr/hyprland.conf`) confirming it had
  actually been used there before, on this same keyboard, without that
  complaint. `repeat_info` is sent to a client once and the client manages
  its own repeat timer entirely on its own from then on (confirmed reading
  `smithay`'s `wayland/seat/keyboard.rs`: `repeat_info` is only ever sent
  on bind and via `change_repeat_info`, never re-driven per keystroke) --
  so a 200ms delay means any key held even slightly past a fifth of a
  second, which is well within normal variance in how long a real
  keystroke's finger-down/finger-up dwell actually is, starts the client's
  own repeat and inserts an unintended extra character. This is
  indistinguishable from "double-typing" to the person typing, entirely
  client-side, and was invisible to the earlier diagnostic logging (which
  only watched the compositor's own reception/dispatch, both of which were
  and remain genuinely clean). Changed both backends' `add_keyboard(..,
  200, 25)` to `600, 25`, and `state.rs`'s `REPEAT_DELAY` constant (used
  for the compositor's own held-keybinding repeat, kept in sync with the
  seat's setting by design) to match. The double-typing bug is no longer
  listed as unexplained - this is the leading fix for it, though not yet
  live-verified against a real typing session (see the "don't restart
  without permission" note elsewhere in this doc/session).
- **`xdg_toplevel`'s `Activated` state was never sent to any client at
  all - found investigating a live report that a single open window,
  with nothing else it could possibly be losing focus to, still didn't
  look focused.** `set_keyboard_focus` (the one chokepoint every focus
  change already goes through) only ever called `keyboard.set_focus`,
  which delivers `wl_keyboard.enter`/`leave` - real input focus - but
  says nothing about `xdg_toplevel` state. The only place `Activated` was
  ever touched was `foreign_toplevel::update_activated`, which is a
  completely different, dock-facing protocol
  (`zwlr_foreign_toplevel_handle_v1`), not the client's own window. GTK4/
  libadwaita's `:backdrop` CSS pseudo-class - and most other toolkits'
  equivalent - keys off exactly the state that was never sent, so any
  client that draws its own focus indicator this way (a differently
  colored/styled titlebar when "unfocused") looked permanently unfocused
  regardless of real keyboard focus or window count: a lone window is
  never *not* the focused one, so this was actually the easiest case to
  notice it in, not a coincidence. Fixed with `set_window_activated`, a
  new helper next to `set_keyboard_focus` that calls `smithay::desktop::
  Window::set_activated` (which already unifies the xdg-shell and X11
  cases - sets the pending state for the former, talks straight to the X
  connection for the latter) for both the window losing focus and the one
  gaining it, sending a `send_configure` for the native-Wayland case since
  `set_activated` alone only queues the pending state.

- **Every `class`-based `srd.rule` silently never matched a native Wayland
  window - the actual root cause of Firefox's persistent double titlebar,
  and (same mechanism) of every other rule in the shipped `rules.lua`
  quietly not applying to native-Wayland apps (floating for `mpv`/`vlc`/
  `pavucontrol`, workspace assignment for `discord`/`Spotify`, etc.).
  Found by nesting srdwm under the live session with an isolated
  `XDG_RUNTIME_DIR` (symlinking only the host socket in, under a name
  that doesn't collide with the guest's own auto-picked server socket --
  reusing the same name once caused the *next* nested launch to fail to
  connect out, since the guest's own `bind_auto` had overwritten the
  symlink) and screenshotting a real Firefox window via `grim`.**
  `WindowManager::add_window` matches rules once, against whatever
  `title`/`app_id` the `Window` already has - but `new_managed_window`
  populates those from `XdgToplevelSurfaceData` at `get_toplevel` role-
  assignment time, which is *before* essentially every real client's
  `set_title`/`set_app_id`/first commit, not racily but every single
  time. So `add_window` always evaluated rules against an empty
  `app_id`, and `srd.rule({ class = "firefox" }, { decorated = false })`
  - meant to stop srdwm drawing a second titlebar over Firefox's own --
  could never match. `sync_toplevel_metadata` already re-read the real
  `app_id`/`title` on every commit (for the foreign-toplevel broadcast),
  but never gave rules a second chance. Fixed with `Window::
  rules_applied` (sticky once real identity has been evaluated, so a
  later unrelated title change - a browser tab switching - can't
  re-match and re-apply) and `WindowManager::reapply_rules_if_pending`,
  called from `sync_toplevel_metadata` once `title`/`app_id` actually
  change *and a rule actually matched* (see the next entry for why that
  second condition matters), followed by the same `redraw_decoration_
  buffer`-then-`sync_geometry` ordering `set_decorated_from_mode` uses for
  any `decorated` flip. XWayland was never affected: `map_window_request`
  (`xwayland.rs`) sets `app_id` from `X11Surface::class()` synchronously,
  before `add_window` runs. Regression test: `class_rule_applies_once_
  app_id_is_known_after_creation`. Screenshot-verified end to end in the
  isolated nested instance: Firefox's own CSD tab strip is now the only
  chrome, with no srdwm-drawn band above it.
- **The nested (winit) backend's own debug window was titled "Smithay"**
  - the hardcoded default `smithay::backend::winit::init()` uses
  internally, surfaced while investigating the above and reasonably
  read as "this compositor is just smithay with no work of its own on
  top" rather than the dev-window cosmetic default it actually is.
  Switched to `winit::init_from_attributes` with `.with_title("srdwm")`,
  the only behavioral difference from `init()`.
- **`reapply_rules_if_pending` calling `sync_geometry` unconditionally on
  every title/app_id change (not just the first, rule-matching one) was a
  real bug in its own right, independent of the fix above.**
  `sync_geometry` calls `Space::map_element`, which - per smithay 0.7.0's
  own source - always re-stacks its target to the top on every call
  (remove, push to the end, stable-sort by `z_index`; every window shares
  the same default `z_index`, and `activate: bool` only toggles the
  `xdg_toplevel` activated *state*, it does not gate the restack). A
  window's title changing is not a user action and has nothing to do with
  raising it - but it happens constantly for perfectly ordinary reasons
  (a browser tab finishing a page load) long after the window's own
  creation. Fixed by having `reapply_rules_if_pending` return whether a
  rule actually matched (`Some`, not just "already evaluated"), and only
  calling `redraw_decoration_buffer`/`sync_geometry` from
  `sync_toplevel_metadata` when it did - the overwhelmingly common case
  (no matching rule) now correctly does neither.
- **FIXED (a later pass, same session) - the window-stacking bug above:
  two overlapping native Wayland toplevels compositing in the wrong
  front-to-back order was real, reproducible, and root-caused by
  instrumenting a locally vendored copy of smithay directly (`vendor/`
  + `[patch.crates-io]`, both removed once done - see the commit/diff
  history if the technique is needed again).** `eprintln!`s in
  `OutputDamageTracker`'s `damage_output_internal` (the element-collect
  loop) and its draw loop, plus one in `sync_geometry`, proved the actual
  mechanism: `smithay::desktop::space::Space::map_element` - 0.7.0's only
  way to update an element's tracked position - *always* re-stacks its
  target to the top of `Space`'s internal order as a side effect, `activate`
  argument or not (there is no "move without restacking" in this smithay
  version). `sync_geometry` calls `map_element` for reasons that have
  nothing to do with raising a window: a title/app_id changing, an
  ordinary resize frame, anything that touches position or size. Two
  windows created moments apart each independently go through their own
  startup title/app_id negotiation, each triggering a handful of
  `sync_geometry` calls purely from that - so whichever one's startup
  sequence happened to settle *last* silently won `Space`'s notion of "on
  top", a race with no relationship to which window `WindowManager` (or
  the user) actually considered focused. This is why it looked like
  "whichever was created first always wins" in early testing and why every
  srdwm-side rendering variable (push order, forced full redraws, buffer
  age) made no difference - none of them touched the actual mechanism.
  Fixed with `CompState::resync_stacking_order`, called right after every
  `map_element` in `sync_geometry`: re-applies `WindowManager.order`
  (already the single source of truth for stacking, and already what
  `hit_test`/`window_at`/rendering itself use) to `Space` via
  `raise_element` in bottom-to-top sequence, so the two can never drift
  apart again regardless of why `sync_geometry` was called. Verified live,
  repeatedly, in an isolated nested instance: two overlapping `wezterm`
  windows with no rules involved, staggered by both 5s and 2s to stress
  the startup-timing race, correctly show the focused one on top every
  time; `srd dispatch focus <id>` on the backgrounded one correctly brings
  its content to the front, confirming this tracks real focus changes and
  isn't just "newest window wins" by coincidence.

## `ext_idle_notify_v1` / `zwp_idle_inhibit_manager_v1` (added this pass)

Both use smithay's own complete, built-in modules (`wayland::idle_notify`/
`idle_inhibit`) - unlike everything else hand-written in this crate against
raw protocol bindings, neither needed that: smithay already ships full
working server-side implementations of both, timers included.

- `IdleNotifierHandler`/`IdleInhibitHandler` implemented for `CompState`
  (`protocols.rs`); `input::notify_idle_activity` calls `IdleNotifierState::
  notify_activity` from all four real input paths (`handle_pointer_position`,
  `handle_pointer_button`, `handle_keyboard_key_event`,
  `handle_workspace_scroll`, which every `PointerAxis` event already routes
  through) - deliberately including while the session is locked, since idle
  activity is about the seat, not about which surface an event reaches.
  Throttled to once per 250ms: `notify_activity` removes and re-inserts a
  calloop timer per live notification on every call with no throttling of
  its own, and idle timeouts are measured in minutes, so nothing needs
  finer resolution than that - the same class of hot-path-per-motion-event
  cost this session's earlier diagnostic-logging regression already proved
  worth being careful around, just cheap enough (in-memory bookkeeping, not
  synchronous I/O) that throttling rather than omitting it was the right
  call.
- `IdleInhibitHandler::inhibit`/`uninhibit` track inhibiting surfaces in a
  new `CompState::idle_inhibiting_surfaces` list and call `IdleNotifierState::
  set_is_inhibited` accordingly. Deliberately not workspace-visibility-aware
  (an inhibiting window on a workspace you've switched away from still
  keeps the system awake) - see that field's own doc comment for the
  reasoning. `remove_window` also clears a window's entry as a safety net:
  smithay's own `IdleInhibitorState` only calls `uninhibit` on an explicit
  `destroy` request, never on ungraceful client death, so a crashed video
  player would otherwise hold the whole system awake forever with no
  client left to ever release it.
- **The winit (nested/dev) backend genuinely has no `calloop` event loop of
  its own at all** (documented in `ipc.rs`'s own module doc comment,
  predating this work) - but `IdleNotifierState::new` requires a real
  `LoopHandle` to register its per-notification timers against, and its
  handler trait must return a real, working state unconditionally (no
  `Option` in the signature), so this couldn't be skipped for that backend
  without either breaking the shared `CompState` type both backends use or
  advertising a global whose events would then simply never fire - worse
  than not having the protocol at all. Gave `WaylandPlatform` (winit.rs) a
  second, narrowly-scoped `calloop::EventLoop<'static, CompState>` used
  for nothing except hosting these timers, dispatched non-blocking
  (`Duration::ZERO`) once per `poll_events` tick alongside the existing
  per-tick work. The udev backend already had a real event loop for this
  to use directly, no new plumbing needed there.

## Placement, dragging, fullscreen-vs-dock, decoration polish (added this pass)

- **`Monitor` conflated two genuinely different rectangles into one
  `geometry` field: the exclusive-zone-shrunk usable area (what
  placement/tiling/maximize should respect) and the output's true full
  rect (what fullscreen - and a window being interactively dragged --
  should be able to reach or cross).** Reported as two symptoms of the
  same cause: fullscreen (`Super+z`) stopped short of a dock's reserved
  strip instead of covering it like every other compositor's fullscreen
  does, and a floating window being dragged could not be moved into that
  strip at all - not merely discouraged, physically unreachable at any
  drag speed or angle, since `update_drag`'s clamp used the same shrunk
  `geometry`. Fixed by adding `Monitor::full_geometry` (defaults to
  `geometry` for any backend not yet taught the distinction), populated
  in `udev.rs`/`winit.rs`'s `monitors()` from the output's real mode size
  rather than `non_exclusive_zone()`. `toggle_fullscreen` and
  `WindowManager::update_drag`'s clamp now use `full_geometry`;
  `toggle_maximize`, `SmartPlacement`'s grid/cascade, and `snap_zone`'s
  top-edge-maximize case deliberately still use `geometry` - a *new*
  window's placement and a top-edge snap are both "maximize", which
  should keep avoiding the dock the same as before. Regression tests:
  `fullscreen_covers_the_full_monitor_ignoring_a_dock_reservation`,
  `maximize_still_respects_the_dock_reservation`,
  `dragging_a_window_can_cross_into_the_dock_reserved_strip`.
- **`PlacementConfig::snap_threshold` (edge-magnetism distance for
  Windows-Snap-style drag-to-edge) was still reported as too sensitive
  at 20px, the value an earlier pass already reduced it to from 50.**
  Compounded by the `full_geometry` fix above: before that fix, a
  dragged window could never actually reach the true screen edge behind
  a dock, so it could get within the old 20px threshold of `snap_zone`'s
  comparison edge well before the cursor was anywhere near a real edge.
  Reduced to 8px - tight enough to require deliberately reaching the
  edge, not just moving generally toward it. `drag_ending_near_edge_
  snaps_to_half_screen` updated to match (drags to a point 8px inside
  the edge instead of 20px).
- **Every bordered window (the default - `border_width: 2` unless a
  rule zeroes it) rendered with square corners, while only the rare
  borderless window got the rounded titlebar treatment** - reported as
  "not all window borders are rounded," which is more precisely "almost
  none are." `render_titlebar`'s `round_corners` was deliberately `false`
  whenever bordered, specifically to avoid a titlebar rounding its own
  top corners while the square border frame around it didn't (a visible
  gap at exactly those two corners). Fixed the mismatch from the other
  side instead of disabling rounding: new `decoration::render_border_top`
  renders the border's top strip as its own small rounded-corner bitmap
  (reusing `round_top_corners`, radius `CORNER_RADIUS + border_width` so
  the cut continues outward from the titlebar's), wired into both
  backends' render loops in place of a plain `SolidColorRenderElement`
  for that one strip - the other three (bottom/left/right) don't touch a
  visible corner and stay solid fills. `redraw_decoration_buffer` now
  always passes `round_corners = true`. Regression test:
  `border_top_rounds_its_own_top_corners_to_match_the_titlebar`.
- **`srd clients`' IPC response carried no geometry** - raised by AGS
  (the panel this project's `PANEL_SUPPORT_TODO.md`/AGS's own
  `BACKLOG.md` were written against): its Overview/window-switcher needs
  window rectangles to lay out miniatures to scale, and neither
  `zwlr_foreign_toplevel_management_v1` nor `ext_foreign_toplevel_list_v1`
  carries geometry at all, by design of those protocols - this
  compositor's own IPC was the only place it could come from. Added
  `x`/`y`/`width`/`height` (the same global logical-pixel space
  everything else here uses) to `ClientInfo` in `ipc.rs`.
- **FIXED (a later pass, same session): the window z-order bug noted
  here originally as unfixed.** See the entry directly above the
  idle-notify section for the full writeup - root cause was `Space::
  map_element` silently re-stacking on every `sync_geometry` call,
  fixed with `CompState::resync_stacking_order`.

## Input-accuracy pass: cursor shape, scroll (added a later pass, same session)

- **The built-in arrow cursor's tail forked into two legs of visibly
  different widths - one tapering to a point like the rest of the
  shape, the other a constant-width block that never tapered, ending in
  an abrupt flat stop.** Looked fine glanced at in a full screenshot; the
  asymmetry only became obvious rendering the bitmap in isolation at a
  large scale, which is what actually caught it, after being told directly
  that a screenshot glance wasn't good enough - correctly. Redesigned as
  a single triangular foot mirroring the head's own taper. Regression
  test: `arrow_tail_is_a_single_tapering_shape_not_a_lopsided_fork`,
  which asserts every tail row is one contiguous opaque run with
  non-growing width - it fails against the old bitmap.
- **Scrolling forwarded to clients was missing two things `PointerAxisEvent`
  actually provides, one of them a real protocol requirement, not a nicety.**
  `AxisSource::Finger` (a touchpad) - per `AxisFrame::source`'s own doc
  comment - *requires* a `stop()` event on the frame where the axis
  genuinely has no more motion (`event.amount(axis)` returns `None`);
  nothing ever sent one, on either backend. A client has no reliable way to
  know a two-finger scroll gesture ended without it, which matters for
  kinetic/momentum scrolling and for not leaving a gesture "stuck" as far
  as the client's concerned right before the next one starts - exactly
  the class of thing that reads as "scrolling doesn't really work" rather
  than "no events arrive" (discrete wheel scrolling, needing no stop event,
  was never affected). Also added `amount_v120` (discrete wheel steps,
  `AxisFrame::v120`) alongside the existing pixel `value()` - optional per
  protocol, but some clients use it to tell a physical wheel click from
  smooth scrolling. Both fixed in `udev.rs`'s `PointerAxis` handler.
- **The nested (winit/dev) backend had no scroll handling at all --
  `InputEvent::PointerAxis` fell into a catch-all and was silently
  dropped, unconditionally, regardless of device.** Only really affects
  development/nested testing (the real session runs on `udev.rs`, already
  fixed above), but worth closing since it's exactly the kind of gap that
  would have made this specific bug class impossible to verify by testing
  nested in the first place. Added the same forwarding (stop/v120 included)
  as udev.rs's fix.
- **Window borders (`decoration::border_strips`) are drawn `border_width`
  pixels outside a window's `geometry`, but hit-testing only ever checked
  `geometry` itself** - so the visible border was a dead zone: hovering it
  showed no resize cursor and it couldn't be grabbed, even though it's what
  visually reads as the window's actual edge. `ResizeEdge::hit_test` now
  takes `border_width` and widens the containment check by that much on
  every side; `resize_edge_at`'s own margin comparisons needed no matching
  change, since they already treat anything at or outside `frame`'s edge as
  maximally "near". Regression test:
  `border_pixels_are_hoverable_not_a_dead_zone`.
- **New feature: `zwlr_output_power_management_v1`** (DPMS on/off per
  output) - `crates/wayland/src/output_power.rs`, hand-written against
  `wayland-protocols-wlr`'s raw bindings (no smithay helper exists, same
  pattern as `screencopy.rs`). udev/DRM backend only: there's no real
  display to power down when nested under a host compositor, so the global
  is genuinely not created there (`CompState::_output_power_state` is
  `Option`, `None` for `winit.rs`) rather than advertised-and-always-
  failing. `CompState::set_output_power` finds the connector's generic KMS
  "DPMS" property by name (`drm-rs` has no dedicated call for this, only
  the same `get_properties`/`set_property` every connector property goes
  through) and sets it via the raw `DRM_MODE_DPMS_ON`/`_OFF` UAPI values
  (hardcoded rather than pulling in `drm-sys` for two constants that have
  been stable since DPMS was added to the KMS UAPI). Complements, and is
  deliberately independent of, `ext_idle_notify_v1`: that protocol only
  tells a client the seat went idle, it has no way to blank a screen
  itself - a real "screen off after N minutes idle" feature needs an idle
  daemon watching the former and calling `set_mode` on this. Built and
  tested; **not yet live-verified against real DRM hardware** - this
  session's nested-instance testing setup stopped being able to nest under
  the live host partway through this pass (a `winit`-backend "Failed to
  initialize an event loop" that reproduces identically against the
  already-installed, pre-existing binary too, so it's an environment
  change - the live session switched host compositors mid-session - not
  a regression from this work), so this needs a real restart to confirm
  the DPMS property is actually found and set correctly on real hardware.
- **New feature: `zwlr_gamma_control_manager_v1`** (per-output gamma ramp --
  night light / `gammastep`/`wlsunset`) - `crates/wayland/src/
  gamma_control.rs`, same hand-written-against-raw-bindings pattern and
  same udev-only/`Option` reasoning as `output_power.rs` right above.
  `gamma_size` (sent when a client creates a control object) and the
  actual ramp length both come from the CRTC's own `gamma_length` via
  `drm-rs`'s `get_crtc`. The interesting part is `set_gamma`: the
  protocol hands the table over as a **memory-mapped fd**, not a value on
  the wire (`size` `u16`s per channel, red/green/blue back to back) --
  `memmap2` (already a transitive dependency of smithay's own `wl_shm`
  handling, just not previously used directly by this crate) maps it
  read-only, and the three channel slices are read out by hand
  (`u16::from_ne_bytes` per pair - no cross-endianness concern, client
  and compositor are always the same machine) before handing them to
  `drm-rs`'s `set_gamma`. Built and tested; **not yet live-verified
  against real hardware**, for the same nested-testing-environment reason
  as `output_power.rs` above (needs a real restart).

- **New feature: window animations** (`general.animations`/
  `general.animation_duration`) - these two config keys were validated/
  defaulted by `crates/config` but nothing ever read them, same dead-config
  bug class already found and fixed for `workspace.count`. Wiring lives in
  `crates/srdwm/src/main.rs`'s new `apply_general_settings`, read into two
  new `WindowManager` fields (`animations_enabled`, `animation_duration_ms`)
  the same way `apply_workspace_count` already reads `workspace.count` --
  and, found as a side effect of tracing this same wiring gap,
  `general.window_gap` had the identical bug (`TilingConfig::default()`
  hardcoded `gap_inner: 8, gap_outer: 16` regardless of what `init.lua`
  set; now read into `WindowManager.tiling` there too).
  Deliberately geometry-only, no fade/scale-of-content: content is
  composited through `self.space` (see `resync_stacking_order`'s doc
  comment for why per-window custom render elements were ruled out earlier
  this session as the content path), which has no per-element alpha/scale
  knob independent of the rest of the output - reintroducing that path
  just for animations would have reopened the not-fully-understood
  z-order risk that path carried before its root cause was found. What
  *is* safe to animate through `self.space` is exactly what interactive
  drag/resize already proves out on every single motion frame: a
  `Window.geometry` change applied via `map_element` and (on a size
  change) `xdg_toplevel.configure`. `crates/wayland/src/state.rs` reuses
  that exact mechanism at a fixed ~60fps cadence instead of on pointer
  motion:
  - `Window` (core) gained `anim_from: Option<Rect>`, set by
    `WindowManager::toggle_maximize`/`toggle_fullscreen` to the geometry
    just moved *from*, only when `animations_enabled`. Interactive drag/
    resize never sets it, so those still track the pointer 1:1 with no
    tween.
  - `sync_geometry` (wayland) takes (reads and clears) `anim_from`; if set
    and different from the target, it registers a `WindowAnim` (eased
    ease-out-cubic interpolation) and applies the tween's current rect
    instead of jumping straight to `geometry`.
  - `CompState::tick_animations`, called once per frame from both
    backends' `poll_events` (`render_frame`/`render_udev_frame`), advances
    every in-flight tween and re-runs `sync_geometry` for it; a finished
    tween is dropped *before* its last call so that call lands exactly on
    `Window.geometry`, not on the eased curve's last sub-pixel step.
  - New windows get a small "open-slide" tween too (`new_managed_window`
    sets `anim_from` to a rect ~24px below the resting position, same
    size) - position-only, deliberately no resize, since a freshly-
    mapping client's first paint may not have arrived yet and repeatedly
    reconfiguring it to intermediate sizes during that window risked
    looking worse than no animation, not better. Window *close* is not
    animated - the client's resources are already gone by the time
    srdwm knows about it, so there is nothing left to tween without
    rendering a static last-frame texture, a materially different (and
    separate) problem, left for a later pass.
  Covered by unit tests in both `srdwm-core` (`maximize_records_anim_from_
  when_animations_enabled`, the disabled-config counterpart, and the
  fullscreen equivalent) and `srdwm-wayland` (`WindowAnim`'s easing:
  starts at `from`, ends exactly at `to`, strictly between on every axis
  midway). **Not yet live-verified** against a real interactive maximize/
  fullscreen/open, for the same nested-testing-environment reason as the
  DPMS/gamma-control entries above (needs a real restart).
  **Note for whoever restarts next**: the shipped `~/.config/srd/init.lua`
  currently has `srd.set("general.animations", false)` (carried over from
  a prior Hyprland config, back when srdwm had no animation support at
  all) - with this wiring in place that line now actually takes effect
  and animations will stay off under the current config. Flip it (or
  remove the line, since the built-in default is `true`) to see this.

- **Fixed: dock/panel state going stale after a compositor keybinding**
  (`srd.window.maximize()`/`.fullscreen()`/`.minimize()`,
  `srd.workspace.next()`/`.prev()`/`.switch()`) - both were long-standing,
  explicitly documented gaps in `docs/PANEL_SUPPORT_TODO.md`'s P1 section:
  `crates/config` is the platform-agnostic scripting engine (shared with
  the X11/macOS backends) and only ever holds a `WindowManager` reference,
  never `CompState`, so a Lua-driven state change had no way to reach
  `foreign_toplevel::send_state`/`workspace::broadcast_active_workspace`,
  which are Wayland-protocol-specific and live one layer up. Every *other*
  trigger (pointer titlebar actions, a client's own request, the
  `SUPER+scroll` workspace gesture) already re-broadcast correctly; only
  the Lua-bound paths were silently stale. Closed with the periodic-diff
  option that entry already named as the alternative to threading a
  callback through `WindowManager`: `CompState::tick_dirty_broadcasts`,
  called once a frame from both backends' `poll_events` (same cadence as
  the animation tween's `tick_animations`) --
  `foreign_toplevel::broadcast_dirty_state` diffs `maximized`/`minimized`/
  `fullscreen` per window against what was last sent and re-broadcasts
  anything changed; `workspace::broadcast_dirty_active` diffs the current
  workspace id and only calls `broadcast_active_workspace` (real protocol
  traffic to every handle, not a cheap comparison) on an actual change.
  This catches the specific documented gap and, being a diff against live
  `WindowManager` state rather than a per-call-site hook, any future
  keybinding/API that changes the same state without its own broadcast
  call, with nothing further to remember.

- **FIXED: sustained high CPU usage, and no clean shutdown on `SIGTERM`.**
  Found chasing a real report - a peer session working on the AGS shell
  measured srdwm at 33% CPU on a 4-core machine while it was the live
  compositor, and separately found a stale `srdwm-<display>.sock` left
  behind after a session switch away from srdwm, with nothing listening on
  it. Both were real, and both are now fixed:
  - **The winit (nested/dev) backend's render loop had no pacing at all.**
    `poll_events` called `render_frame` -> full render + `swap_buffers`
    every single iteration with nothing in between that ever blocked:
    `pump_winit`'s `dispatch_new_events` polls and returns immediately
    either way, and smithay 0.7.0's winit backend hardcodes
    `vsync: false` on the EGL surface it creates (every entry point into
    `backend/winit/mod.rs` does this, not just the one this backend uses
    for its custom `WindowAttributes` - confirmed by reading the crate
    source directly), so `swap_buffers` never waits for a display refresh
    either. Measured live in an isolated nested instance, zero windows
    open: **52.5% of one core, sustained** (`ps -o %cpu`). Fixed by giving
    `idle_event_loop.dispatch` (already called every tick for `ext_idle_
    notify_v1`'s timers) a real timeout - the remaining budget until
    `TARGET_FRAME_TIME` (1/60s) has elapsed since the last frame, instead
    of always `Duration::ZERO` - rather than adding a second, separate
    sleep. Re-measured after the fix, same isolated instance, same zero
    windows: **7.1-7.4%**, roughly a 7x reduction. The udev/DRM backend
    was not affected the same way - its `poll_events` already blocks on
    `event_loop.dispatch(Some(Duration::from_millis(16)), ...)` - but see
    the border-buffer fix below for a *second*, independent way a bordered
    window could have kept it rendering (and page-flipping) every frame
    regardless of pacing.
  - **Border strips were rebuilt from scratch every render frame, in both
    backends, with a fresh `Id` every time** - the top strip via
    `decoration::render_border_top` + a brand-new `MemoryRenderBuffer`,
    the other three via `SolidColorRenderElement::new(Id::new(), ...)`.
    Confirmed by reading smithay 0.7.0's `OutputDamageTracker::
    damage_output_internal` directly: it looks up each element's previous
    state by `Id` and falls back to `.unwrap_or(true)` ("damage it") when
    no match is found. A fresh `Id` every frame means no match is *ever*
    found, so every bordered window's border was marked damaged on every
    single frame, forever - not a wider damage rect occasionally, actual
    damage unconditionally, every time, for as long as any window with
    `border_width > 0` was on screen (the default, so in practice always).
    On the udev backend this meant `has_damage` was permanently true and a
    real DRM page flip fired every ~16ms regardless of whether the screen
    had changed at all - the output could never actually go idle. Fixed
    by caching the top strip's bitmap the same way (and at the same
    trigger points - creation, a resize, a rule re-applying) the titlebar
    already was, in a new `CompState::border_top_decorations`, and by
    giving the other three strips a *persistent* `SolidColorBuffer` per
    window (`CompState::border_side_buffers`) updated in place via
    `.update()` every frame instead of rebuilt - `SolidColorBuffer::
    update` only bumps its internal commit counter when the size or
    colour actually changed, which is what lets the tracker correctly see
    "nothing changed" on a static screen. Since caching the titlebar-
    adjacent border-top bitmap meant it could go stale on a focus change
    (colour depends on focused/unfocused) where nothing was rebuilding it
    before, `set_window_activated` (the real focus chokepoint) now calls
    `redraw_decoration_buffer` on an actual activation change too - which
    incidentally fixes a second, pre-existing bug in its own right: the
    *titlebar text* colour was never being refreshed on focus change
    either, only whenever some unrelated resize happened to trigger a
    redraw regardless.
  - **No `SIGTERM`/`SIGINT` handler at all.** Default disposition is
    immediate termination with no Rust `Drop` impl ever running, so an
    external shutdown (a session manager or `systemd-logind` ending the
    session normally, not a crash or a `kill -9`) skipped `IpcServer::
    drop` (`crates/wayland/src/ipc.rs`) and left its socket file behind.
    Confirmed live testing the fix: the *old* binary genuinely ignored
    `SIGTERM` outright (`SigCgt` in `/proc/<pid>/status` showed something
    in the dependency chain - not srdwm's own code, which installed no
    handler - already catching it) and needed `SIGKILL` to die at all;
    the new binary, with `crates/srdwm/src/main.rs`'s `install_signal_
    handlers` called first thing in `main`, exits cleanly on `SIGTERM` and
    removes every socket (`srdwm-<display>.sock` and the Wayland display
    socket itself) as part of that exit. The handler itself only sets a
    process-wide `AtomicBool` (the one thing async-signal-safe to do);
    the main loop polls it once per iteration and, if set, routes through
    `running.set(false)` - the exact same path `srd.quit()` already
    uses - rather than a second, separate shutdown path. Unix only
    (`cfg(unix)`, covering Linux and macOS): `libc` added as a direct,
    `cfg(unix)`-gated dependency of `crates/srdwm` for this (already
    present transitively at the identical version, so no new dependency
    tree growth); Windows gets a no-op stub, `SIGKILL` cannot be caught by
    any process on any platform so a teardown that escalates straight to
    that is not fixable from here.

- **A priority pass driven by a deep-dive comparison against sway, Hyprland,
  river, i3, bspwm, dwm, komorebi, GlazeWM, yabai, AeroSpace and Amethyst**
  (full writeup published as a standalone artifact; the gaps found there
  drove this whole pass, roughly in priority order):
  - **FIXED: X11 keybindings silently broke under Num Lock.**
    `grab_keybindings` grabbed each combo's raw modifier mask only, not
    once per lock-modifier combination (none/Num Lock/Caps Lock/both) --
    every real X11 WM (i3, bspwm, dwm) does this because `XGrabKey`
    matches state exactly, not as a subset, so a real keypress's state
    (which always includes whichever lock modifiers happen to be toggled)
    never matched a grab that didn't. Num Lock's own modifier bit isn't
    fixed by the X11 spec (unlike Caps Lock, always `ModMask::LOCK`) --
    found via `get_modifier_mapping` + the keycode `XK_Num_Lock` (`0xff7f`)
    resolves to, at connect time. `crates/x11/src/lib.rs`.
  - **FIXED: a much larger dead-config surface than previously known.**
    Auditing every `set(...)` in `crates/config`'s defaults block against
    real read sites (the same class of bug `window_gap`/`general.
    animations` turned out to be) found ~80 more validated-but-never-read
    keys. Wired the ones with an unambiguous, already-rendered counterpart:
    `theme.decorations.title_bar.background`/`border.active_color`/
    `border.width` (a new `srdwm_core::ThemeConfig` on `WindowManager`,
    replacing hardcoded Nord-palette constants that happened to match --
    live on both the Wayland and X11 backends), `workspace.names` (a new
    `WindowManager::rename_workspace`), and `workspace.auto_back_and_forth`
    (sway's "reselecting the active workspace jumps back to the previous
    one" behavior, a new `previous_workspace` field). The rest (`debug.*`,
    `performance.*`, `platform.*.use_*`, the `layout.tiling`/`dynamic`/
    `floating` namespace, `window.remember_*`) are real features, not
    one-line wiring fixes, and stay open - listed below.
  - **FIXED: `WindowMatch` was the least capable rule matcher of any WM
    compared**, including static-config dwm - only exact `class` and
    substring `title_contains`. Added `title_regex`/`class_regex` (Rust
    `regex` crate, case-sensitive unless the pattern starts with `(?i)`)
    and `instance` (X11 `WM_CLASS`'s instance half) to `srdwm_core::
    WindowMatch`, all ANDed with the existing fields, and to `srd.rule`'s
    Lua API. Found and fixed a second, more fundamental bug in the same
    pass: **the X11 backend never read `WM_CLASS` at all**, so `app_id`
    was permanently empty on every X11 window and every class-based rule
    silently failed to match anything - the same root cause already found
    and fixed for native Wayland windows earlier this project
    (`with_toplevel_app_id`'s doc comment), just never ported to X11.
    `crates/x11/src/lib.rs`'s new `window_class`.
  - **NEW: scratchpad** (`srd.window.scratchpad()`/`.scratchpad_show()`) --
    sway's `move scratchpad`/`scratchpad show`, the single most-used
    "quick terminal" pattern in tiling WMs and the biggest single feature
    gap the comparison found. `WindowManager::scratchpad_add`/
    `scratchpad_show` reuse the existing `minimized`+`visible_windows`
    mechanism (a new `Window::scratchpad` field is purely a pool-
    membership marker) rather than inventing new visibility plumbing, so
    it rides on the same generic `sync()` diff in `crates/srdwm/src/
    main.rs` that already drives minimize/restore/geometry/decoration for
    every backend - no Wayland- or X11-specific wiring needed at all.
    Showing a hidden scratchpad window moves it onto whichever workspace
    is current, matching sway rather than pinning it to wherever it was
    hidden from.
  - **NEW: `srd`'s IPC gained event subscription** (`{"cmd":"subscribe"}`,
    `srd subscribe`) - the single highest-leverage gap the comparison
    found: every compositor compared (sway, Hyprland, i3, bspwm) has an
    event-subscribe side to its IPC; `srd`'s was poll-only, which is
    exactly why an AGS peer session had to poll raw `wlr-foreign-toplevel`
    from a separate Python helper instead of using this socket at all. A
    `subscribe` connection gets an immediate snapshot, then one more
    `{"event":"clients",...}` line every time the window list actually
    changes (diffed once a tick, skipped entirely when nothing did) --
    kept open rather than the usual one-request-one-response-close, the
    one exception to that shape. Live-verified end to end against a real
    client in a nested instance: initial empty snapshot, a push on
    Alacritty opening, a push back to empty on it closing.
    **`crates/wayland/src/ipc.rs` moved to `crates/platform/src/ipc.rs`**
    as part of this - it never actually touched anything Wayland-specific
    (pure `WindowManager` + sockets), so this also closed a separate,
    bigger gap the comparison found: **the X11 backend had no IPC server
    at all**, not even the pre-existing one-shot version. Wiring it in
    also required fixing `X11Platform::poll_events`, which used to block
    indefinitely on `wait_for_event()` - meaning the IPC socket (and
    everything else) would've gone unresponsive for as long as nothing
    happened on the X11 connection at all. Replaced with a bounded
    `poll(2)` (~16ms, matching the Wayland backends' own cadence) on the
    connection's own fd. `crates/ctl` (the `srd` CLI) updated to pick the
    right `WAYLAND_DISPLAY`-vs-`DISPLAY` socket-naming key via `srdwm_
    platform::detect()` (previously Wayland-only, hardcoded) and to
    support `srd subscribe`.

- **New feature: `zwlr_output_management_v1`** - the top item on the
  field-survey comparison's cross-platform priority list. Lets a settings
  panel enumerate every output (name, description, physical size, make/
  model, every supported mode, current position/scale/transform/enabled
  state) and request changes. `crates/wayland/src/output_management.rs`,
  hand-written against `wayland-protocols-wlr`'s raw server bindings, same
  pattern as `foreign_toplevel.rs`/`workspace.rs`. Not `Option`-gated per
  backend like DPMS/gamma-control - enumeration and applying position/
  scale/transform go through `Output::change_current_state`, which already
  works identically on both.
  - **Deliberately conservative on `apply`/`test`**: disabling/enabling a
    head (srdwm has no concept of an output that exists but isn't mapped)
    and switching resolution/refresh rate (real DRM mode-setting - finding
    the matching connector mode and reprogramming the CRTC, substantial
    hardware-dependent work, and this environment has no multi-mode
    hardware to verify it against regardless) both fail honestly (`failed`)
    rather than silently no-op. A `set_mode`/`set_custom_mode` matching the
    head's *already-current* mode - the common case, since a real panel
    echoes back existing state alongside whatever it's actually changing --
    is accepted as a no-op.
  - Moving a head keeps three separately-cached copies of output position
    in step (`Output` itself, `CompState::outputs` used for hit-testing,
    and on the udev backend `UdevHead::location` used to translate render
    geometry into head-local space) via a dedicated `apply_output_position`
    helper - each exists for its own documented reason and would
    otherwise silently drift apart after an output-management-driven move.
  - Hotplug and `apply()`-driven changes share one path: `broadcast_dirty_
    outputs`, called once a frame from `tick_dirty_broadcasts` (the same
    "diff once a tick, only do real work on a real change" shape as
    `foreign_toplevel`/`workspace`'s own broadcasts), diffs the live output
    set/state against what was last sent and creates/destroys head objects
    for anything that appeared/disappeared, re-sending current state on
    everything else.
  - **Live-verified end to end**, not just unit-tested: a `pywayland`-based
    client generated from the real protocol XML, run against a nested
    instance. Confirmed correct `head`/`mode` enumeration (name
    `srdwm-wayland`, description, make `srdwm`/model `winit`, 8 accumulated
    modes from the nested window's own resize history, current mode,
    position, transform, scale) matching the actual running instance; a
    real `create_configuration` -> `enable_head` -> `set_position(777,
    333)` -> `apply()` round trip returned `succeeded` and the head's
    position genuinely changed, confirmed by the compositor pushing a
    fresh `position` event on the same client handle; a `disable_head` ->
    `apply()` correctly returned `failed` rather than silently doing
    nothing. No errors or panics in the compositor log across all three
    rounds.

## Live daily-driving pass: border occlusion, real focus sync, EWMH init (added this pass)

srdwm became the actual live compositor for the first time this session
(not nested, not tested via Hyprland), which surfaced three real bugs no
amount of nested/unit testing had caught:

- **Border-bleed-through.** `smithay::desktop::space::render_output`
  always composites `custom_elements` (this codebase's borders/decoration)
  above *every* window's own content, with no way to interleave by real
  stacking order - confirmed live: a cascade of terminals showed every
  earlier window's border strip as solid lines cutting through the
  frontmost window. Fixed with occlusion-aware clipping:
  `srdwm_core::Rect::subtract_all` decomposes each border strip against
  every window stacked in front of it into the sub-rectangles still
  visible, rendered from a growable per-window pool of persistent
  `SolidColorBuffer`s (`crates/wayland/src/elements.rs`'s
  `visible_border_fragments`/`border_fragment_buffer`) since the fragment
  count varies frame to frame with whatever's currently stacked above.
- **`Platform::focus` was dead code.** Declared in the trait, implemented
  by both Wayland backends, but never called anywhere in `main.rs`'s event
  loop - so IPC/Lua-driven focus changes (`srd dispatch focus`,
  scratchpad) updated `WindowManager`'s own bookkeeping but never real
  Wayland keyboard focus or X11 `_NET_ACTIVE_WINDOW`. Caught while
  verifying `_NET_ACTIVE_WINDOW` for a peer session's dock. Fixed by
  wiring `sync()` (`crates/srdwm/src/main.rs`) to call `platform.focus()`
  on the resolved focus target every tick, and by having both backends'
  `Platform::focus` impls call the full `crate::input::focus_window` path
  instead of only touching core state.
- **`_NET_CLIENT_LIST` could start life stale.** Not reproduced under a
  clean session (confirmed via a same-machine peer's repro harness: both
  graceful close and SIGKILL prune the property within a second), but a
  session restarting while the X root window's properties persist across
  it was a real, cheap-to-close gap - nothing guaranteed a client only
  ever read the property after this compositor's own first
  `update_net_client_list()` call. `EwmhState::connect`
  (`crates/wayland/src/xwayland.rs`) now clears `_NET_CLIENT_LIST`/
  `_STACKING` immediately after interning the EWMH atoms, before any
  window has ever mapped - a session can no longer start life advertising
  windows it has never seen.

None of these three were found by reading the code or by the existing
test suite - all three came from actually running srdwm live, opening
real windows, and checking real EWMH/render state against what a real
client would see.

## AGS dock parity: maximize/fullscreen live-resync, IME, `floating` in IPC (added this pass)

Raised by the same AGS peer session porting its dock off Hyprland-only
`hyprctl` calls, this time chasing a real user-reported symptom rather
than a protocol trace:

- **A maximized window didn't grow when a dock released its reserved
  space.** `WindowManager::set_monitors` (`crates/core/src/manager.rs`,
  called whenever a layer-shell client's exclusive zone changes) already
  recomputed `Monitor::geometry`/`full_geometry` correctly, but never
  touched the `geometry` of a window that was *already* maximized or
  fullscreen - it kept whatever rect it was sized to at the moment it was
  toggled on. An auto-hide dock dropping its zone to 0 while a window was
  maximized therefore left that window stuck at its old, dock-shrunk size
  until manually un-maximized and re-maximized. Fixed: `set_monitors` now
  re-syncs any maximized/fullscreen window's `geometry` to its monitor's
  current `geometry`/`full_geometry` (whichever mode it's in) in the same
  pass. Confirmed `maximized`/`fullscreen` map deliberately onto
  Hyprland's monocle/true-fullscreen modes respectively (`toggle_maximize`
  targets the exclusive-zone-shrunk usable rect, `toggle_fullscreen` the
  true full rect) - not incidental, an existing doc comment on
  `toggle_fullscreen` already said as much. Three new regression tests.
- **`zwp_text_input_manager_v3` + `zwp_input_method_v2`** - see
  `docs/PANEL_SUPPORT_TODO.md`'s P2 entry for the full writeup.
- **`floating` added to `srd clients`.** The dock's overlap check needs to
  tell a layout-placed tiled window sitting flush against its reserved
  edge (expected) from a user-dragged floating window actually overlapping
  it (a real overlap) - geometry alone can't distinguish the two.
  `Window.floating` was already a first-class field; this was a direct
  passthrough in `crates/platform/src/ipc.rs`.

## Live daily-driving pass, round 2: real stacking-order bug, titlebar occlusion gap (added this pass)

Found live, on the real restarted (non-nested) session, by actually opening
overlapping windows and screenshotting rather than reading code:

- **Focus never visibly "stuck" to a window - root cause.** `main.rs`'s
  `sync()` drove its per-window `apply_geometry`/`redraw_decoration` loop
  off `WindowManager::visible_windows()`, which iterates a `HashMap` in
  arbitrary order. Both of those `Platform` calls end up calling
  `CompState::sync_geometry`, which - as a documented side effect of
  smithay's own `Space::map_element` - re-raises whichever window it's
  called for to the top of `Space`'s real render order. Since `sync()` runs
  on essentially any dirty event (a keystroke, a resize frame, a workspace
  poll - many times a second in normal use), every single tick re-shuffled
  every visible window's on-screen stacking order to whatever the `HashMap`
  happened to yield that pass, completely unrelated to which window was
  actually focused. Confirmed live: a freshly-launched, logically-focused
  `gnome-calculator` window (`srd clients` reporting `focused: true`)
  rendered entirely *behind* an older, unfocused terminal on every
  screenshot. Fixed: `sync()` now iterates `visible_windows_front_to_back()`
  (real stacking order) reversed, bottom-to-top, so each pass's cascade of
  re-raises ends deterministically with the true topmost window raised
  last - restoring the same order it started with instead of scrambling
  it every tick. This was very likely the dominant contributor to several
  distinct-sounding live reports this session (focus not sticking after
  interacting with a panel, borders/decoration looking desynced from their
  window) rather than three separate bugs.
- **A background window's titlebar bled through a foreground window.** The
  border-occlusion fix earlier this pass (see the section above) only ever
  touched the border *strips* - the titlebar bitmap itself (`self.
  decorations`, holding the title text and min/max/close buttons) was still
  pushed into `custom_elements` completely unconditionally, with a comment
  claiming "titlebar/content ... occlude correctly on their own paths",
  which was true for content but never actually true for the titlebar.
  Reported live as "still see the behind window's bar." Fixed in both
  `udev.rs` and `winit.rs`: the titlebar push now gets the same all-or-
  nothing occlusion test the top border strip already used (skip the draw
  entirely once the titlebar's rect is fully covered by windows stacked in
  front of it).
- **Cursor appearance investigated, not a bug.** The built-in arrow (white
  fill, black outline) renders correctly - confirmed by cropping the exact
  same bitmap over a light background (crisp, fully outlined) versus a
  dark terminal (outline blends into the background, leaving only the
  white fill visible, which reads as "looks weird" without being
  corrupted). A real but purely cosmetic contrast issue, not a rendering
  bug; not touched this pass since it needs a considered redesign (a halo/
  double-outline, the same technique real cursor themes use) rather than a
  one-line fix, and the existing bitmap has its own passing shape-
  regression tests that a hasty change could easily break.

## Titlebar window menu + global menu support (added this pass)

Two features, both requested directly: make the titlebar/border
interactions "highly functional" (the user's own words, after several
rounds of live bug reports), and start on optional global-menu support now
that srdwm is a real, independent compositor rather than something
adapting to an existing one.

- **Right-click titlebar window menu.** Right-click on a titlebar
  previously did nothing at all - the only right-button behaviour
  anywhere was the SUPER+right-drag resize gesture, and only with the
  modifier held. `context_menu.rs` (state/hit-testing, pure and unit-
  tested) + `decoration::render_context_menu` (the pixels, also unit-
  tested) + wiring in `input.rs`/`state.rs`/`udev.rs`/`winit.rs`. Four
  actions - Minimize, Maximize/Restore (label reflects current state),
  Always on Top (checkmark-prefixed once pinned), Close - no submenus, no
  live hover highlight (would need the render buffer rebuilt on every
  motion event over the menu; not worth the per-frame cost for a first
  pass). A press anywhere while the menu is open resolves it (selects a
  row) or dismisses it; neither case falls through to whatever a normal
  click there would have done. Cleaned up if the window it belongs to
  closes out from under it (crash, kill, or its own Close action racing
  ahead), on both the native and XWayland removal paths.
- **Middle-click titlebar lowers the window.** The convention several X11
  WMs (twm, fvwm, IceWM) have always had; srdwm never did. New
  `WindowManager::lower_window` (opposite of the existing `raise_window`,
  same pinned-window protection via `restack_pinned`).
- **Global menu: address over the protocol, content over D-Bus.** Per
  design input from an AGS peer session already building the consuming
  shell: a compositor should carry the menu's *address* (D-Bus bus name +
  object paths) and leave the content on D-Bus, where GTK4 already
  exports it as a real `GMenuModel` (`org.gtk.Menus`/`org.gtk.Actions`)
  that GTK consumes natively - submenus, toggles, accelerators, icons,
  all for free, with no model-walking/rendering code of this compositor's
  own to have gaps in. New `srdwm_core::GlobalMenu { bus_name, menu_path,
  app_path, window_path }`, exposed as `global_menu` on every `srd
  clients` entry (`null` when a window hasn't exported one).
  - **XWayland**: `EwmhState::read_global_menu` (`xwayland.rs`) reads the
    four `_GTK_*`/`_UNITY_OBJECT_PATH` atoms straight off the X11 window,
    refreshed on every real focus change (the properties are usually set
    once, shortly after the client's own D-Bus registration completes,
    which can race its window's initial map - reading only at map time
    would miss a client that finished registering a moment later; a focus
    change is a natural, already-existing hook, and the menu only needs
    to be current for whichever window is actually focused anyway).
  - **Wayland-native**: GTK's own private `gtk_shell1`/`gtk_surface1`
    protocol, generated at compile time from a vendored copy of GTK's own
    XML (`crates/wayland/protocols/gtk-shell.xml`) via `wayland-scanner`
    - no published `wayland-protocols-*` crate exists for this one, so
    it's generated the same way `wayland-protocols-wlr` generates its own
    bindings internally, server-side only. `gtk_shell.rs` sends
    `capabilities` (both `global_app_menu`/`global_menu_bar` bits) right
    after a client binds the global - GTK only bothers calling
    `set_dbus_properties` later if it saw that first - then stores
    whatever a `gtk_surface1.set_dbus_properties` request carries onto
    the matching `WindowId`'s `global_menu`, and clears it again if the
    object is destroyed while the window itself stays open.
  - **Known, expected limitation, not a bug**: even with a perfect
    address, most non-GTK apps show nothing - the app has to actually
    export a `GMenuModel` over D-Bus itself (`appmenu-gtk-module` for
    real GtkMenuShell-based GTK apps; Qt and anything self-drawing its
    own menus, LibreOffice's VCL toolkit included, generally won't).
    Flagged by the same AGS session ahead of building the consumer, from
    their own live testing against LibreOffice.
  - Not yet live-verified against a real client on either path - built
    and compiles cleanly (workspace-wide, including the new
    `wayland-scanner`/`wayland-backend` build-time codegen dependency),
    pending the same restart as everything else this pass.

## Live daily-driving pass, round 3: real full_geometry bug, titlebar fragments, `srd monitors` (added this pass)

All found live, on the real session, chasing specific user-reported
symptoms rather than by reading code first:

- **Fullscreen never actually covered a bar/dock - confirmed by
  triggering it and reading the resulting geometry back over IPC.**
  Landed at exactly the reserved-zone-shrunk rect on a 1920x1080 output,
  not the true output. Root cause: all three real backends'
  `monitors()` (`udev.rs`, `winit.rs`, `crates/x11`) construct `Monitor`
  via `Monitor::new(id, name, rect)` where `rect` is *already*
  zone-shrunk, and `Monitor::new` defaults `full_geometry` to whatever
  `geometry` was constructed with - so `full_geometry` was silently
  identical to `geometry` for every real monitor either Wayland backend
  ever reported. `toggle_fullscreen`'s entire "ignore the reserved zone"
  design was a no-op in practice, and the existing unit tests (which
  construct `Monitor` by hand with the two fields genuinely different)
  couldn't have caught it - they exercise the invariant, never the
  construction path that was collapsing it. Fixed in `udev.rs`/
  `winit.rs` by setting `full_geometry` separately from the true output
  size; `crates/x11` doesn't have the same zone-shrinking step at all, so
  it wasn't affected. Live-reverified twice more afterward: once via a
  real `waybar` in a nested instance with `srd monitors` (see below)
  showing `geometry`/`full_geometry` correctly split by the bar's exact
  36px, and independently by the AGS peer session against their own real
  dock.
- **Titlebar bleed-through, the sequel.** The occlusion fix from the
  previous pass (see above) was all-or-nothing: skip the titlebar draw
  only once *fully* covered. A titlebar only *partially* covered - the
  common case for overlapping/cascaded windows - still drew in full,
  bleeding through the covered part, reported live as the same "still
  see the behind window's bar" complaint the earlier fix was supposed to
  close. Now fragment-clipped the same way the three solid border strips
  already were, using `MemoryRenderBufferRenderElement::from_buffer`'s
  `src` parameter to crop the bitmap itself per visible fragment instead
  of drawing the whole thing or nothing.
- **`Bottom`/`Background` layer surfaces were unclickable, full stop.**
  Found while investigating a live "clicking the dock does nothing"
  report (the dock itself turned out to be `Layer::Top`, already
  checked, so not the cause of that specific report - but a real,
  separate gap all the same): `layer_surface_under` only ever checked
  `Layer::Overlay`/`Layer::Top`; nothing in the click or motion path ever
  checked `Bottom`/`Background` at all, so a surface at either layer
  (a desktop-icons layer, a wallpaper daemon wanting clicks) could never
  receive pointer input regardless of what was or wasn't covering it.
  Fixed by checking `Bottom`/`Background` as a final fallback, after
  windows/decoration/`Overlay`/`Top` all come up empty - correct
  ordering, since `Bottom`/`Background` are meant to sit *behind* normal
  windows while `Overlay`/`Top` sit in front of everything.
- **`srd monitors`.** Requested directly by the AGS peer session after
  two separate live-debugging rounds (the maximize-past-dock and
  fullscreen-past-dock questions above) each took several turns of
  indirect reasoning that reading this back directly would have settled
  immediately. Returns `geometry` (usable, what maximize/tiling target)
  and `full_geometry` (true output, what fullscreen targets) per
  monitor. Live-verified with a real `waybar` in a nested instance.
- **The `srd` CLI binary itself hadn't been reinstalled all session.**
  Every fix this whole multi-pass session reinstalled `srdwm` (the
  compositor); nothing had reinstalled the separate `srd` binary (the
  control CLI) even once, so `srd monitors` above genuinely didn't exist
  on the invoking side until this was caught and fixed. `srd` isn't a
  long-running process, so this needed no restart to pick up - unlike
  `srdwm` itself, every `srd` invocation from this point on already sees
  the current binary.

## Not implemented anywhere yet

All three protocols originally identified as blocking srdwm-wayland from
being a real daily-driver session (bars/launchers/notifications/lock UIs,
clipboard, screen locking) are now implemented and verified - see the
Wayland backend section above. What is left:

- **Multi-GPU** - only the primary GPU's connectors are driven. A GPU
  appearing or disappearing is logged and ignored.
- A native GUI settings app (the legacy project's `GUI_SETTINGS.md` was
  pure design doc even in C++; not revisited here).
