# Desktop-shell / panel support - TODO

Findings from trying to run a GTK4 + gtk4-layer-shell panel (AGS/Astal) on srdwm.
Everything below was measured against the **running binary built from this
worktree at 2026-08-12 17:32**, not read out of the source - re-verified after
that rebuild, so it is current, and reproduced both on the real session and on a
nested `srdwm --wayland`.

The C++ tree on `main` is stale and misleading for this (its
`wayland_platform.cc` has `layer_shell_` commented out while the live binary
advertises `zwlr_layer_shell_v1` v4) - ignore it.

## What srdwm advertises today (16 globals)

```
ext_session_lock_manager_v1 (v1)          wl_compositor (v5)
wl_data_device_manager (v3)               wl_output (v4)
wl_seat (v9)                              wl_shm (v2)
wl_subcompositor (v1)                     wp_fractional_scale_manager_v1 (v1)
wp_viewporter (v1)                        xdg_wm_base (v6)
zwlr_data_control_manager_v1 (v2)         zwlr_layer_shell_v1 (v4)
zwlr_screencopy_manager_v1 (v2)           zwp_primary_selection_device_manager_v1 (v1)
zxdg_decoration_manager_v1 (v1)           zxdg_output_manager_v1 (v3)
```

Layer-shell v4, session lock, data-control and xdg-output is a good base.

---

## P0 - the panel cannot stay connected

### 1. FIXED - smithay posted `invalid_size` on a commit after role destroy

**Root cause (smithay 0.7.0, upstream) and the local workaround, both verified
end to end on 2026-08-12 19:28. A GTK4 panel now runs on srdwm.**

#### What was wrong

`src/wayland/shell/wlr_layer/handlers.rs:125` registers a **pre-commit hook** on
the `wl_surface` at `get_layer_surface` time. That hook runs on every subsequent
commit of that surface, forever, and validates the layer-shell cached state:

```rust
if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
    guard.surface.post_error(Error::InvalidSize,
        "width 0 requested without setting left and right anchors");
}
```

When the client destroys the layer surface, smithay `.reset()`s
`LayerSurfaceCachedState` to `Default` (size `0x0`, no anchors) rather than
removing it, and does not unregister the hook. So the next commit on that
`wl_surface` validates the reset state, fails, posts a protocol error, and the
connection dies.

gtk4-layer-shell does create -> commit -> destroy -> commit as part of normal
setup, which is why every GTK4 panel died and waybar (one surface, never
destroyed) did not. Nothing to do with `wl_output` being named or nil.

#### Why it could not be fixed behind the hook

- Smithay's hook is registered at `get_layer_surface`, *before* our
  `new_layer_surface` callback runs.
- `destroyed()` calls our `layer_destroyed` and then resets the cached state
  unconditionally, so anything set there is overwritten.
- Smithay discards the `HookId` from its own `add_pre_commit_hook`, so its hook
  cannot be removed.

#### The fix that works: get in front of it

Three facts from the 0.7.0 source make it possible:

1. `add_pre_commit_hook` / `remove_pre_commit_hook` are public API.
2. Hooks are a `Vec`, **pushed and iterated in registration order**
   (`tree.rs`: `pre_commit_hooks.push(hook)`, `for hook in hooks`).
3. `CompositorHandler::new_surface` fires from `wl_compositor::CreateSurface`,
   strictly before any `get_layer_surface` on that surface.

So a hook registered in `new_surface` sits at index 0 and runs *before*
smithay's. Implemented in `crates/wayland/src/{protocols,state}.rs`:

- `CompState` gains `dead_layer_surfaces: HashSet<WlSurface>`.
- `layer_destroyed` inserts the surface.
- `new_surface` registers a per-surface pre-commit hook that is a no-op unless
  the surface is in that set, in which case it nudges `pending.size.w`/`.h` from
  0 to 1 on whichever axis is unanchored - making the state self-consistent
  before smithay validates it.

**Caveat, keep the comment in the code:** this depends on hook execution being
registration order. That holds in 0.7.0 and it is a plain `Vec`, but it is not a
documented API guarantee - a future smithay could reorder and this would silently
stop working. Pin the version, and remove this once upstream fixes the root bug.
0.7.0 (2025-06-24) is the newest release on crates.io, so there is nothing to
bump to today.

#### Verification

Against the exact binary (sha256 `7faf27f1...`):

```
minimal repro (lsmin.py, raw protocol, no GTK):
  3 layer surfaces, no destroy                   SURVIVED
  create, destroy, create                        SURVIVED
  create, destroy, RECOMMIT surface, create      SURVIVED   <- was CONNECTION KILLED
invalid_size errors posted:                      0
```

AGS end to end on a nested srdwm: **runs, 0 errors, all 5 gtk4-layer-shell
surfaces created (3 nil / 2 named output), 13 configures sent, 5 buffers
attached, still alive after 60s.** Screenshot confirms the bar and dock actually
render - clock, resources graph, tray, power button, dock icons. The workspace
pill is correctly absent, since srdwm exposes no workspace protocol (see P1).

### 2. Send a protocol error instead of closing the socket silently

When the connection does go, there is **no `wl_display.error` event at all** - the
socket simply closes. From the client that is indistinguishable from a compositor
crash. `wl_resource_post_error` (or smithay's equivalent) on any fault would have
turned this whole investigation into one log line.

### 3. FIXED - No dmabuf - every client was forced through shm

Neither `zwp_linux_dmabuf_v1` nor `wl_drm` was advertised, so no client could hand
over a GPU buffer. GTK4 defaults to its Vulkan renderer and died:

```
vkGetPhysicalDeviceSurfaceFormatsKHR(): A surface is no longer available. (VK_ERROR_SURFACE_LOST_KHR)
libEGL warning: failed to get driver name for fd -1
libEGL warning: MESA-LOADER: failed to retrieve device information
```

`GSK_RENDERER=cairo` was required for any GTK4 client to get as far as the
layer-shell hang above. On a 4-core / 3.8GB laptop, pushing every client
through software rendering is a large permanent cost.

**Fixed**: `zwp_linux_dmabuf_v1` v3 (`DmabufState::create_global`, `crates/wayland/src/protocols.rs`'s
`DmabufHandler` impl, `delegate_dmabuf!`). The non-obvious part is that this works
at all on the udev backend despite it rendering entirely in software
(`PixmanRenderer`, no GPU pipeline - see `udev.rs`'s module doc comment):
smithay's `PixmanRenderer` already implements `ImportDma`, importing a dmabuf by
mmap'ing it and reading the pixels directly, which only works for the
`Linear` modifier - exactly the one `PixmanRenderer::dmabuf_formats()`
advertises. A client (GTK4 included) still allocates through its own
EGL/gbm path against the real DRM render node - untouched by this
compositor either way - and hands over a Linear-modifier dmabuf, which
pixman reads straight off with no GPU involvement on the compositor side.
`dmabuf_imported` eagerly validates by actually importing on the udev
backend (`self.udev`'s `PixmanRenderer` is reachable from `CompState`
directly); the winit (nested/dev) backend's `GlesRenderer` lives outside
`CompState` (a sibling field on `WaylandPlatform`) and isn't reachable the
same way, so buffers are accepted there without eager validation and
imported lazily the first time they're actually rendered, same as every
other buffer type. Went with v3 (`create_global`, a plain format list)
over v4's `..._with_default_feedback` (which needs a `main_device` `dev_t`
to steer multi-GPU clients toward the right render node) - a real gap
worth closing later, not required for a single-GPU client to allocate and
hand over a Linear-modifier buffer, which is all this backend can use
regardless. Live-verified in an isolated nested instance (`SRDWM_NESTED=1`,
so the session's real autostart never fired): `zwp_linux_dmabuf_v1` version
3 shows up in a real client's (`wezterm`) registry bind over
`WAYLAND_DEBUG=1`, and the client connects and maps a window normally with
no protocol error or crash. Actually handing over a GPU-allocated dmabuf
buffer end to end (GTK4 with `GSK_RENDERER` unset, in particular) is not
yet re-verified live - this development sandbox has no working `/dev/dri`
render node to allocate one against (`eglGetPlatformDisplay: EGL_BAD_ALLOC`
even on the winit backend's own `GlesRenderer`), so that retest needs the
user's real machine.

---

## P1 - protocols a bar needs, none of them workaroundable client-side

The data never leaves the compositor, so a panel simply cannot show these.
Current `delegate_*` set in `crates/wayland/src/`: compositor, data_control,
data_device, fractional_scale, layer_shell, output, primary_selection, seat,
session_lock, shm, viewporter, xdg_decoration, xdg_shell, xwayland_shell.

- [x] DONE - **`zwlr_foreign_toplevel_management_v1`**'s `app_id`/`title` were
      empty on the wire (state/activated worked) - live-verified by a peer
      session's raw protocol capture against a running dock. Root cause:
      `new_toplevel` fires at `xdg_surface.get_toplevel()` (role
      assignment), which happens before a client's `set_title`/`set_app_id`/
      first commit for essentially every real client, not racily but every
      single time - so `new_managed_window` read `XdgToplevelSurfaceData`
      before either was ever set. Fixed by re-reading both on every
      `commit()` (`sync_toplevel_metadata` in `state.rs`) and re-announcing
      to foreign-toplevel listeners when either changed.
- [x] DONE (not yet live-verified against AGS) - **`zwlr_foreign_toplevel_management_v1`**
      - enumerate windows with title/app_id/state, and activate/close/
      maximize/minimize them. Went with the wlr protocol over the newer
      `ext_foreign_toplevel_list_v1` + a separate management protocol:
      smithay 0.7 only has a built-in helper for the `ext_` list-only half,
      but the wlr one covers enumeration *and* interactivity in a single
      interface, which is what was actually asked for (see the downstream
      report this was scoped against: "click-to-focus in the dock, the
      focused-app highlight, alt-tab actually switching, middle-click-to-
      close" - none of that is possible from list-only enumeration alone).
      Hand-written against `wayland-protocols-wlr`'s raw server bindings,
      same pattern as `screencopy.rs` (`crates/wayland/src/foreign_toplevel.rs`).
      Override-redirect windows are deliberately never announced, matching
      `_NET_CLIENT_LIST`/ICCCM. `Activated` state is kept live on every
      focus change (the one thing that actually needed to be, per the
      report above).
      DONE - maximize/fullscreen/minimize changes that originate from the
      pointer (titlebar button, double-click) or from a client's own
      request (`xdg_toplevel`'s `set_maximized`/`unset_maximized`/
      `set_fullscreen`/`unset_fullscreen`/`set_minimized`, and the matching
      X11/XWayland WM hints) now all call `foreign_toplevel::send_state`,
      so a dock/panel sees the change regardless of which of those three
      triggered it.
      FIXED - the fourth trigger, a *compositor keybinding*
      (`srd.window.maximize()`/`.fullscreen()`/`.minimize()` from the Lua
      config), did not re-broadcast: `crates/config` only ever holds a
      `WindowManager` reference, never `CompState`, so it cannot reach
      `foreign_toplevel::send_state` directly. Went with the periodic-diff
      option this entry named as the alternative to a callback: `CompState::
      tick_dirty_broadcasts` (called once a frame from both backends'
      `poll_events`, same cadence as the animation tween's `tick_animations`)
      diffs `maximized`/`minimized`/`fullscreen` per window against what was
      last sent and re-broadcasts anything that changed, regardless of which
      call site changed it - catches this gap and any future one the same
      way, rather than one more call site to remember. See
      `foreign_toplevel::broadcast_dirty_state`'s doc comment.
- [x] DONE - **`ext_workspace_manager_v1`** - enumerate workspaces,
      which is active, and switch. This list entry was stale: the whole
      protocol (`crates/wayland/src/workspace.rs`) was already implemented
      and wired into both backends' globals - one flat `ext_workspace_
      group_handle_v1` shared by every output (srdwm has no per-output
      workspace concept), `activate` requests routed to `switch_workspace`,
      `create_workspace`/`remove`/`assign`/`deactivate` deliberately not
      advertised as capabilities (srdwm's workspace set is fixed at
      startup). What *was* a real gap, found auditing every `switch_
      workspace` call site the same way `foreign_toplevel`'s broadcast gap
      was found: the `SUPER+scroll` workspace-cycle gesture in `input.rs`
      called `switch_workspace` directly without calling `workspace::
      broadcast_active_workspace` afterward, so a dock's workspace pill
      only ever updated when a switch came through this protocol's own
      `activate` request, going stale the moment the scroll gesture (or the
      `srd.workspace.next()`/`.prev()`/`.switch()` Lua API, which has the
      identical gap for the identical `crates/config`-has-no-`CompState`
      reason `foreign_toplevel`'s keybinding gap does) changed it instead.
      Fixed the `input.rs` scroll-gesture call site directly; the Lua-API
      call sites are covered by the same `tick_dirty_broadcasts` fix as the
      maximize/fullscreen/minimize keybinding gap above - `workspace::
      broadcast_dirty_active` diffs the current workspace id once a frame
      and only re-broadcasts on an actual change (unlike the foreign-
      toplevel diff, calling `broadcast_active_workspace` unconditionally
      would be real protocol traffic every frame, not a cheap comparison,
      so this gates it first).
- [x] FIXED (not yet re-verified live) - **`zwlr_screencopy_manager_v1`: `grim`
      hangs on the DRM/udev session but WORKS on the nested winit backend.**
      Root cause found by reading `render_udev_frame` in
      `crates/wayland/src/udev.rs`: it drains *all* of
      `CompState::screencopy_pending` once per call and services the whole
      batch against whichever head is first in that pass's `ready` list (heads
      with a page-flip in flight are filtered out of `ready`). If `ready` is
      empty on a given call - every head mid-flip - the drained captures were
      dropped at function exit with neither `ready` nor `failed` ever sent,
      which is indistinguishable from a hang to the client. Two related bugs
      fixed together: (1) `PendingCapture`/`FrameData` now carry the `Output`
      the capture actually targets, and captures are partitioned per-head
      inside the loop instead of all going to the first head (also fixes
      multi-monitor captures reading the wrong screen's framebuffer); (2)
      anything left unserviced after the loop - wrong head not ready this
      pass, or the two earlier-return paths (`udev` not yet initialized,
      session inactive after a VT switch) - is put back into
      `screencopy_pending` instead of dropped, so the next call gets another
      chance rather than the client waiting forever. Needs a `grim` retest
      against the real DRM session to confirm.
- [x] DONE - **`xdg_activation_v1`** - so a launcher can raise the app it just
      spawned instead of it opening unfocused behind everything.
      `XdgActivationState` + `XdgActivationHandler` (`crates/wayland/src/protocols.rs`),
      `delegate_xdg_activation!`. No token bookkeeping of our own - the
      default `token_created` already accepts every token, fine for a
      single-user session with no cross-client trust boundary to enforce.
      `request_activation` maps the activating surface to a `WindowId` via
      `surface_to_id` and reuses the same `focus_window` path a dock's
      foreign-toplevel "activate" request already goes through. A no-op,
      not an error, if the surface isn't tracked yet (activation racing
      ahead of the window's own mapping) - the protocol doesn't require
      honoring every activation. Live-verified in an isolated nested
      instance: `xdg_activation_v1` version 1 advertises correctly, a real
      client connects and maps normally, no crash.

---

## P2 - full-desktop quality of life

- [x] DONE (built, tested, installed; not yet live-verified against a real
      lock daemon) - **`ext_idle_notify_v1` + `zwp_idle_inhibit_manager_v1`**
      - idle lock, and inhibiting it during video playback. Both use
      smithay's own built-in modules; see `docs/IMPLEMENTATION_STATUS.md`
      for the full writeup, including the one real gap (not workspace-
      visibility-aware) and the winit-backend calloop-loop wrinkle this
      needed to work around.
- [x] DONE (built, tested, live-verified end to end in a nested instance
      against a real protocol client) - **`zwlr_output_management_v1`** -
      display arrangement/scale from a settings panel. `crates/wayland/src/
      output_management.rs`. Not `Option`-gated like DPMS/gamma-control --
      enumeration and applying position/scale/transform work identically
      on both backends via `Output::change_current_state`. Deliberately
      does not support disabling/enabling a head or switching resolution/
      refresh rate (real DRM mode-setting, hardware-dependent work this
      pass didn't attempt) - both fail honestly (`failed` event) rather
      than silently no-op. Live-verified with a `pywayland`-based test
      client generated from the real protocol XML: binds the manager,
      receives correct `head`/`mode` events (name, description, make,
      model, 8 accumulated modes, current mode, position, transform,
      scale) matching the running nested instance; `create_configuration`
      -> `enable_head` -> `set_position(777, 333)` -> `apply()` returns
      `succeeded` and the head's position genuinely changes (confirmed via
      a pushed `position` event on the same handle); `disable_head` ->
      `apply()` correctly returns `failed` rather than silently doing
      nothing.
- [x] DONE (built, tested; not yet live-verified against real hardware --
      see `docs/IMPLEMENTATION_STATUS.md`) - **`zwlr_output_power_management_v1`**
      - DPMS on/off per output, for an idle daemon or settings panel.
      `crates/wayland/src/output_power.rs`, udev/DRM backend only.
- [x] DONE (built, tested; not yet live-verified against real hardware --
      see `docs/IMPLEMENTATION_STATUS.md`) - **`zwlr_gamma_control_manager_v1`**
      - night light / `gammastep`/`wlsunset`. `crates/wayland/src/
      gamma_control.rs`, udev/DRM backend only.
- [ ] `zwlr_virtual_pointer_manager_v1` + `zwp_virtual_keyboard_manager_v1` -
      required by `ydotool` and by any automated UI testing.
- [x] DONE - **`wp_cursor_shape_manager_v1`** - `protocols.rs`'s
      `TabletSeatHandler` doc comment and `delegate_cursor_shape!(CompState)`
      confirm this was already wired up (routes into `SeatHandler::
      cursor_image`, same path a client-drawn cursor surface uses); this
      list entry had simply never been updated. What actually needed work
      was `cursor.rs`'s *art*, not the protocol - see `IMPLEMENTATION_
      STATUS.md`'s entry on the new crosshair/move/pointer-hand bitmaps.
- [ ] `zwp_pointer_constraints_v1` + `zwp_relative_pointer_manager_v1` - games.
- [ ] `wp_presentation` - accurate frame timing.
- [x] DONE (built, tested; not yet live-verified against a real IME) -
      **`zwp_text_input_manager_v3` + `zwp_input_method_v2`** - IME (fcitx5,
      ibus: CJK/dead-key composition, emoji picker). Both are smithay
      built-ins (`TextInputManagerState`/`InputMethodManagerState`),
      registered in `state.rs`/`udev.rs`/`winit.rs`; `InputMethodHandler`
      impl in `protocols.rs`. Focus tracking needed no wiring at all --
      `CompState::KeyboardFocus` is a plain `WlSurface`, and smithay's own
      blanket `impl KeyboardTarget for WlSurface` already drives `seat.
      text_input()`/`seat.input_method()` from inside `enter`/`leave`,
      which `set_keyboard_focus`'s existing `keyboard.set_focus(...)` call
      already triggers. The IME's own composition/candidate popup is
      tracked as a `PopupKind::InputMethod` in the same `PopupManager`
      that already owns every `xdg_popup` - `elements.rs`'s
      `popup_render_elements` renders both kinds identically, so the popup
      actually draws rather than existing invisibly.
- [ ] `wp_single_pixel_buffer_v1`, `xdg_foreign_v2`.

---

## P3 - integration, not protocol

- [x] DONE - **An IPC/CLI.** `srd` (`crates/ctl/`) talks to a
      per-display Unix socket (`$XDG_RUNTIME_DIR/srdwm-<display>.sock`,
      `crates/platform/src/ipc.rs`'s `IpcServer` - moved out of
      `crates/wayland` since it never touched anything Wayland-specific,
      so both backends can share one implementation - polled once per
      tick in all three backends now) with a JSON protocol: `clients`
      (id/app_id/title/workspace/focused/minimized/visible/scratchpad/
      floating/x/y/width/height per window - `floating` added for an AGS
      dock's auto-hide overlap check, which needs to tell a layout-placed
      tiled window flush against its reserved edge from a user-dragged
      floating one actually overlapping it; geometry alone can't
      distinguish the two), `dispatch <toggle_visibility|focus|
      close> <id>`, and now `subscribe` - a full event-stream like `niri
      msg -j`'s: the one connection stays open and gets pushed a fresh
      `clients` snapshot every time the window list actually changes,
      instead of a client having to poll and diff it themselves. This was
      the single highest-leverage gap found comparing srdwm's IPC against
      sway/Hyprland/i3/bspwm's own (all of which already have this) --
      an AGS peer session had hit exactly this wall building a Python
      helper to poll raw `wlr-foreign-toplevel` instead of using this
      socket at all. Live-verified end to end against a real client
      (Alacritty) in a nested instance. **X11 previously had no IPC server
      of any kind** - the crate move fixed that too, along with
      `X11Platform::poll_events` used to block indefinitely on
      `wait_for_event()`, which would have left the socket unresponsive
      between X11 events; now a bounded `poll(2)` (~16ms).
- [x] DONE (not yet live - built and installed, pending restart) - **Set
      `XDG_CURRENT_DESKTOP=srdwm`** for the session. Set in `udev.rs`
      (deliberately not `winit.rs`, since that's the nested/dev backend) --
      no longer inherited from whatever compositor the session was
      previously started under.
- [x] DONE (already correct, no change needed) - **Remove runtime sockets
      on exit.** `IpcServer`'s `Drop` impl already removes its socket path
      unconditionally; verified while auditing `foreign_toplevel`/protocol
      cleanup paths this pass.
- [x] FIXED - `jq: parse error: Invalid numeric literal at line 1, column 28`
      repeated continuously in `~/.local/state/wm-session-latest.log` --
      `hyprctl`'s own "not running" message goes to stdout, not stderr, so
      every script that still called it unconditionally after the
      Hyprland-to-srdwm migration fed that sentence straight into `jq` as
      if it were JSON. Fixed with an `[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]`
      guard added to `~/.scripts/sys/night-light`, `reading-mode`, and
      `~/.scripts/utils/toggle-blur`, `move_terminal` - confirmed present
      in all four as of 2026-08-19.

---

## How to reproduce

```sh
# nested, so it doesn't disturb a running session
WAYLAND_DISPLAY=wayland-0 srdwm --wayland &

# panel, forced onto the software renderer (see P0.3)
WAYLAND_DISPLAY=wayland-1 GSK_RENDERER=cairo WAYLAND_DEBUG=1 ags run 2>&1 | tee trace.log

# the whole bug in one number - this prints 0, and must not:
grep -E '^\[[0-9]' trace.log | grep -v ' -> ' | grep -c layer_surface
```

For comparison, the same panel on niri: runs clean with zero errors and three
mapped `gtk4-layer-shell` surfaces.
