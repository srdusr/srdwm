# Feature gap survey: srdwm vs. comparable Wayland compositors

Requested directly: compare srdwm against similar projects and note what
else is worth having. Compared against niri (cloned at
`~/reference-wms/niri`, smithay-based like `crates/wayland`, the closest
architectural sibling per `docs/PRIOR_ART.md`) plus general knowledge of
Hyprland and sway, since neither is cloned locally. srdwm is dynamic/
floating-first with opt-in tiling (`crates/core/src/layout.rs`), not
tiling-first - comparisons below are scoped to what a daily-driver desktop
user would actually notice, not "does it tile as well as sway."

## Already comparable

Confirmed by reading the actual code, not by trusting `docs/TODO.md`'s own
claims:

- **Session lock** (`ext-session-lock-v1`) - full implementation
  (`crates/wayland/src/lock.rs`), verified live with a purpose-written
  protocol test client, not just against a real locker binary.
- **Tiling** - a real dwm/i3-style master-stack layout
  (`crates/core/src/layout.rs`), opt-in per srdwm's dynamic-first design,
  not the only layout.
- **Global menu / app menu export** - `com.canonical.AppMenu.Registrar`
  and dbusmenu (`crates/platform/src/appmenu_registrar.rs`,
  `crates/wayland/src/appmenu.rs`). Neither niri nor sway ship this at all;
  it is a GNOME-HUD/Unity-era convention most compositors dropped.
- **Right-click desktop/icon context menus** - real, compositor-owned
  floating UI (`crates/wayland/src/desktop_menu.rs`,
  `crates/wayland/src/context_menu.rs`). niri and sway have no desktop
  icons at all; this is closer to what Hyprland users get from a separate
  panel, but srdwm draws it itself.
- **Gamma control** (`wlr-gamma-control-unstable-v1`, night-light/redshift-
  style color temperature) - `crates/wayland/src/gamma_control.rs`. Same
  protocol niri implements.
- **Idle handling** (`ext-idle-notify-v1`, `zwp-idle-inhibit-manager-v1`)
  - `crates/wayland/src/protocols/idle.rs`, `crates/wayland/src/
  output_power.rs`. Screen-dim/DPMS-on-idle and an app's ability to
  suppress it (a video call staying awake) both work.
- **Layer-shell** (bars, docks, launchers, notifications) - full, with
  exclusive-zone handling on fractionally-scaled outputs (a genuinely hard
  case niri and sway both get right too, and srdwm has now independently
  fixed the same class of bug on its own scale-below-1.0 feature).
- **Output management** (`wlr-output-management-unstable-v1`) - real,
  plus srdwm's own layout persistence (`crates/wayland/src/
  monitor_layout.rs`) that neither waits on nor depends on a panel to
  restore monitor position/enabled-state across a restart.
- **XWayland** - full, with WM_TRANSIENT_FOR-based dialog detection,
  clipboard bridging, and EWMH.

## Genuine gaps, ranked by how much a daily user would notice

1. **No screen-sharing / video-call screen capture.** `grep -rn pipewire`
   and `grep -rn portal` across `crates/wayland/src` return nothing. niri
   ships real PipeWire-backed screencasting (`src/screencasting/
   pw_utils.rs`) so `xdg-desktop-portal-wlr`/`-gnome` can hand a window or
   output to Zoom, Discord, Google Meet, OBS, etc. srdwm has
   `zwlr_screencopy_manager_v1` (one-shot capture, what `grim` uses) but
   nothing a portal can wire to for a *live* video stream. This is the
   single most likely thing a daily user hits and can't work around --
   worth scoping as real work, not a one-line addition (needs a PipeWire
   dependency and an `xdg-desktop-portal` backend, or at minimum wiring an
   existing generic wlr backend against srdwm's own screencopy).
2. **No `zwlr_virtual_pointer_manager_v1`.** Already flagged in `docs/
   TODO.md`'s own protocol-gaps section: `ydotool` and any UI-automation
   tool that wants precise synthetic clicks has no real protocol path here
   (`zwp_virtual_keyboard_manager_v1` exists; the pointer half doesn't).
   niri implements this (`src/protocols/virtual_pointer.rs`). Directly
   relevant to this project's own testing methodology, independent of
   whether any real user-facing app needs it.
3. **No `ext-workspace-v1`.** srdwm's workspaces are real and queryable
   (`srd workspaces`), but only through its own bespoke IPC - a
   third-party panel/switcher that speaks the standardized
   `ext-workspace-v1` protocol (what niri and sway both also implement
   for exactly this reason) has nothing to bind to. Low-impact today only
   because AGS is a first-party shell built against `srd`'s own IPC
   directly; would matter the moment someone wants to run a generic
   workspace-switcher widget unmodified.
4. **No touchscreen support (`wl_touch`).** Zero matches for
   `TouchHandler`/touch-slot handling anywhere in `crates/wayland/src`.
   Both niri and sway support it. Not clearly a gap worth closing blind --
   see `docs/TODO.md`'s own "decide scope first" note on this: a
   touchscreen on a device with no on-screen keyboard/gesture layer is a
   different, larger product decision than just wiring the protocol.
5. **No accessibility tree (AccessKit or similar).** niri ships a real
   `a11y.rs` exposing window/workspace state to assistive tech via
   AccessKit. Nothing comparable exists in srdwm. Niche for this specific
   user's own daily-driver use case, but worth naming since sway has had
   growing accessibility interest too.
6. **No live/animated wallpaper support in the compositor itself.** Not
   actually a gap versus niri or sway - neither of them render wallpapers
   either; that is `swaybg`/`swww`/`mpvpaper`-style external clients'
   job, and srdwm already has exactly that split (`awww`, a swww fork,
   launched from `~/.config/srd/autostart.sh`, with the compositor just
   compositing whatever that client draws via layer-shell). If "live
   wallpapers" means something *animated* rather than just a static image,
   that is an `awww`/wallpaper-daemon feature request, not a compositor
   one - worth confirming with whoever owns that tool before treating it
   as an srdwm gap at all.

## Deliberately out of scope / not real gaps

- **Multi-GPU.** Documented and accepted (`docs/IMPLEMENTATION_STATUS.md`):
  only the primary GPU's connectors are driven, a GPU appearing/
  disappearing is logged and ignored. Neither a laptop-daily-driver nor
  this machine's real hardware needs it.
- **A native GUI settings app.** Never existed even as working code in
  the legacy C++ project; Lua config plus `srd` CLI covers the same
  ground niri's own KDL config file does, and neither niri nor sway ship
  a GUI settings app either - this would be an srdwm-specific addition,
  not catching up to a peer.
- **Hardware DRM cursor plane / direct scanout / VRR / HDR.** Real,
  ranked gaps against niri and Hyprland specifically (both do real
  GPU compositing with these), but already tracked in `docs/TODO.md`'s
  own "Render pipeline - researched, ranked, not started" section as
  large, sequenced architectural work, not something this survey adds
  anything new to.
- **"Different monitor modes combined in one," phone-monitor/VM special
  workspace, optional AGS/srdwm phone mode.** None of niri, sway, or
  Hyprland have anything resembling these - they are not features to
  catch up on from a peer project, they are net-new product ideas
  specific to this user's own workflow. Out of scope for a feature-parity
  survey; each needs its own real design pass (see `docs/TODO.md`/the
  night's parked questions for what's still undecided about them).
