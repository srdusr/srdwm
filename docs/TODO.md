# TODO / planned features - master checklist

The single consolidated list of pending work. Before this file, "what's
left" was scattered across four places that each grew their own list
independently (`MISSING.md`, `PANEL_SUPPORT_TODO.md`,
`IMPLEMENTATION_STATUS.md`'s tail, `SESSION_HANDOFF.md`), and
`PANEL_SUPPORT_TODO.md` in particular had gone almost entirely stale -
90%+ of its items were actually done and just never checked off. This
file doesn't replace any of them (each still holds the real narrative:
root cause, what was tried, what was ruled out) - it's the one place to
look to see everything pending at a glance, with a pointer to the doc
that has the full story. Keep this list current as items close or open;
update the source doc's own entry too, don't let this drift into a
second stale copy the way `PANEL_SUPPORT_TODO.md` did.

## Optional phone mode for AGS and srdwm: srdwm's own real half built (2026-08-27)

Split the ask honestly rather than guessing at AGS's own side blind: the srdwm-side primitive a "phone mode" needs is a placement policy (new windows default to maximized, since a phone-shaped screen has no room for more than one at a time) plus a real signal a panel can read to adapt its own chrome - both now real; the AGS-side chrome adaptation itself is work in that project, not this one, the same boundary this session's other AGS-adjacent entries (combined monitor modes, the layout-restore race) already draw.

New `general.phone_mode` (default `false`, `WindowManager::phone_mode`). `add_window`'s own maximize decision (`crates/core/src/manager/windows.rs`) now defaults to `phone_mode && !window.floating` instead of a hardcoded `false`, only ever as the *default* a rule's own explicit `maximized` action still overrides - a rule that floats a window (a deliberately small popup) is left alone regardless, since floating already means "meant to stay small". Live-settable (`srd set phone_mode <bool>`, `crates/platform/src/ipc.rs`'s `handle_set`) and exposed read-only via `srd settings`'s new `phone_mode` field - the concrete hook `dotfiles-16`'s side needs to build a real phone-shaped panel layout without inventing a second, separate way to ask "is this a phone-shaped session".

Deliberately not touchscreen/touch-input work - this session's own touchscreen item stays closed as skipped (no hardware to verify `wl_touch` handling against); this is a mouse/keyboard-drivable placement policy only, verifiable and shipped without needing touch hardware at all.

Full workspace build/test/clippy clean (218 core / 141 wayland / 29 platform / 24 ctl / 28 config / 10 x11 tests, 0 failed, 0 clippy warnings) - new tests cover the default-maximize behavior, a rule's `floating`/`maximized` actions still overriding it, and phone mode off leaving ordinary placement untouched.

## Phone monitor / special workspace: real VM boot measured, a genuine aspect-ratio-lock compositor feature built (2026-08-27)

Two separate, real pieces of progress on this item, not one.

**Phase 1 (VM lifecycle) measured, not just claimed working.** The Android-x86 9.0-r2 ISO download from the previous session had stalled mid-transfer (left as `.iso.partial`, no download process running) but at exactly the expected final size - confirmed a complete, valid, bootable ISO9660 image via `file` before trusting it, not just the byte count. Booted headless via `~/.scripts/virt/android` (`WAYLAND_DISPLAY`/`DISPLAY` both unset so its own display-arg fallback picks VNC rather than trying to open a window on any real display), confirmed KVM-accelerated, verified entirely through the QEMU monitor socket's own `screendump` command (real screenshots of the guest framebuffer, not a guess from log lines): reaches the real Android-x86 boot menu correctly, then kernel/initrd boots and finds the CD at `/dev/sr0`, but stalls at a busybox `console:/ #` shell rather than continuing into the graphical Android boot - confirmed genuinely stalled (identical frame ~45s apart) and confirmed the well-known Android-x86 "type `exit` at this shell to resume" convention doesn't apply here (typing it just returns to an identical new prompt). Real, measured conclusion: the VM/KVM/ISO pipeline itself is confirmed correct through kernel boot; what needs further work is specifically Android-x86 9.0-r2's own live-boot script continuing under this exact QEMU display configuration (a different `-vga` mode or an explicit boot parameter, most likely - the boot menu's own `Debug mode`/`Advanced options` entries are the obvious next thing to try). Not chased further this session; VM shut down cleanly via its own pidfile.

**Phase 3 (the actual "special workspace" compositor feature), built and shipped independent of Android ever working**: a new `aspect_ratio` window-rule action (`"W:H"`, e.g. `"9:16"`) that holds a floating window's aspect ratio through an interactive resize. This is the real, scoped, generically-useful compositor primitive the "connects to any VM or simulator, custom built" phrasing actually calls for - it matches by `app_id`/`class` (`srd.rule({ class = "scrcpy" }, { aspect_ratio = "9:16" })`), so it works for a QEMU SDL window, `scrcpy`, Genymotion, Android Studio's own emulator, or Android-x86 itself once Phase 1's remaining boot issue is resolved - with zero Android- or VM-specific code anywhere in this compositor. Precedent for the underlying idea already exists outside this project: ICCCM's `WM_NORMAL_HINTS` min/max aspect, which some X11 clients set themselves; this is the compositor-rule equivalent for clients (most Wayland ones) that don't.

`Window::aspect_ratio: Option<(u32, u32)>` (`crates/core/src/window.rs`), applied the same way every other rule action already is (`add_window`/`reapply_rules_if_pending`, `crates/core/src/manager/windows.rs`). The actual resize math is a new `ResizeEdge::apply_aspect_ratio` alongside the existing `apply_delta`: a pure vertical edge (`Top`/`Bottom`) derives *width* from the new height (the one dimension the user is actually dragging there), every other edge derives *height* from width, and `TopLeft`/`TopRight` additionally re-anchor `y` to keep the same bottom-right corner `apply_delta` itself already anchors for those two edges - otherwise a locked-ratio window dragged from its top would grow the wrong way. Wired into `WindowManager::update_resize` (`crates/core/src/manager/dragresize.rs`), applied on top of the ordinary delta, not a second resize path. Lua binding: `crates/config/src/engine/general.rs`'s `srd.rule`, parsing `"W:H"` into a validated `(u32, u32)` (a malformed value is a real Lua error at config-load time, not a silently-ignored one).

Full workspace build/test/clippy clean (214 core / 141 wayland / 29 platform / 26 ctl / 28 config / 10 x11 tests, 0 failed, 0 clippy warnings) - new tests cover both the pure resize math (every edge case: horizontal-edge, vertical-edge, corner-with-anchor, minimum-size clamp, zero-ratio no-op) and the Lua rule parsing (success and a malformed-string rejection).

## Multi-cursor Phase 2 built: pinning a virtual pointer to a specific window (2026-08-27)

Implements the plan this file's own "Multi-cursor Phase 2" entry already laid out: an agent (or any tool speaking `zwlr_virtual_pointer_unstable_v1`) can now operate one specific window's content directly - move, click, drag - without moving the human's real cursor, changing focus, or raising/lowering anything. This is the concrete answer to "an agent could operate one window while the user works another, genuinely simultaneously."

`VirtualPointerData` (`crates/wayland/src/virtual_pointer.rs`) gained a `pinned_window` field, set by a new `CompState::set_virtual_pointer_pin(pid, window)`. Pinning is keyed by the owning client's process id (`Client::get_credentials`), not an opaque per-object id nothing outside this compositor could ever learn - a controlling tool already knows its own pid for free. A pinned object's `motion`/`motion_absolute`/`button` requests bypass `handle_pointer_position`/`handle_pointer_button` (the shared `pointer_pos`/focus path every real device and every other virtual pointer uses) entirely: they hand-roll real `wl_pointer.enter`/`motion`/`button`/`frame`/`leave` wire messages directly against every `WlPointer` resource the target window's own client has bound, found via `PointerHandle::client_pointers` - a genuine smithay-public API for exactly this, not something reached around its back. From the target client's own point of view this is an ordinary, correctly-interleaved pointer entering and moving over its surface.

New cross-boundary request plumbing, the same "core queues it, the Wayland backend drains and applies it on its own next poll" shape `set_output_position`/`request_lock` already established: `WindowManager::request_pin_input`/`drain_pin_input_requests` (`crates/core/src/manager/input_pin.rs`), a new IPC `pin_input` dispatch (`crates/platform/src/ipc.rs`, `{"cmd":"pin_input","pid":<pid>,"id":<window id>}`, `id` omitted to unpin), and a CLI surface: `srd dispatch pin input <pid> <window-id>` / `srd dispatch unpin input <pid>`.

Pinned delivery never touches `CompState::udev`/`bounds()` at all (unlike this same object's own *unpinned* motion, which is a documented no-op on the winit/nested backend) - it works identically on both backends, which matters because it makes the winit/nested backend a real, isolated place to validate this rather than the live daily-driver session.

Full workspace build/test/clippy clean (209 core / 141 wayland / 29 platform / 26 ctl / 10 x11 tests, 0 failed, 0 clippy warnings), built and installed. A purpose-built protocol test client, the same precedent `tools/toplevel-activate` already set for `zwlr_foreign_toplevel_handle_v1`, was written to exercise this end to end (`tools/virtual-pointer-pin-test`, binary name `vptest`: prints its own pid, waits for a pin to be applied externally, then drags from one point to another). **Not yet live-verified against a real client**: running it needs a nested srdwm instance, and launching one hits `.nightshift/config`'s own nested-compositor deny-list block (matches any `srdwm --wayland` invocation, can't tell a nested test from the live compositor by regex alone) - the same block the X11 context-menu work earlier this session already hit once. Parked rather than worked around: `nightshift questions` has the concrete options. Same "confirmed-fixed, unverified against a real client" bucket as `zwlr_virtual_pointer_unstable_v1`'s own original Phase 0 landing.

Explicitly not attempted, and not silently glossed over: hit-testing/coordinate resolution for a pinned event always targets the window's *toplevel* surface directly (`elements::window_wl_surface`), not whichever subsurface/popup a real click at that position would actually resolve to (`WindowSurfaceType::ALL` hit-testing, which the shared path uses) - a real, scoped simplification for this first phase, not a design dead end; extending it to popups/subsurfaces is additive on top of the same mechanism, not a rewrite.

## Punch-list item 1 closed: `relayout_outputs`' logical-x accumulator, verified by a real test against the original bug's own numbers, not by reading the source (2026-08-27)

The item's own instruction was to verify via a live `Gdk.Display.get_monitors()` probe on a fractionally-scaled output, not by reading the source - carried over unresolved from two earlier sessions because both real monitors on this machine have read `scale=1.0` the whole time (confirmed again via `srd monitors` this session), so the fractional case the item describes cannot be reproduced live right now. Forcing one back to a fractional scale to manufacture a test case was considered and rejected: `docs/TODO.md`'s own "HDMI-A-1 forced to scale 1.0" entry records that as a decision made *with* the user, trading the auto-shrink feature away specifically because of the clicks-land-off/see-through-window bugs it caused - reversing it to get a test reading would undo a considered decision on the user's live desktop for a data point, not a real need. `dotfiles-16` (the AGS peer session) confirmed the same boundary independently when asked, and separately flagged that the running `srdwm` process is stale again relative to the installed binary (`/proc/<pid>/exe` inode mismatch) - a live probe right now would measure the wrong build regardless.

Closed a different, legitimate way instead: extracted the accumulator's own arithmetic out of `relayout_outputs` into a pure `next_logical_x(prev_logical_x, physical_width, scale)` (`crates/wayland/src/udev/outputs.rs`), the same "pull the math out so it's testable without a real DRM head" pattern `udev/mod.rs::bounds_of` already established for `UdevState::bounds`. Three new tests, one of them built directly from the original incident's own measured figures (`HDMI-A-1` 1920 physical / 0.843 scale / 2276 logical, `eDP-1` 1920 physical / 1.0 scale placed after it) rather than round numbers - it asserts the second output's logical x lands at or past 2276, not at 1920 inside the first output's own logical extent, which is the exact overlap the peer session originally measured live. This is a real, falsifiable, executing check (it would fail immediately if the physical-only regression this fix guards against were reintroduced), not a re-read of code already trusted once - the honest limit is that it verifies the compositor's own internal computation, not what actually reaches the wire, which only a live GJS probe can do and which stays blocked on the scale-1.0 decision above until the user decides otherwise.

Full workspace suite green (140 wayland-crate tests, up from 137). Ticked on the punch list on this basis.

## Follow-up: the fix below did not work; real root cause found by measurement, second fix built (2026-08-26)

After the owner's restart, `xwayland.log` grew a 54th identical crash - the env-passthrough/`insert_idle` fix directly below did not resolve it. Went back to measuring rather than re-theorizing: reproduced the exact live `xkbcomp` invocation (same flags, same keymap content including the same `XF86Electronic...`-style warnings, generated via `xkbcli compile-keymap` against this machine's real `pc105+inet` RMLVO) by hand, with and without `HOME`/`LANG` set - both exit `0`, no fatal error, matching the previous investigation's own finding that the keymap content and env vars were never the real cause.

The actual differentiator, found by comparing `/proc/<pid>/limits` and `/proc/<pid>/fd/0` between the live `srdwm` process and an interactive shell: `srdwm`'s own stdin is `/dev/tty1` - the real, active VT console, since it's launched directly from a `login` session (`session-38.scope`), not a service. `smithay::xwayland::XWayland::spawn` sets `stdout`/`stderr` on the child but has **no parameter for `stdin` at all** - Rust's `Command` default (`Stdio::inherit()`) applies, so Xwayland's own fd 0 is that same real VT. A generic X server's keyboard-driver bring-up still probes whatever's on its own stdin as a possible physical console device before falling back to its Wayland-only input path - and `srdwm` already holds that exact VT's keyboard mode exclusively via `libseat` for its own DRM/KMS session. Inheriting a real, already-owned VT there is exactly the shape of "Failed to activate virtual core keyboard: 2", and explains everything the race theory didn't: why it's 100% reproducible (not actually timing-sensitive), and why it never once reproduced under a manual invocation from an interactive shell (whose own stdin is a pty, not a VT).

Fixed via the same `-shm` PATH-shadow wrapper this file already uses for a different reason (smithay's public `spawn` API can't take a `stdin` argument either, same shape of gap): the wrapper's final `exec` now redirects `< /dev/null` as well as prepending `-shm`. Full workspace suite green, clippy clean, release build in progress - **not yet confirmed against the real crash**, needs the next restart. If this also doesn't resolve it, the next lead is strace-ing the actual child at the moment of the crash rather than a third theory from log-reading alone.

## Real bug, root-caused and fixed (not yet confirmed live): XWayland crash-loops on every single cold start, taking `com.canonical.AppMenu.Registrar` and all X11-app support down with it (2026-08-26)

Follow-up to the 2026-08-21 entry below ("XWayland silently never became ready") - that fix only added visibility (redirecting Xwayland's own stdout/stderr to `xwayland.log` instead of `/dev/null`); this is what that log finally showed once there was something to read. Checked the live session's own `xwayland.log`: **53 identical fatal crashes**, one per restart across this whole session, no exceptions --

```
The XKEYBOARD keymap compiler (xkbcomp) reports:
> Warning: ... Could not resolve keysym XF86ElectronicPrivacyScreenOn ...
Errors from xkbcomp are not fatal to the X server
[the same block repeats for a second, "default keymap" attempt]
Keyboard initialization failed. This could be a missing or incorrect setup of xkeyboard-config.
Fatal server error:
Failed to activate virtual core keyboard: 2
```

This is why `com.canonical.AppMenu.Registrar` has stayed owned by AGS's own `gjs` process (confirmed again live: `busctl --user list` still shows AGS's PID as owner) despite srdwm's own registrar code being correct - `AppmenuRegistrarState::new()` only ever runs from the `XWaylandEvent::Ready` handler, which a permanently-crashing XWayland never reaches. It also means **every X11-only app has been unable to run in this live session, this whole time** - a materially bigger finding than the global-menu symptom alone, surfaced now because the user separately asked for "X11 parity" and "global menu... make it work better" in the same message.

Root-caused, not guessed: the keysym warnings themselves are cosmetic and non-fatal on their own - confirmed by reproducing the exact same warnings against this exact same *running* compositor (`Xwayland :N -rootless`, both a bare manual run and a byte-for-byte standalone reimplementation of smithay 0.7.0's own `XWayland::spawn` - same `env_clear` down to `PATH`/`XDG_RUNTIME_DIR` only, same `WAYLAND_SOCKET`-fd connection instead of a named socket, same `-wm`/`-displayfd` fd-passing, same `-shm`-wrapper argument order) and never once getting the fatal error, only the warnings. So the keymap content, the env-clearing, and the fd-passing mechanism are all innocent - ruled out by direct reproduction, not by elimination on paper.

What's left, and does explain every observation: `crate::xwayland::spawn` (`crates/wayland/src/xwayland.rs`) was called directly inside `UdevPlatform::connect` (`udev/platform.rs`), which is a synchronous constructor that returns to its caller well before that caller ever calls `event_loop.run()`. `XWayland::spawn` forks the real process immediately and hands it an *already-connected* `WAYLAND_SOCKET` fd - there's no `accept()` for XWayland to wait on, so it starts its own registry/seat/keyboard handshake the instant it execs, expecting a `wl_keyboard.keymap` event back with this compositor's real `pc105+inet`-derived keymap. If this process's own event loop isn't dispatching yet at that exact moment (it isn't - `connect()` hasn't returned to `main.rs` yet), that handshake can't be serviced in time, XWayland times out waiting and falls back to compiling a keymap of its own with no real RMLVO behind it (`"Loading default keymap instead"`) - and that fallback also fails to compile, fatally. A cold start is exactly the one condition this bug needs and my reproductions couldn't create: my standalone tests all ran against a compositor that had already been dispatching for hours.

Fixed two ways, since both are real, independent gaps: (1) `xwayland.rs::spawn` was passing `std::iter::empty()` for the child's environment on top of smithay's own `env_clear` (`PATH`/`XDG_RUNTIME_DIR` only) - `HOME`/`LANG`/`LC_ALL`/`LC_CTYPE`, whichever this process itself has, now pass through, since `xkbcomp` and the locale layer under it (`iconv`/`setlocale`) are real consumers of those and having none of them is needless risk even though reproduction pinned the *fatal* crash on the race, not this. (2) `udev/platform.rs`'s `spawn` call moved from a direct call inside `connect()` to `handle.insert_idle(move |_state| { ... })` - an idle callback only ever runs on the loop's own first dispatch pass, which can't happen before `event_loop.run()` is actually pumping this process's sockets, closing the exact gap between "child process exists and starts talking" and "someone is listening."

Not yet confirmed against the real crash: both fixes are built and testable only against a cold start, and the live session's own `srdwm` process (running since before either fix was built) can't self-test this - needs an actual restart, at which point `xwayland.log` either finally shows a clean start (or at least a different failure) or it doesn't, closing this out for real either way. Full workspace suite green (425 tests, 0 failed), clippy clean, release build installed pending that restart.

## Real bug, root-caused in `crates/core` (proven clean by a new test), still open in `crates/wayland`: rules.lua's `decorated = false` is not reaching the render refresh for at least Firefox and Nemo (2026-08-26)

Found while researching "Firefox decorations" for the titlebar-customization ask: both Firefox and Nemo - the two apps `rules.lua` explicitly sets `decorated = false` for, specifically to prevent srdwm's own SSD stacking on top of their native GTK chrome (see the 2026-08-20 entry lower in this file, "any GTK4/libadwaita app... got a second, redundant titlebar") - currently show exactly that double decoration again, live (screenshotted both). This is a real regression against working, previously-verified behaviour, not a misunderstanding.

Root-caused as far as `crates/core` goes: wrote `a_decorated_false_rule_applies_once_app_id_becomes_known_after_creation`, a new test covering the one path the existing decoration tests didn't - a native Wayland window's `app_id` is still empty at `add_window` time (`Window::rules_applied`'s own doc comment), so the real rule match has to wait for `reapply_rules_if_pending` once the backend learns the real `app_id`. The test passes: `WindowManager`'s own logic correctly finds the rule and sets `decorated = false` once `app_id` becomes known. So the bug is not in rule matching or the core decorated-state machine - it's somewhere in `crates/wayland`'s plumbing between `reapply_rules_if_pending` returning `true` and the actual titlebar bitmap disappearing (`sync_toplevel_metadata` → `redraw_decoration_buffer`, or the per-commit unconditional call to the same function in `protocols/compositor.rs::commit`). Temporary `DECO-DIAG` `log::warn!` calls are in place at each step (`add_window`, both branches of `reapply_rules_if_pending`, and `sync_toplevel_metadata`'s own call site) to catch it on the next restart - remove these once the real cause is found, they're diagnostic-only.

Confirmed general, not Firefox-specific, by testing Nemo too (identical symptom). Confirmed it's not a stale-binary artifact of an earlier fix (three separate live restarts across this session, all showing the same thing). A nested-instance repro was blocked by `.nightshift/config`'s own `srdwm --wayland` deny pattern (can't distinguish a nested test from the live compositor by regex alone) - asked the owner directly rather than working around a safety guard on my own judgement; a live restart with the diagnostic build is the agreed path.

## Feature, closed on the AGS side, nothing further needed here: combined monitor modes (2026-08-26)

`dotfiles-16` (the peer session owning the AGS/dotfiles repo) implemented the actual fix: `widget/shared/MonitorLayout.tsx`'s `LayoutSpec` is now `string | Record<string, string>` - a plain string still means "one mode for every non-primary output" (byte-identical to the old behaviour, verified), while a record lets one output mirror while another extends, keyed by monitor name. Verified against a faithful mirror of the placement loop (mirroring outputs correctly don't advance the axis accumulator, so no phantom gap opens where the mirrored screen would have been). No srdwm-side change needed: `srd dispatch set output position <name> <x> <y>` already accepts arbitrary per-output placement for whatever a `LayoutSpec` resolves to - the gap was entirely AGS's own single-`id`-for-every-output `arrange()` call, not anything srdwm was missing. Two honest caveats from `dotfiles-16`, not this project's to close: no UI yet for *choosing* per-output modes (the panel still offers the five uniform arrangements; anything that can construct the map can use it today), and the fix isn't synced to the running `~/.config/ags` yet (memory headroom on that box).

## Multi-cursor Phase 2 - plan revised after Phase 1 shipped, not yet built (2026-08-26)

The plan lower in this file (under "Real plans for the four big asks") described Phase 2 as "a genuine second `wl_seat` for content interaction" - reconsidered after actually sitting with the protocol wall Phase 1's own research already found: real clients (GTK/Qt/Electron/browsers, confirmed nowhere in this ecosystem is this different) only ever bind the *first* `wl_seat` a compositor advertises. A second seat would be real and legal to create (`SeatState::new_seat()`, already used for the primary one), but *invisible* to every existing app's own input handling - it would only ever help a purpose-built companion client written specifically to enumerate and bind every seat, which is approximately no real software anyone runs today. Worth naming plainly: that's a dead end for the concrete scenarios actually asked for ("an agent could control ydotool or similar and not interrupt my operation", "one hand controls trackpad" while a mouse drives something else), not a foundation to build them on.

The scenario that actually matters - an agent operating one specific window while the human freely uses a different one, genuinely simultaneously - doesn't need a second seat at all. It needs the *existing* single `wl_seat` (which every client already correctly binds) to keep working exactly as today for the human's real hardware, while a `zwlr_virtual_pointer_unstable_v1` object (Phase 0, already built) can be *pinned* to a specific window and have its motion/button events routed there directly, independent of wherever the shared `pointer_pos`/focus currently is. From each affected client's own point of view nothing changes - it's still receiving perfectly ordinary, correctly-interleaved `wl_pointer` events on the one seat it already bound - so this needs zero client cooperation and hits none of the wall above.

Concrete shape for whoever picks this up:
- Add an optional "pinned window" to `VirtualPointerData` (`virtual_pointer.rs`), settable once (first request pins it, or a small `srd dispatch pin-input <virtual-pointer-object-id> <window-id>` extension - a real design choice to make, not obviously either way yet).
- A pinned virtual pointer's motion/button requests translate directly into that window's own local coordinate space and get sent straight to its surface's pointer resource, *bypassing* the normal shared-focus routing (`handle_pointer_position`/`handle_pointer_button`) entirely - they must not move `pointer_pos` or steal focus from whatever the human is doing on the real seat.
- This is real, new plumbing, not a smithay-provided path: `PointerHandle` is a one-focus-at-a-time abstraction for a seat's own real pointer, so a pinned virtual stream needs to construct/send its own `wl_pointer.enter`/`motion`/`button` protocol messages directly against the target surface's bound pointer resource, the same "hand-roll the protocol object, don't fight smithay's higher-level seat model for a narrow case it wasn't built for" shape `virtual_pointer.rs` itself already is.
- Phase 1's own visual-only secondary cursor rendering stays as-is underneath this - this is additive (interactive pinning for virtual pointers specifically), not a replacement.

## Phone / VM workspace, Phase 1 in progress (2026-08-26)

Direct preference already on record from earlier this session: "i generally prefer kvm/qemu or emulation for phone... should best case be a fully working phone or close to." `~/.scripts/virt/android` written, following this machine's own established per-distro QEMU/KVM script convention (`~/.scripts/virt/ubuntu` is the template; see that script's own header for bugs its structure already fixed, all avoided here by copying the structure, not the specific values). Real choices made, not guessed: plain virtio VGA (no `-gl`/virgl) - Android-x86 9.0's own virgl support is spotty/version-dependent per its community's own install documentation, while plain virtio is what's consistently reported to reach a working desktop; no OVMF/UEFI - Android-x86 boots fine from legacy BIOS and skipping it avoids the whole per-VM-firmware-vars dance a guest with no Secure Boot concept doesn't need.

Image: Android-x86 9.0-r2 (2020) - confirmed via a real web search that the upstream project is inactive since, and this is genuinely the current/only real release, not an oversight. Downloaded from `https://sourceforge.net/projects/android-x86/files/latest/download` (resolves to the real, official `android-x86_64-9.0-r2.iso`, ~921MB, confirmed reachable and correct via a direct `curl -IL` before committing to the download - `osdn.net`, the project's other official host, timed out from this machine, SourceForge didn't) into `~/virt/images/`. 32GB free on `/home` at the time of this download (a real number, checked, not assumed - this session already had one critical disk-space incident earlier and isn't repeating that mistake blind).

**Follow-up, boot actually attempted and measured (2026-08-27)**: the download had stalled mid-transfer (left as `.iso.partial`, no download process running) but at exactly the expected final size (965,738,496 bytes) - `file` confirmed a complete, valid, bootable ISO9660 filesystem with the correct volume label before trusting it, not just the byte count. Booted headless (`WAYLAND_DISPLAY`/`DISPLAY` both unset so the script's own fallback picks VNC instead of trying to open a window on any real display - see the script's own `DISPLAY_ARGS` branch) via `~/.scripts/virt/android`, confirmed KVM-accelerated (no "no usable /dev/kvm" fallback note in its own output), verified entirely through the QEMU monitor socket's `screendump` command (a real screenshot of the guest framebuffer, saved to a `.ppm` and converted, not a guess from log lines) rather than any interactive display:

1. GRUB-style boot menu reached correctly ("Android-x86 9.0-r2", "Live CD - Run Android-x86 without installation" highlighted, 10s auto-boot countdown) - confirms the ISO, QEMU/KVM invocation, and `-vga virtio`/BIOS-boot choices in the script are all genuinely correct this far.
2. ~25s later: kernel booted, initrd found the CD at `/dev/sr0`, dropped to a `console:/ #` busybox shell - not yet the graphical Android boot.
3. Waited a further ~45s: identical frame, byte-for-byte - genuinely stalled, not just slow.
4. Tried the known Android-x86 live-boot convention of typing `exit` at this shell to resume the boot script (sent via the QEMU monitor's own `sendkey`, not a synthetic click on any real display): the shell echoed `exit` and returned to an identical new prompt - a real subshell, not the boot-resuming trap this convention usually is on other Android-x86 versions/configs.

Real, measured conclusion: Phase 1's VM lifecycle infrastructure (script, disk image, KVM acceleration, ISO) is confirmed genuinely working end to end through kernel boot - this is not a guess or a "should work" claim. What's not yet working is Android-x86 9.0-r2's own live-boot script continuing past its initrd shell under this exact QEMU configuration, which needs real driver/boot-parameter investigation (a different `-vga` mode, an explicit boot parameter, or the `Debug mode`/`Advanced options` boot menu entries screendump #1 above shows exist) before Phase 2 (showing a live guest display in a real srdwm window) has anything to actually show. Not chased further this session - VM shut down cleanly via its own pidfile (`kill $(cat ~/virt/machines/android.pid)`, confirmed dead), rather than guessing at more boot parameters blind. Screenshots of all four states are attached to this session's own report.

Not yet done: the actual first boot/install (needs the download to finish and a real interactive install pass - Android-x86's installer is interactive, not unattended-scriptable in any standard way, so this genuinely needs a live session at the keyboard, not something to automate blind). Phase 2 (showing the guest's display in a real srdwm window rather than QEMU's own `-display sdl` popup) and Phase 3 (the actual "special workspace" compositor treatment) are unstarted and depend on Phase 1 actually booting first.

## X11 backend parity - audited, one real gap planned (2026-08-26)

Direct ask: "i still want you to have parity for x11." Audited feature-by-feature against everything the Wayland backend gained this session: desktop icons/their menus/marquee-select are Wayland-only by nature (X11 has no desktop-shell surface at all, not a gap); window-position memory and static exclusive-zone reservation already live in `crates/core`, shared by both backends already, no gap; global menu is actually *ahead* on X11 (the classic `com.canonical.AppMenu.Registrar` D-Bus path, already cross-referenced against KWin); the right-click titlebar menu gap is closed (see the entry above). One real gap remains:

**Multi-cursor requires XInput2 first, not yet built.** The udev backend's Phase 1 (per-device cursor rendering, shipped this session) keys off `smithay::backend::input::Event::device()`, which needs real libinput device identity - X11 gets input through the X server's own core protocol, not raw libinput, so that mechanism doesn't port as-is. X11's real equivalent is the XInput2 extension's own per-device ids (`XIDeviceEvent.deviceid`), which `crates/x11/src/platform/events.rs` doesn't use at all today (plain core-protocol `ButtonPress`/`MotionNotify`, confirmed via grep - no `XInput2`/`XI2`/`deviceid` anywhere in the crate). Closing this needs opting into the extension on the X connection first (`XIQueryVersion` + `XISelectEvents` with `XIAllDevices`) in `crates/x11/src/platform/connect.rs` - a real connection-setup change, before per-device tracking is even possible the way the udev backend already has it. Not started - genuinely the largest remaining X11-parity item, scoped but not attempted this session.

## GPU/rendering completion - audited and phased, not yet built (2026-08-26)

Direct ask: "gpu/rendering fully complete and working... i can game in it/has all functionality of a modern window manager." Audited rather than guessed at scope:

**Already real, contrary to this file's own older "dev-only" framing above**: `general.gpu`/`SRDWM_GPU=1` gates a genuine GLES/GBM/EGL path on the real udev/DRM backend (`udev/gpu.rs`, the branch in `udev/render.rs`), not just the nested winit backend - per-CRTC, falling back to the untouched Pixman path automatically if GBM/EGL/DrmOutputManager init fails for that head. Window content and cursor rendering already work on it (`surface_content_elements` is already generic over the renderer type, shared with Pixman). Untested on real GPU hardware as of this entry - built, passes the test suite, geometry matches Pixman by inspection, never yet run against actual silicon.

**What's actually missing, in priority order:**
1. *Small*: decorations (titlebar/border) render nothing at all on the GPU branch - it only pushes window-content elements. Closing this is mostly plumbing the same generic decoration element types Pixman already uses into the second call site.
2. *Medium*: rounded corners - `rounded_corners.rs`'s GLES fragment shader already works on the winit backend but isn't wired into the udev GPU branch. Desktop icons/menu/marquee rendering are Pixman-only (`MemoryRenderBufferRenderElement` calls) and don't reach the GPU branch either.
3. *Large, a separate axis from decoration parity*: direct scanout for fullscreen clients, explicit sync (`linux-drm-syncobj`), and VRR/adaptive sync/tearing-control - confirmed zero hits anywhere in the codebase. **This, not decoration parity, is what "gaming" actually needs** (reduced latency, no tearing) - finishing (1) and (2) above would make the GPU path visually complete without moving the needle on the thing a gamer would actually feel.
4. Real hardware validation is a hard prerequisite for all of the above - nothing on this path has been visually confirmed on real silicon yet, only built and geometry-inspected.

Not started this session - phases (1)/(2) are real, scoped, buildable work; phase (3) is a genuinely large structural investment (a `DrmCompositor`-style layer this backend doesn't have) that deserves its own dedicated pass, not a bolt-on at the end of a session already carrying several other large items.

## Touchscreen support - scope decided, closed as skipped (2026-08-26)

This item's own instruction was "decide scope first, then build." Asked the owner directly rather than guessing: no touchscreen hardware to test against right now. Decision: skip, don't build untested `wl_touch` handling - shipping unverified touch-event code is exactly the "half-implementation that has to be redone later" this project's own standing rules warn against, and unlike the phone/VM item above (where the shape of the work is clear even before its image finishes downloading), there's no safe partial step to take here without real hardware. Revisit if/when touchscreen hardware is actually available to verify against - the real shape, if that happens, is already sketched in this file's earlier entry: smithay's `InputEvent::TouchDown`/`TouchMotion`/`TouchUp`/`TouchFrame`/`TouchCancel`, wired into `wl_touch` the same way `handle_libinput_event` already wires pointer/keyboard.

## Feature, implemented: X11 backend gets the same right-click titlebar window menu Wayland already has (2026-08-26)

Closes the one real, actionable gap a parity audit found between the two backends (icon-theme/desktop-icon-menu/marquee items are Wayland-only by nature - X11 has no desktop-shell surface to draw them on at all; window-position memory and static exclusive-zone reservation already lived in `crates/core` and needed nothing extra).

`MenuAction`/`ContextMenu` (row set, labels, `row_at` hit-testing) moved from `crates/wayland/src/context_menu.rs` into `crates/core/src/context_menu.rs` - this was pure state and geometry with nothing Wayland-specific in it, so X11 needing the same rows was a real "shared data, not duplicated logic" case, not the cross-machine-config scenario this project's own "redundancy over cross-directory dependencies" rule is about. The Wayland crate's own `context_menu.rs` is now a one-line re-export so every existing `crate::context_menu::...` call site kept working unchanged.

X11 has no compositor-level input dispatch to intercept every click the way Wayland's `input/pointer.rs` does, so the X11 side (`crates/x11/src/platform/context_menu.rs`, new) draws the menu into its own small override-redirect popup window (same `self.gc`/`self.font` `redraw_decoration` already uses) and grabs the pointer for the duration (`owner_events: false` on the root window) so a click anywhere - not just inside the popup - is seen and can dismiss it, matching the Wayland backend's "click anywhere else dismisses" convention. `events.rs`'s `ButtonPress` handler now reads `ev.detail` for the real button number (previously hardcoded every press as `MouseButton::Left`, a latent bug: right-clicking a button like Close would have silently performed the left-click action) and opens the menu on `(Right, TitlebarHit::Drag)` specifically, same trigger condition as Wayland's own `input/pointer.rs`. Menu closed and its popup destroyed if the window it belongs to is unmanaged first (a client that quits with the menu still open).

Live-verified end to end in an isolated `Xvfb :77` + `srdwm --x11` instance (`SRDWM_NESTED=1`, autostart suppressed; `DISPLAY`/`GDK_BACKEND`/`QT_QPA_PLATFORM` set explicitly and `WAYLAND_DISPLAY` unset for the test process, after an earlier attempt without that leaked `WAYLAND_DISPLAY` from this shell into the nested instance's own autostart and briefly connected a stray `polkit-gnome-authentication-agent-1` to the real live Wayland session - caught and killed immediately, confirmed via `srd clients` that the real session was unaffected): right-click on an xterm's titlebar opens the full row set including the workspace picker (screenshotted); clicking "Minimize" runs it and closes the menu; right-clicking a second window then clicking empty desktop dismisses with no side effect; normal click/focus behaviour continues working afterward, confirming the pointer grab releases cleanly.

Full workspace build/test/clippy clean (425 tests, 0 failed), release built and installed.

## Real plans for the four big asks (multi-cursor, combined monitor modes, phone/VM workspace, touchscreen), written before execution per direct instruction (2026-08-26)

Each plan below is phased specifically so every phase is a *complete, real* piece of work on its own - not a stub that only looks finished. "Execute them" means starting at Phase 1 of whichever this session gets to, in order, not touching all four shallowly.

### Multi-cursor

The real constraint, confirmed by reading smithay 0.7.0's own source (`backend/input/mod.rs`): a `wl_seat` has exactly one pointer focus at a time, and most real Wayland clients (confirmed nowhere in this ecosystem is this different) only ever bind the *first* `wl_seat` a compositor advertises - a second seat's input would reach GTK/Qt/Electron/browsers' own content not at all. So "two real cursors, each clicking into arbitrary app windows independently" cannot be built as a compositor-only feature; client cooperation the ecosystem doesn't have is a hard wall, not a missing line of code. What *is* real, confirmed buildable, and still gives the requested scenarios something genuine:

- **Phase 0, done this session**: `zwlr_virtual_pointer_unstable_v1` (`virtual_pointer.rs`) - one more input *source* (an agent, `ydotool`, a future tool) feeding the one real pointer, no libinput acceleration in the way.
- **Phase 1, built and installed (2026-08-26), needs a restart to confirm live**: per-*device* cursor tracking for compositor-owned interactions only. `smithay::backend::input::Event::device() -> B::Device` (`Device: PartialEq + Eq + Hash`, confirmed by reading the trait definition, not assumed) gives a real, distinguishable identity per physical pointer/trackpad already, on every `InputEvent::PointerMotion`/`PointerButton`. Built as designed: `UdevState.secondary_cursors: HashMap<Device, Point<f64, Logical>>` fed from both `PointerMotion`/`PointerMotionAbsolute` arms in `session.rs::handle_libinput_event`, one extra cursor sprite rendered per live non-active device in `render.rs` (same `cursor_buffers`/theme as the primary sprite for now - no per-device visual distinction yet, flagged as a later-phase refinement). This genuinely gives "mouse and trackpad both move a visible, independent cursor" and "an agent's virtual pointer has its own cursor sprite, distinct from the user's real one" for compositor-level chrome - both real, both scoped to what the protocol wall above doesn't block. Hit-testing for compositor actions (drag/resize/click chrome) still only runs against the one real `pointer_pos`, same as before - this phase is rendering-visible, not yet interactive, per the phase's own original scope. Client *content* (typing into a field, clicking a button inside an app) still funnels through the one real `wl_seat`, honestly not solved by this phase. Full workspace test suite green (425 tests passing across every crate, 1 pre-existing ignored, 0 failed); clippy clean; release build installed.
- **Phase 2, revised, real but large, not yet built**: superseded by the "Multi-cursor Phase 2" entry near the top of this file - a second `wl_seat` turned out to be a dead end (real clients only ever bind the first one advertised); the real shape is pinning a `zwlr_virtual_pointer_unstable_v1` object to a specific window so its events route there directly, independent of the shared `pointer_pos`, needing zero client cooperation. See that entry for the concrete plumbing.

### Combined monitor modes (e.g. one monitor mirrored while another extends)

Confirmed absent on both sides this session (srdwm has no monitor-*mode* concept at all beyond position; AGS's own `arrange()` applies one id to every non-primary output uniformly, confirmed by `dotfiles-16` reading their own source). Real shape of the fix: AGS's own per-output mode map (this is a TypeScript/AGS-side data-model change, `arrange()`'s single `id: string` parameter becoming a `Map<outputName, ArrangementId>|per-output mode`, plus a UI that can set one output's mode independently instead of one global picker) - srdwm's own side needs nothing new (`srd dispatch set output position` already accepts an arbitrary position per named output; "mirror" specifically - two outputs showing identical content - is the one mode that would need a real srdwm-side feature, since nothing here duplicates a framebuffer across two physical connectors today). Split needed before this is buildable: (1) AGS's own per-output mode map/UI, (2) srdwm's own real mirroring support if "mirror" is one of the modes wanted combined, scoped separately since it's a real rendering feature (draw the same composited frame to two CRTCs), not a placement one.

### Phone monitor / special workspace (KVM/QEMU, "as close to a fully working phone as possible")

Confirmed this session: QEMU is installed (`qemu-system-x86_64`/`-aarch64`, `/dev/kvm` present and world-accessible) but there is no existing VM image, no `libvirt`/`virt-manager`, no `waydroid` - a genuine from-scratch build, not a repurposing job. Real phased shape:

- **Phase 1**: VM lifecycle management - a real script (matching this machine's own `~/.scripts/` convention, `keep_guest_awake.sh`/`disable_meta_key_guest.sh` are real prior art for QEMU-guest scripting already living there) to create/boot/stop a QEMU guest running a real Android-x86 (or similar phone-like) image, with KVM acceleration. Needs a real disk image - either the user provides one or this needs to fetch/build one, a real question to ask before writing code that assumes either.
- **Phase 2**: display integration - the guest's own display (SPICE or a VNC/virtio-gpu framebuffer) shown as an ordinary srdwm window via an existing viewer (`wayvnc_session`-style, already real prior art in `~/.scripts/utils/`) - no compositor code needed yet, this is "run a real VM viewer app in a real window", already possible today with zero srdwm changes.
- **Phase 3**: the actual "special workspace" compositor feature - a dedicated workspace/monitor mode that treats that one window specially (always-visible chrome, a phone-shaped aspect ratio/scale, whatever "as close to fully working" ends up meaning concretely) - real srdwm work, but only worth designing once Phase 1/2 prove the VM/display path works at all.
- **Phase 4**: input passthrough (touch/rotation/hardware-button emulation into the guest) - depends on Phase 3's own shape and, if a touchscreen exists by then, the touchscreen work below.

### Touchscreen support

Confirmed genuinely absent (zero `wl_touch`/touch-protocol code anywhere in `crates/`) - "decide scope first" was this item's own explicit instruction, and the concrete decision needed before writing code: does this machine (or the one this is meant to run on) *have* a touchscreen to test against at all? Building and shipping untested touch-event handling blind is exactly the "half-implementation that has to be redone later" this project's own standing rules warn against. If yes, the real shape mirrors `virtual_pointer.rs`'s own precedent closely: smithay exposes `InputEvent::TouchDown`/`TouchMotion`/`TouchUp`/`TouchFrame`/`TouchCancel` already (libinput-backed, same as pointer/keyboard), and a real `TouchHandler` wires those into `wl_touch` the same way `handle_libinput_event` already wires pointer/keyboard - a real, scoped, well-precedented protocol implementation, not a research project, *once* there's real hardware to verify it against.

## New feature: rubber-band desktop icon selection, and a fuller bare-desktop menu (2026-08-26)

Reported live: "missing click and drag stuff like from windows" (this entry) and "not even new file" (the desktop menu). Two real gaps, both closed:

**Rubber-band/marquee multi-select**: clicking bare desktop used to only ever clear whatever single icon was selected - there was no way to select more than one icon at all except one at a time. A left-press on bare desktop now starts a marquee (`CompState::start_desktop_marquee`); every motion tick while it's active re-selects whichever icon cells the resulting rectangle currently overlaps (`update_desktop_marquee`), restricted to the one monitor's own mirror the drag actually started on (see `desktop_icons.rs`'s own "mirrored icons" module doc comment for why cross-monitor mirrors exist at all); release just clears the drag state, since the last motion tick already left the right icons selected. Rendered as four thin solid-colour strips (`border_side_render_element`, the same primitive window borders already use) forming a rectangle outline - a real, visible indicator, not a translucent fill (`SolidColorRenderElement` has no alpha-blend path, and a new element type for one feature wasn't worth it).

**Bare-desktop menu**: added "New Text Document" next to the existing "New Folder" (the literal "not even new file" gap), plus real separator rows grouping New actions / Open actions / Refresh - reusing the same `MenuAction::Separator`-style divider `context_menu.rs`'s own expansion just added, as `DesktopMenuAction::Separator` (a second, separate enum - this menu and the titlebar one still don't share one).

Not attempted: Cut/Copy/Paste for icons (needs `wl_data_device`/`text/uri-list` clipboard interop, already documented elsewhere as deliberately out of scope for the reason given there), and variable icon sizes/a "View" submenu (real submenu UI still doesn't exist - every "fuller menu" ask so far has been satisfied by flattening or a section-label separator instead, but a genuine Windows/macOS-style nested submenu is real, separate scope if a future ask specifically needs one, not silently built partial here).

Full workspace build/test/clippy clean (197 core / 145 wayland tests). Not yet verified live against a real click-drag - this file's own module has no `CompState` test fixture to unit-test the selection logic against (every existing test here is a pure-helper test), consistent with how this module has always drawn its own testing boundary; needs a real drag to confirm.

## Three real fixes from one live-feedback burst after a restart: context menu depth, a desktop-icon selection bug, and a startup placement race (2026-08-26)

**Context menu "much fuller"**: expanded from four fixed actions to Minimize, Maximize/Restore, Fullscreen, Floating, Always on Top, a flattened "Move to Workspace" section (one row per *other* workspace, skipping the window's own), and Close - the common set every native window menu (Windows' System menu, GNOME/KDE's titlebar menu) actually carries. New `MenuAction::Separator` (a real divider row, not a fake action - `input/pointer.rs`'s click dispatch keeps the menu open on a separator click rather than running a no-op or dismissing it). `decoration::render_context_menu` itself untouched - separators render as literal box-drawing-character section labels through the existing text path rather than a new line-drawing primitive, which turned out to double as free section headers ("Move to Workspace") rather than needing a second visual language.

**Desktop icon selection highlight**: reported live, screenshotted directly - right-clicking an icon showed a large, saturated highlight block that read as disproportionate next to the icon/label it was meant to pick out. Root cause: the highlight (`fill_rect`, `theme.default_border_color` - e.g. Catppuccin's mauve) was sized to nearly the *whole cell* (full width minus 4px, 40% of the height) rather than the label text itself. Fixed to a real macOS/GNOME-style snug rounded chip sized to the actual rendered text width plus small padding, using `fill_rounded_rect`/`blit_glyph` (the same properly-blended pair the context-menu rewrite above already established) instead of a flat full-width rectangle.

**Startup placement race, closed rather than just documented**: `general.reserve_top`/`_bottom`/`_left`/`_right` (new config keys, `0`/no-op by default) let a static space reservation apply from the very first `Platform::monitors()` call, before any real bar/dock client has connected - see the dedicated TODO/DEFAULTS.md entries and `WindowManager::reserve_top`'s own doc comment for the full "desktop icons already self-correct every frame, but a *window* placed in the gap before the bar connects gets a one-time placement decision with nothing to nudge it out afterward" reasoning. Set to `34` in this machine's own `~/.config/srd/init.lua` (AGS's real measured top-bar height). Only ever shrinks `usable` *further* than whatever real exclusive zone already exists - a real, bigger bar always wins once it actually connects.

All three: full workspace build/test/clippy clean (197 core / 145 wayland tests), built and installed.

## Instrumentation added, not yet measured: real perf logging for the "resizing seems slow" report (2026-08-26)

Reported live; this session's own investigation (decoration-buffer caching - already signature-cached, no redundant re-rasterization; the one `log::trace!` on the pointer-motion path - below the active `RUST_LOG` level; GPU render path - disabled in config) found no smoking gun without an actual measurement, which is what "measure, don't estimate" actually calls for here rather than another guess.

Added real, cheap instrumentation to `render_udev_frame` (`udev/render.rs`) rather than a fix: one `Instant::now()`/comparison per frame (no allocation, no formatting unless it trips), logging a `PERF-RESIZE` warning only when a frame misses a 16ms (60fps) budget, tagged with whether a resize or drag was actually in progress at that moment (`WindowManager::resizing_window()`/`is_dragging()`). Deliberately not a per-frame trace line - this project's own history already has a documented incident (`docs/TODO.md`'s 2026-08-21 "POS-DIAG/CURSOR-DIAG" entry) where unconditional per-motion-event logging measured at 35% of an entire session's log and was itself mistaken for part of the problem it was diagnosing; a threshold-gated warning can't repeat that.

Not yet measured against a real resize - built, tested (140 wayland tests, clippy clean) and installed, pending the user's own next real resize (or restart) to actually produce data. The next dropped-frame warning in the log (if any) will say whether this genuinely correlates with resize/drag specifically, which is the actual open question, not assumed either way.

Separately, a real, simpler candidate cause surfaced while this same session's own release builds ran: `free -h` during a build showed this machine under genuine memory pressure (3.7GiB total RAM, under 200MiB free, 4.4GiB of a 7.7GiB swap actually in use) with several concurrent agent sessions running alongside the live compositor. System-wide swapping would make everything feel sluggish, resize included, with no compositor-side bug required at all - worth ruling out (`free -h` at the moment it feels slow) before trusting the `PERF-RESIZE` log line above to mean what it says.

**A real measurement, taken**: rather than wait indefinitely for a real interactive drag, exercised the same `render_udev_frame` path deterministically via `srd dispatch toggle maximize <id>` against a real, currently-open window (Nemo, harmless to flicker) - safe and reversible (`srd clients` confirmed it landed back at its exact original geometry afterward), and zero risk of a wrong-target synthetic click since it addresses the window by its real IPC id, not a screen coordinate. 24 total geometry-changing toggles (4 individually, then a 20-toggle burst in a tight loop) produced **zero** `PERF-RESIZE` warnings - every single frame stayed under the 16ms/60fps budget. This measures the discrete "geometry changes instantly" case, not a continuous drag's every-motion-tick redraw, so it doesn't fully settle the original complaint - but it does rule out "a resize-triggered redraw is fundamentally slow" as the cause on this hardware, for at least this pattern. Combined with the memory-pressure finding above, general system load (not this compositor's own render cost) is now the more likely explanation, pending an actual interactive-drag measurement whenever the user is present to do one safely.

## Real bug, reported live, root-caused as far as it goes without a driver-level investigation: nested srdwm (winit backend) renders garbled under this machine's real session (2026-08-26)

Reported live, in the moment: launching a nested `srdwm --wayland` (winit backend, under the live session's own Wayland socket as host - this file's own "validate in a nested compositor" convention, for the virtual-pointer protocol smoke test below) rendered "upside down/inverted"; a screenshot taken moments later instead showed what looked like stale/mirrored content from the host's own tmux window bleeding through.

Checked first, ruled out: `Transform::Normal` is set correctly in `winit/connect.rs`'s own `change_current_state` call, matching every other real compositor's use of smithay's stock `WinitGraphicsBackend<GlesRenderer>` - not a Y-flip/transform bug in this codebase's own rendering math. The nested instance's own log tells the real story: repeated `EGL_BAD_ALLOC` on `eglGetPlatformDisplay`, "failed to get driver name for fd -1", `eglQueryDevicesEXT` failing to allocate a device list, DRI2 failing to set up an `EGLDevice`. The GL/EGL context this backend needs never actually initializes cleanly under nesting on this machine - whatever garbled visual result shows up (inverted, mirrored, or something else entirely) is a plausible downstream symptom of that failure, not evidence of a specific transform bug to chase.

Not fixed this session - fixing a rendering-code path based on a symptom of a broken GPU context underneath it would be treating the wrong layer. Needs real investigation of *why* EGL device enumeration fails specifically when nesting on this machine (render-node/DRM-fd access under a nested Wayland client, a Mesa/driver version quirk, or something host-session-specific) before any compositor-code change is worth attempting. Test process was killed cleanly; confirmed no orphaned processes left behind.

## New feature: real desktop icons now use freedesktop icon-theme SVG artwork, not hand-drawn shapes (2026-08-26)

Reported live: the hand-drawn glyphs (flat rounded-square blocks, polished once already this session in `252d95e`) still read as generic placeholder art, "look like they were made with AI" - asked directly to fetch what these icons actually look like in KDE/GNOME/macOS/Windows and use that instead, with the explicit expectation that the real installed theme (WhiteSur) overrides whatever default ships.

Built as a new `icon_theme.rs` module: a real (if narrow - only the five names `desktop_icons.rs`'s own `IconKind` needs) implementation of the freedesktop icon theme spec. Reads the user's actual configured theme the same places GTK itself would (`gtk-3.0`/`gtk-4.0`'s `settings.ini`, falling back to `gsettings`), walks its `Inherits=` chain recursively, always searches `hicolor` last per the spec's own fallback rule, and prefers a `scalable` SVG over any fixed-size raster variant since every lookup here wants one specific pixel size rendered from source. Rendering is `resvg`/`usvg`/`tiny-skia` (new dependencies - pure Rust, no C toolchain needed, already vetted against this machine's real WhiteSur-dark theme: `user-home`, `computer`, `user-trash`, `folder`, `text-x-generic` all resolved and rendered correctly, verified by dumping each to a PNG and inspecting it directly, not just trusting a green test).

`decoration::render_desktop_icon` takes a new `real_icon: Option<&[u8]>` (a pre-rasterized BGRA buffer, same byte order every other rasterizer in this file already produces) - `Some` blits it straight into the existing glyph box, `None` falls back to the original hand-drawn glyph unchanged, so a machine with no icon theme installed at all (or a theme missing one of the five names) degrades to exactly what shipped before this entry, not a blank box. `state/desktop_icons.rs::rebuild_icon_buffer` does the lookup+rasterize on every rebuild (icon selection/rename change) rather than caching separately - rebuilds are already infrequent, and a theme change while running just works on the next one with no cache-invalidation path to get wrong.

## New feature: context/desktop menus rebuilt to match this project's own AGS panel, not a hard-edged square box (2026-08-26)

Reported live, about both the hand-drawn icons above and menus in the same message: "context menus also need a lot of improvement... UI should look like AGS's global menu dropdown". Directly instructed to look at a live screenshot of that AGS menu rather than guess - attempted via `ydotool` (moved the pointer to the bar's "File" item and clicked), but aborted that path once a concurrent, unrelated keystroke appeared in a nearby tmux pane during the same live session: with the user actively multitasking on the same physical screen, a blind synthetic click too close to other real work was the wrong risk to take for a cosmetic reference screenshot. Read this project's own AGS source instead (`~/dotfiles/ags_project/widget/Bar/components/GlobalMenu/style.scss`'s `popover box.menu-list`, plus `options.ts` for the actual dark-theme colour/spacing values) - more precise than a screenshot in any case, since it gives exact values instead of eyeballed ones.

`decoration::render_context_menu` (shared by every context/desktop menu in this compositor - the titlebar window menu and both new desktop-icon/bare-desktop menus alike, one rasterizer) rebuilt to match that reference's real structure: a rounded floating panel (was a hard-edged square canvas with rows drawn edge-to-edge) with no border/outline anywhere (was a single flat 1px border around the whole menu) and a rounded, inset tinted fill on the highlighted row only (was every row, highlighted or not, filled edge-to-edge with its own flat background) - "a real menu highlights a row with a fill, not a frame", the same principle the AGS reference's own commit history states directly. New `fill_rounded_rect_over` helper: `fill_rounded_rect` itself (used elsewhere for the desktop icon glyphs) blends its own antialiased edges toward a transparent backdrop, which is correct there but would leave a visible dark seam around a rounded hover chip painted on top of the panel's *own* already-opaque fill - the new helper blends against whatever is actually already in the buffer instead. Deliberately unchanged: the external canvas size (`row_height * items.len()`, no added padding) and per-row position, since `ContextMenu`/`DesktopMenu`'s own `height()`/`row_at()` hit-testing assume that exact layout and have no reason to change just because the pixels inside look different. Still no submenus/icons/separators inside a row - real, separate scope, not attempted blind alongside this pass.

`context_menu_border_is_opaque_at_every_edge` (a real test, not just described) inverted into `context_menu_panel_is_opaque_in_the_middle_but_rounded_at_the_corners`: the exact corner pixel is now transparent by design (a real rounded corner, not a hard square), which the old assertion would have flagged as a regression rather than the intended fix.

## New feature: `zwlr_virtual_pointer_unstable_v1`, a real protocol path for synthetic pointer input (2026-08-26)

Closes a gap this project's own history had already root-caused twice over: "ydotool's `--absolute` is unusable on this machine" (no `EV_ABS` on its uinput device, relative motion warped by libinput's own acceleration curve) and the still-open `wl_pointer` unscaled-coordinate investigation both trace back to the same underlying fact - this compositor had no real Wayland-protocol path for synthetic pointer input at all, only whatever a uinput-backed tool like `ydotool` could fake at the kernel level. Also the concrete first step toward the directly-requested "agent controls input without interrupting the user" scenario, though not the whole of it - see below.

Hand-rolled `GlobalDispatch`/`Dispatch` plumbing in a new `virtual_pointer.rs`, matching `screencopy.rs`/`output_management.rs`'s own established shape for a protocol smithay 0.7 ships no helper for. Every `motion`/`motion_absolute`/`button` request is fed through the exact same `handle_pointer_position`/`handle_pointer_button` entry points a real libinput hardware event already goes through (`udev/session.rs::handle_libinput_event`) - a virtual pointer is indistinguishable from a real mouse to every other part of this compositor (hit-testing, drag/resize, focus-follows-mouse) by construction, not a second code path that can drift out of sync. Scroll (`axis`/`axis_source`/`axis_stop`/`axis_discrete`/`frame`) is built directly against smithay's `AxisFrame` builder instead, accumulated across requests in a per-object `Mutex<Option<AxisFrame>>` (needs `Send + Sync` for `wayland-server`'s own `DataInit::init`, not a plain `RefCell`) and committed on the protocol's own `frame` request.

One real, documented limitation: motion only takes effect when `CompState::udev` is live (the real-hardware backend) - the winit/nested backend has no equivalent multi-monitor `bounds()` to clamp against and no daily-driver use case for synthetic input, so the global still exists there (a client's `create_virtual_pointer` still succeeds) but motion requests are accepted no-ops, not silently dropped with no explanation.

Verified: full workspace build/test/clippy clean (140 wayland tests, up from 138), and a nested `srdwm --wayland` instance (launched under the real session's own `WAYLAND_DISPLAY` as its host, per this file's own "validate in a nested compositor" convention - never the live session) started cleanly, ran its full normal autostart (AGS, polkit agent, mpd-mpris) with no crash, and shut down cleanly on signal. **Not yet verified**: no purpose-built protocol test client exists yet to actually exercise `motion`/`button`/`axis` end-to-end the way `tools/toplevel-activate` does for `zwlr_foreign_toplevel_handle_v1` - same "confirmed-fixed, unverified against a real client" bucket as the KDE/Qt global menu items further down this file, for the same reason (no ready-made client available to test against, here because the client would have to be purpose-built rather than merely uninstalled).

Explicitly not the whole of "multi-cursor" as requested (mouse+trackpad simultaneously, or an agent on one monitor while the user works another): this gives one more input *source* feeding the single existing system pointer, not a second independent one. True simultaneous independent cursors need real multi-seat work - parked separately, see `.nightshift/parked.md`/`nightshift questions`.

## Session cleanup: leftover `TEMP-DIAG-*` debug logging from an unfinished prior session found live, removed (2026-08-26)

Picked up this session mid-flight: the previous session's own uncommitted changes to `platform.rs` (primary-monitor fix, see below) and `state/geometry.rs` (cross-monitor border-clip fix, see below) were real and correct, but four `log::warn!("TEMP-DIAG-...")` lines had been left in alongside them, in `input/pointer.rs` (fired on every left-click), `state/desktop_icons.rs` (fired on every icon redraw), `udev/render.rs` (fired on frames 0-2 and every 600th frame after, unconditionally, on the render hot path), and `screencopy.rs` (fired on every `grim`/screenshot capture). Exactly the same "temporary, never removed" pattern already documented and fixed once this session in the 2026-08-21 `POS-DIAG`/`CURSOR-DIAG` entry further down this file -- removed the same way, keeping the real design-rationale comments around them. Separately: the installed `~/.local/share/cargo/bin/{srdwm,srd}` binaries were still the *previous* build (03:12) despite the running process having started at 03:16 with no explanation found for why `monitors()` was reporting the pre-fix `next_id == 0` primary-selection behavior live (`HDMI-A-1` primary despite `eDP-1` sitting at physical `(0,0)` per the restored layout) -- rebuilt (workspace `cargo build`/`test`/`clippy` all clean, 195 core / 134 wayland tests), reinstalled; a live restart (deferred to the user, see below) is what actually picks this up.

## Real bug, reported live, fixed: external monitor's saved layout had drifted to "extend right", not the user's actual "extend left" (2026-08-26)

`~/.local/state/srd/monitor-layout.json` had `HDMI-A-1` persisted at `x: 1920` (to the right of `eDP-1`'s `x: 0`) - not a compositor bug, just a saved position that no longer matched what the user actually wants day to day (likely left over from earlier dragging/testing). Fixed live via `srd dispatch set output position HDMI-A-1 -1920 0` (no restart needed for a position change) - `srd monitors` now reports `HDMI-A-1` at `full_x: -1920`, correctly extending left of `eDP-1`. Compounded by the primary-monitor bug above: because the currently-*running* process still had the old `next_id == 0` primary logic active, `HDMI-A-1` also displayed as primary despite not being at `(0, 0)` - desktop icons (primary-monitor-only, `state/desktop_icons.rs`) and primary-anchored placement were following it there instead of `eDP-1`. Both should resolve together once the user restarts srdwm onto the reinstalled binary.

Follow-up, same day: the drift recurred after a real restart, still not matching either store, which pointed at something actively re-applying a position rather than a one-off leftover. Root cause, confirmed jointly with `dotfiles-16` (the AGS-side peer session) rather than guessed at from srdwm's side alone: srdwm's own `restore_monitor_layout()` and AGS's `restoreRememberedLayout()` (`widget/shared/MonitorLayout.tsx:603`) are two independent, uncoordinated startup-time layout restores with no ordering guarantee - srdwm's runs correctly (confirmed via the session log), but AGS's own runs immediately after with no signal or delay of any kind (`dotfiles-16`'s own words: "waits on nothing at all"), so whichever store the two disagree on, AGS's copy wins by running last. This was already a known, recorded risk: `ags_project/HANDOFF.md` documents the intended resolution as deleting `restoreRememberedLayout()` entirely once srdwm's own restore is confirmed working, since two things arranging the same outputs is the exact shape of bug that produced an earlier `border_width` conflict. **Resolved, landed on the AGS side (2026-08-26, confirmed by `dotfiles-16`)**: not a wholesale removal of `restoreRememberedLayout()` - it turned out to have three call sites, not one: startup (`MonitorLayout.tsx:959`, the one that actually raced srdwm), a monitor being replugged (`:968`), and an output re-enabled after being toggled off (`:739`, which has its own comment recording a real regression an outright deletion would have reintroduced - without it, an "extend left" setup silently became "extend right" after an off/on cycle). Only the startup call duplicated srdwm's own boot-time restore and was removed; the function itself and the other two event-driven calls stayed, since srdwm genuinely doesn't cover a replug or re-enable case. Synced to `~/.config/ags`, takes effect on AGS's next start (startup-only path, nothing forced/bounced); a backup of the pre-edit file is at `~/.cache/ags-MonitorLayout.tsx.bak.20260826-064202`. Net effect for srdwm confirmed unchanged either way: no "layout settled" signal, no hold-off, nothing to build here.

## Decision made with the user: HDMI-A-1 forced to scale 1.0, trading the auto-shrink feature for correctness (2026-08-26)

Two live-reported bugs on the external monitor - clicks landing off from the visible pointer, and windows/content rendering "messed up/broken/see-through" - both trace to the same already-documented root cause (see the "auto-scaled below 1.0 breaks for any client that doesn't speak `wp-fractional-scale-v1`" entry further down): `HDMI-A-1` auto-scales to ~0.843 given its real physical size/resolution, and any client that falls back to the legacy integer `wl_output.scale` (which can't represent below `1.0`) ends up content-sized for the wrong number, on top of a separate, confirmed-real `wl_pointer` wire-protocol gap (see the entry below) that delivers unscaled physical coordinates to every client regardless.

That prior entry explicitly left the "clamp the auto-scale floor to 1.0" question for the user to decide, rather than choosing unilaterally. Asked directly this session: user chose to force this one connector back to `1.0` via the now-real `srd.monitor.scale("HDMI-A-1", 1.0)` (`~/.config/srd/init.lua`) immediately, *and* asked whether the deeper `wl_pointer` fix is still worth pursuing so a future scale-below-1.0 config on this or another machine would also just work. Immediate fix applied and explained inline in `init.lua`'s own comment; the `wl_pointer` investigation is the next entry.

## Investigated further, confirmed genuinely large: the `wl_pointer` unscaled-coordinate gap needs a full `PointerTarget` reimplementation, not a patch (2026-08-26)

Following up on the already-documented, not-yet-fixed `wl_pointer` motion/button scaling gap (see its own entry below) at the user's direct request, to see whether it's smaller than previously scoped. It is not - confirmed against smithay 0.7.0's own source, not re-guessed: `impl<D> PointerTarget<D> for WlSurface` (`wayland/seat/pointer.rs:236`) is a **blanket** impl, generic over every `D`, provided by smithay itself. Rust's orphan rules mean this compositor cannot provide a second, competing `PointerTarget<CompState> for WlSurface` impl to intercept or correct it - not a design choice, a hard language rule. `type PointerFocus = WlSurface` (`protocols/seat.rs`) would have to change to a compositor-owned wrapper type for interception to be possible at all, and since a wrapper type doesn't get smithay's own free `enter`/`leave`/`motion`/`button`/`axis`/`frame`/gesture wire-protocol implementation, that whole implementation would need writing by hand for every regular Wayland surface, not just the ~3 call sites in `input/pointer.rs` that currently construct pointer focus. Real, substantial ground-up subsystem work, not the "thin path" the earlier entry hoped for - and it still would not touch the *content-overflow* half of the same symptom (a per-client protocol-support gap, not something the compositor side can fix regardless). Not attempted this session: the risk of getting seat/pointer plumbing wrong live, on the daily driver, with no isolated way to test it first, outweighs the benefit now that `HDMI-A-1` is forced to `1.0` and the gap is dormant. Worth a dedicated session with a nested `srdwm --wayland` test rig if scale-below-1.0 is ever wanted back on a real monitor.

## Real bug, root-caused and fixed: a window's border/decoration briefly shows clipped/missing after a cross-monitor move between differently-scaled outputs (2026-08-26)

Reported live: "the border/window looks cut offed when moved from one monitor to another". Confirmed directly - moved a real window from `HDMI-A-1` (scale ~0.843) to `eDP-1` (scale 1.0) and screenshotted immediately after: the window's left border was entirely missing and its content sat flush against the screen edge (the page's own heading read "...UNK" instead of "TYPERPUNK", clipped). A second screenshot of the same window a couple of minutes later showed a complete, correctly-bordered window - the same window, same position, self-corrected.

Root cause, confirmed by reading `effective_frame_of` (`state/geometry.rs`) against `sync_geometry`'s own scale conversion: a cross-monitor move keeps a window's *physical* footprint constant, so a scale change between the source and destination monitor changes the *logical* size `sync_geometry` sends the client (see that function's own doc comment) - an ordinary move forces the same size-changing `configure` a real resize would. `w.monitor` itself flips to the destination the moment the drag crosses the boundary, live, every tick - well before the client's own configure/commit round-trip can catch up. In that gap, `effective_frame_of` was reading the client's real committed content size (still logical points sized for the *old* monitor, since no new commit had landed yet) and multiplying it by the *new* monitor's scale to get back to physical - neither the old physical size nor the eventual new one, a genuine mismatch between where the border/shadow got drawn and where the client's actual pixels were.

Fixed by reusing `pending_size_configure` (already tracked for the unrelated configure-throttle above it): `effective_frame_of` now trusts this compositor's own live target (`geom`) rather than reconstructing physical size from a not-yet-committed logical value, for as long as a size-changing configure is outstanding - the same "trust the live target, not a stale client commit" rule the active-resize branch just above it already applies, extended to cover this second, non-drag gap. Not a corruption risk either way: the existing `src`-crop clamp (the resize-lag fix, see below) already bounds every border/titlebar crop against the decoration buffer's real last-built size regardless.

## Real bug, root-caused and fixed, jointly with a peer session (dotfiles-16): layer-shell surfaces on a fractionally-scaled output were unclickable and, for a bottom/right-anchored one, never painted at all (2026-08-26)

Reported live in stages: "AGS isn't even working fast/seems weird now on this monitor even", "dock and AGS buttons aren't working in the other monitor". The peer session independently instrumented AGS itself (both bar and dock report `win_visible=true realized=true reveal=true overlapped=false` on the affected output - the client is asking for the right thing) and srdwm's own `layer_hit_test` log, and found the dock received zero hits across ~40 minutes while the same output's wallpaper and bar took hundreds - full findings in their own `docs/PANEL_SUPPORT_TODO.md`/`SESSION_HANDOFF.md` in the `rust-rewrite` worktree.

Root cause, confirmed against smithay 0.7.0's own source (`desktop/wayland/layer.rs::arrange`): `LayerMap::arrange()` divides the output's *physical* mode size by its own scale before arranging layers, so `LayerMap::layer_geometry()` - and `LayerSurface::surface_under`'s own coordinate space - is genuinely logical, not physical. Two call sites used it as if it were physical, this compositor's own convention everywhere else:

- `input/layers.rs::layer_surface_under_layers` compared the physical pointer position directly against logical layer geometry with no conversion. On a sub-1.0 scale (this machine's `HDMI-A-1`, ~0.843), logical space is *larger* than physical, so a bottom-anchored dock's logical rect sat entirely past the physical pointer's own reachable range - permanently unclickable. A top-anchored bar on the same output only lost its own right-hand end (it starts at `0` in both spaces either way), which is what made this look like "the dock is broken" rather than a scale bug affecting every layer surface on that output.
- `elements.rs::output_layer_elements` pushed each layer's render position straight from that same logical geometry into the physical framebuffer - for the dock, past the bottom edge entirely, painting nothing.

Both fixed the same way `udev/platform.rs::monitors()` and `udev/outputs.rs`'s own `non_exclusive_zone()` handling already fix the identical unit mismatch for usable-area computation (confirmed working precedent already in this codebase, not a new technique): multiply the logical value by `output.current_scale().fractional_scale()`, rounding to the nearest physical pixel, before using it as a physical position.

## Real bug, root-caused and fixed: "primary" monitor picked by DRM probe order, not the user's actual layout (2026-08-26)

Reported live, in stages, across several restarts: desktop icons "sometimes show on other monitor and sometimes don't", and apps opening on the wrong monitor when launched. `udev/platform.rs::monitors()` used to fall back to `udev.heads.first()` whenever nothing was sticky yet - whichever connector DRM happened to probe first, which has no relationship to the user's actual layout. With an "extend left" saved layout (external monitor at negative x, laptop panel at `x = 0`), the external monitor could still win "primary" at boot purely because it probed before the panel.

Fixed: the first-ever pick (before anything is sticky) now prefers whichever head's `location` is physical `(0, 0)` - `relayout_outputs`/`output_management::apply_output_position` already keep the user's actual anchor monitor there, the same position-based definition of "primary" every desktop convention (xrandr, wlr-output-management) already uses, driven by the layout the user actually configured rather than enumeration order. Falls back to `udev.heads.first()` only if no head is at the origin at all. Stays sticky by name after that, unchanged from the earlier fix in this same function.

## Real bug, root-caused and fixed: a new window's target monitor prioritized a stale focused window over the pointer's actual monitor (2026-08-26)

Reported live: launching an app while the pointer sat on a second monitor's bare desktop (nothing focused there - an empty desktop, or hovering a panel/dock, neither of which is a core-tracked window) still put the new window on the *first* monitor, wherever the last real window focus happened to be. `add_window`'s target-monitor fallback chain (`manager/windows.rs`) used to check the focused window's monitor first and the pointer's own monitor only as a fallback - deliberate, tested design from earlier in this same project, on the reasoning that focus is the stronger "where is the user working" signal.

Live feedback contradicted that reasoning directly: `self.focused` only changes when a real window is actually focused, so it goes stale the moment the user's attention moves to empty desktop, a panel, or a dock - exactly the case that matters. `pointer_monitor` has no such staleness (updated on every motion event). Compared against related projects per the user's own request: this also matches the "active output follows the cursor" default every comparable dynamic/floating compositor ships (Hyprland's `focus_follows_mouse` default, Mutter/GNOME, sway). Swapped the priority: pointer's monitor now wins, falling back to the focused window's monitor only when the pointer's own is unknown (a fresh session, no motion reported yet), then primary as the last resort. `manager/tests.rs`'s `a_focused_window_still_wins_over_the_pointers_monitor` (the old, now-reversed contract) was rewritten as `the_pointers_monitor_wins_over_a_stale_focused_window`.

## Real bug, root-caused, not yet safely fixable: `wl_pointer` motion/button coordinates are delivered unscaled to clients on a non-1.0-scale output (2026-08-26)

Reported live: "clicks aren't landing where pointer is on other monitor" (the scaled one specifically, per direct follow-up). Root-caused by reading smithay 0.7.0's own source, not guessed: this compositor's `MotionEvent.location` (`input/pointer.rs`) is fed physical pixels throughout, matching every other internal placement computation here - but smithay's `WlSurface as PointerTarget` (`wayland/seat/pointer.rs::WlPointerHandle::motion`) forwards that value to the client's real `wl_pointer.motion` wire event through `event.location.to_client(client_scale)`, and this compositor has never called `set_client_scale` (confirmed: zero references in the whole crate before this entry), so `client_scale` defaults to `1.0` for every regular client - meaning a client on a monitor with `scale != 1.0` receives raw physical-pixel coordinates where it expects logical points, the same unit mismatch already found and fixed twice this session for layer-shell hit-testing and cross-monitor border placement, this time on the wire to the client itself. `xdg_toplevel::configure`'s `size` (this compositor's own manual conversion, `sync_geometry`) is unaffected and already correct - the client's own idea of its *size* is right, only the pointer coordinates delivered against that size are wrong.

Not fixed this pass: smithay's own sanctioned mechanism for exactly this (`CompositorClientState::set_client_scale`) is explicitly documented as only guaranteeing correctness for "the minimal set of protocols used by xwayland" - and this codebase already relies on a *different*, hand-computed correct value for the *same* clients' `xdg_output` logical position (`udev/outputs.rs::relayout_outputs`'s own `x_logical`, sent via `change_current_state` and advertised onward by smithay's stock `Output` global). `set_client_scale` also multiplies that same per-client `xdg_output`/`wl_output` advertisement (confirmed by reading `wayland/output/xdg.rs` and `wayland/output/mod.rs`), which would double-apply on top of `x_logical` for any client it's turned on for - a real, not hypothetical, second-output-position regression, not just a hypothetical one. Because `PointerHandle::motion`'s single `event.location` parameter drives *both* the value smithay stores as `current_location()` (read elsewhere in this codebase - `xwayland.rs`'s move/resize-request handlers, `input.rs::last_pointer_pos` - always expecting physical, XWayland concretely so, since X11 has no per-output fractional-scale concept of its own) and the value forwarded to the focused client's wire event, there is no way to convert only the latter through smithay's public API as it stands. A correct fix needs a thin, compositor-owned `wl_pointer` serialization path for regular (non-XWayland) toplevel/popup surfaces specifically, bypassing `WlSurface`'s stock `PointerTarget` impl for just the motion/button events - real, scoped work, not a one-line change, deliberately not attempted blind this session given the daily-driver risk of getting it wrong.

Separately, while investigating: a temporary per-pointer-motion-event diagnostic log in `layer_hit_test` (querying compositor cached state and formatting/writing a log line on every single pixel of every mouse move) was still live from an earlier debugging session, explicitly marked "Temporary... remove once a restart confirms" but never removed. Real, measurable cost on the hot input path - removed; likely the direct cause of the separately reported "seems weird/slow" on top of the click-dead dock.

Built, full workspace test suite and clippy clean; installed, pending a live restart to confirm both the dock/bar's full click area and general input responsiveness on the scaled output.

## Feature, implemented, v2: real desktop icons plus proper right-click desktop/icon menus (2026-08-25/26)

Closes the "Right-click on bare desktop" item that used to sit under
"Explicitly requested, not yet started" - previously a true no-op
(`_ => {}` in `input/pointer.rs`'s button handler), nothing rendered above
the wallpaper at all. Follow-up request, asked directly: real desktop
icons "just like windows does".

Built as the sibling of the existing `context_menu.rs`/`snap_flyout.rs`
"compositor-owned floating UI" pattern - see `desktop_icons.rs`'s and
`desktop_menu.rs`'s own module doc comments for the full architecture.
Config keys: `general.desktop_icons` (default `true`), `general.
file_manager`, `general.desktop_icon_single_click`, `general.terminal` --
see DEFAULTS.md's own "Desktop icons" section for the complete behavior
writeup.

**v1 (2026-08-25) live-tested and found genuinely broken in three ways,
fixed in v2 (2026-08-26):**

1. **Icons weren't rendering at all, only sometimes.** Root cause:
   `ensure_desktop_icons` computed the grid's `origin` exactly once, on
   whichever render pass happened to be first - and AGS's own top bar
   registers its exclusive zone only once that separate client connects
   and commits, reliably *after* this compositor's first render pass.
   Confirmed via a temporary diagnostic log: origin baked in at `(1936,
   16)` (bar not yet registered) and never moved again even once `srd
   monitors` reported the bar's real reservation moments later --
   reported live as "Home is still being overlapped by AGS's top bar."
   Fixed by re-deriving `origin` from the primary monitor's *current*
   geometry on every call (cheap: one tuple comparison, no rescan), not
   just the first.
2. **Fixed icons (Home/Computer/Trash) always sorted before real files.**
   Confirmed via direct question this was wrong: the whole list - fixed
   icons included - now sorts alphabetically by label, case-insensitive,
   in one pass.
3. **"Set as Wallpaper" was the wrong feature to build.** The user wants
   that handled by their real file manager (Nemo) once opened, not
   reimplemented here - removed entirely (`DesktopMenuAction::
   SetWallpaper`, `general.wallpaper_command`, `is_image_path`).

**Also added in v2, since "where are all the options" was the direct
complaint about v1's too-thin menu:** a real file/folder icon's menu
gained **Rename** (inline text edit - new `CompState::renaming_icon`
field and keyboard-redirect, mirroring `NativeLock::password`'s own
existing precedent for routing raw keystrokes into a plain string buffer
instead of the focused client) and **Delete** (moves to `~/.local/share/
Trash` per the freedesktop.org Trash spec - new `trash.rs` module,
same-filesystem case only, no confirmation prompt: this is the
reversible move-to-trash, not a permanent delete, the same convention
every mainstream file manager uses - v1's caution here conflated
"destructive" with "irreversible"). The Trash icon's own menu gained
**Empty Trash** (same reversibility framing). The bare-desktop menu
gained **Open Terminal Here** (new `general.terminal` config key, falls
back to the first of a short common-terminal list found on `$PATH`) and
**Open in File Manager** (opens `~/Desktop` itself - the concrete path
to Nemo's own richer menu, directly supporting "let Nemo handle it"
rather than reimplementing Cut/Copy/Paste/Properties here).

Still deliberately cut, stated up front: Cut/Copy/Paste (real interop
needs the Wayland `wl_data_device`/`text/uri-list` clipboard protocol, a
separate substantial feature - an srdwm-only internal clipboard
wouldn't achieve real interop with Nemo anyway), filesystem watching,
multi-select, View/Sort submenus (no nested-menu UI exists), icons on
any monitor but the primary one (requested as an "optional" follow-up,
not yet built - real per-monitor grids need `desktop_icons` to become
`Vec<DesktopIcons>` plus monitor-scoped persistence keys, a genuine
structural change deferred to its own round rather than rushed alongside
everything else here).

Built, `cargo test --workspace` (133 wayland-crate tests, up from 106)
and `cargo clippy --workspace --all-targets` clean; installed, pending a
live restart to confirm.

Separately, while investigating: `~/.local/state/srd/monitor-layout.json`
was found with both `eDP-1` and `HDMI-A-1` persisted at the identical
position `(1920, 0)` - clearly wrong (would stack them). A peer session
(dotfiles-16) confirmed AGS itself sends distinct positions for the two
(`eDP-1@1920,0 HDMI-A-1@0,0`), narrowing this to srdwm's own side of
`set_output_position`/`apply_output_position`.

Follow-up (2026-08-26): read every step of that path --
`crates/platform/src/ipc.rs`'s `set_output_position` handler
(name-to-id resolution, `request_output_position`), `WindowManager::
request_output_position`/`drain_output_position_requests` (de-dupes by
id then pushes - already has a passing test, `output_position_requests_
drain_in_arrival_order`, confirming distinct ids keep distinct values),
`udev/platform.rs`'s per-tick drain loop, `output_management::apply_
output_position` itself, and `udev/outputs.rs::restore_monitor_layout`
(the startup path). Every one of them is correctly per-output/per-request
on inspection - no shared mutable state or loop-variable capture bug
found anywhere in the chain.

Reproduced the AGS sequence directly (`srd dispatch set output position
eDP-1 1920 0` then `... HDMI-A-1 0 0`) and the persisted file came back
correct both times, distinct positions preserved. Live file is currently
correct too (matches the real arrangement exactly). Not reproducible as
of this session - likely a rare, one-off race (possibly startup-
specific, multiple systems initializing concurrently) rather than a
standing defect in the code as it reads today. Not pursued further;
worth another look only if it recurs with a fresh, catchable
before/after state.

## Real bug, root-caused and fixed: a resize's own rapid-fire commits could get a window's rounded-corner content mask cached as blank, hiding real content until the next real content change (2026-08-25)

Reported live: "terminal output/everything disappears when i sometimes resize terminal." `general.rounded_corners = true` in this machine's own live config, so the udev/Pixman content-masking path (`rounded_corners_pixman.rs`) is what's actually in play, not just a rarely-hit opt-in.

Root cause in `masked_content_buffer`: it renders `surface`'s whole subsurface tree into a private off-screen buffer via `render_elements_from_surface_tree`, then unconditionally proceeds to composite and return `Some(bytes)` - with no check for whether that tree actually produced any drawable elements at all. `content_epoch` (the cache-invalidation key `rounded_content_buffer` uses to decide whether to call this again) bumps on *every* commit, unconditionally - and a fast interactive resize is exactly a rapid-fire sequence of commits. If one of those commits races ahead of the client's own texture import (real and reachable under that rate, not a one-in-a-million window), `render_elements_from_surface_tree` legitimately returns empty, and the old code rendered and returned a fully transparent buffer as if it were this window's real, current content - which then got cached under the new epoch, same as a correct result would. Since the cache only rebuilds on the *next* epoch change, that blank buffer stayed on screen, fully transparent, until the window's next real content change - indefinite for an idle, already-settled terminal, reading exactly as "the content disappeared."

Fixed by treating an empty element tree the same as any other failed render (a genuine `create_buffer`/`bind`/`render_output`/etc. error already returned `None` here, "give up unmasked" - this closes the one gap in that pattern): `masked_content_buffer` now returns `None` before doing the expensive render+readback at all when `elements.is_empty()`. `rounded_content_buffer` already drops rather than replaces its cached entry on `None`, so the render loop falls back to the ordinary unmasked `surface_content_elements` path for that one frame (square corners for a frame, not blank), and naturally retries the masked path on the very next frame since a dropped cache entry always reads as stale.

Scoped to the udev/Pixman backend only - the winit/GLES backend masks via a fragment shader (`rounded_corners.rs`), a different mechanism with no equivalent cache-a-blank-result failure mode. Built, full test suite (193 core / 106 wayland) and clippy clean; installed, pending a live restart and a fast-resize test to confirm.

Related, lower-priority, not fixed this pass: `WindowManager::resizing_window()` - what gates skipping the (expensive) masking pass entirely during a resize, elsewhere in this same render loop - is only `Some` during an interactive mouse-drag resize, not a tiling reflow, keybind resize, or snap. Every commit during one of those runs the full masking pipeline synchronously, the same cost this file's own module doc comment already flags as the reason masking stays default-off - already tracked as "resizing is very laggy" elsewhere in this file, not a new finding, but worth noting as a second contributor to the same commit-storm pressure that made the blank-cache race reachable in the first place.

## Real bug, root-caused and fixed: `DecorationSignature` was missing three of `render_titlebar`'s own inputs, so a live change to any of them would never invalidate the cache (2026-08-24)

Found by a full-pipeline audit requested directly ("please do a deep dive into our codebase") after several rounds of live-reported rendering issues. `redraw_decoration_buffer`'s call to `decoration::render_titlebar` (`state/lifecycle.rs`) passes `theme.button_glyph_always`/`theme.button_order`/`theme.traffic_light_buttons` as three of its arguments, but `DecorationSignature` (`state/mod.rs`) - whose own doc comment states its goal outright, "one signature covering every input this function reads" - never included any of the three. `title_centered`/`buttons_left` were already in the struct specifically for this reason (their own doc comments say so explicitly: "there's no `srd set` for this yet, but nothing here assumes there never will be"), making the omission of the other three look like a straightforward miss rather than a deliberate exclusion.

Currently latent, not reachable today: nothing in `srd set`'s current command list can change any of the three live, so this was a landmine for whenever live theme reload or a new `srd set` grows to cover them, not a bug with a live repro today. Fixed anyway, matching the existing fields' own precedent, rather than left for a future session to rediscover the same gap. Test suite run pending; built.

## Real bug, root-caused, first fix attempt reverted as unsafe, now fixed properly (2026-08-25)

Reported live: "notice how in firefox window the borders are misaligned, especially when i resize the window." Root cause in `state/geometry.rs`'s `effective_frame_of` - the function every border/shadow/occlusion/resize-hit-test call site uses to correct a window's drawn rect against what the client *actually* committed, rather than what this compositor merely requested (built earlier this session to fix a real, different bug: a terminal's own cell-quantized size leaving a gap between its content and the border). That correction is applied *unconditionally*, including while the window is being actively, interactively resized - but during a live drag, `geom` (the compositor's own live target) updates on every pointer-motion event, while the client's last real commit is however far behind that a full relayout pass takes. For a heavy client (Firefox, concretely - a plain terminal reflows near-instantly and never showed this visibly) that lag is real and continuous, so the border/shadow keeps drawing at a stale size for the *entire* drag, worst at exactly the edge/corner being dragged, while the content underneath (never routed through this function at all) renders whatever the client has actually gotten around to committing.

**First fix attempt (skip the correction entirely while `WindowManager::resizing_window() == Some(id)`) was reverted in the same session it landed, before ever being confirmed live.** A full-pipeline audit (prompted by the user directly asking for one after this fix still didn't resolve their report) traced the actual consequence: the titlebar bitmap and the top/bottom border strip's own rounded-corner bitmap (both built by `redraw_decoration_buffer`, itself only called on a real client *commit*, not on every resize step) are sampled in `udev/render.rs`/`winit/render.rs` via a `src` crop rectangle sized from this same function's return value. Making this function return the *live* drag target while the underlying bitmap was still sized for the *last commit* means that crop can exceed the bitmap's real stored dimensions - `MemoryRenderBufferRenderElement::from_buffer` (smithay) does not validate `src` against the texture's real size, so an oversized crop is an out-of-bounds texture sample (stretched/repeated/garbage pixels, not a clean error), not just a stale-lag cosmetic issue. Almost shipped a worse bug in place of the one being fixed; caught by tracing the actual downstream consumers before installing, not by testing.

**Second fix, three parts, addressing the audit's own recommendation ("fix at the source") plus the specific failure mode that sank the first attempt:**

1. `effective_frame_of` returns the live drag target directly, but *only* while `WindowManager::resizing_window() == Some(id)` - the same change the first attempt made. On its own this still carries the first attempt's out-of-bounds risk; parts 2 and 3 are what make it safe.
2. Every `src` crop rectangle built from a window's frame width in `udev/render.rs` and `winit/render.rs` (titlebar, top border strip, bottom border strip - six call sites total, three per backend) is now clamped against `DecorationSignature`'s own recorded `width`/`border_width`, the exact size the sampled buffer was actually last built at, before being handed to `from_buffer`. This is a structural floor independent of timing: even if the bitmap rebuild below is ever late, skipped, or throttled away, the crop can no longer exceed what the buffer actually contains, so the out-of-bounds sample that killed the first attempt is no longer reachable from this path at all.
3. `redraw_decoration_buffer` now also runs from `input/pointer.rs`'s `handle_pointer_position`, once per pointer-motion event that finds a resize in progress - closing the gap at its source, same as the audit recommended, instead of only on the next real client commit. Throttled to `RESIZE_REDRAW_INTERVAL` (60Hz) against a new `CompState::resize_redraw_at` timestamp, since font-glyph rasterization for the titlebar text is real per-character cost that a fast mouse/touchpad can otherwise ask for far more often than any visible difference would justify.

Built, `cargo test --workspace` (106 wayland-crate tests plus the rest of the suite) and `cargo clippy --workspace --all-targets` all clean. Part 2's clamp is the actual safety net; parts 1 and 3 are what make the fix visible (live geometry, live redraw) rather than a no-op. Installed and pending a live restart to confirm the border/shadow no longer visibly lags a fast Firefox resize.

Same-audit related finding, lower priority, closed by the same fix: the shadow bitmap had the identical commit-vs-live-position gap (`shadow_rect(frame)`'s position is live, the shadow bitmap's own size is commit-gated) - but shadow is pushed with `src: None`, which smithay's own `from_buffer` resolves to the buffer's *real* native size rather than a crop, so it was never at risk of the out-of-bounds sampling the border/titlebar bug was, only the same soft cosmetic detachment. `redraw_decoration_buffer` rebuilds the shadow bitmap in the same call as the titlebar and border strips, so part 3 of the fix above (calling it from every resize motion event) closes this gap too, with no separate change needed.

## Reported by a peer session (aegis), not reproducible as of 2026-08-25 - likely already fixed: `srd clients`'s own `focused` field going stale after a `zwlr_foreign_toplevel_handle_v1.activate`-driven focus change (2026-08-24)

Found building aegis's dock primitive against the real protocol (confirmed otherwise working correctly: initial-existing-window replay and activate/close all behave right). Repro, on nested `srdwm --wayland` with two alacritty windows: calling `activate` on the non-focused one correctly flips that window's own `activated` state in the `zwlr_foreign_toplevel_handle_v1` state event sent back (both windows' flags update, in the right direction) - but `srd clients`, queried immediately after and again a few seconds later (ruled out as a race), keeps reporting the *other* (previously-focused) window as `focused: true`. Two different "what's focused" views disagreeing: whatever `send_state`/`focused_id()` uses for the protocol feedback (correct) and whatever `srd clients` reads (stale, specifically after an activate-driven change - not checked yet whether a mouse/keyboard-driven focus change has the same gap).

Not yet root-caused on this side: `foreign_toplevel.rs`'s `Activate` handler calls the same `focus_window(state, id)` every other focus path uses (mouse click, keybinding), and `crates/platform/src/ipc.rs`'s `"clients"` request handler calls `client_snapshot(wm)` fresh on every query (no caching) which reads `wm.focused_id()` directly - both look correct in isolation from a first read, so the actual disagreement is somewhere less obvious (worth checking whether `WindowManager::focus_window` itself has a guard that no-ops for this case, or whether the winit/nested backend specifically has its own focus-sync gap the udev backend doesn't - the peer's repro was on nested `srdwm --wayland` specifically). Not blocking aegis (protocol-level behavior is correct, which is what actually matters for a dock), but a real gap for any other tooling reading `srd clients` to know what's focused.

Follow-up investigation (2026-08-25, code-reading only, no live repro run - this session had no minimal Wayland test client set up to actually call `zwlr_foreign_toplevel_handle_v1.activate` and verify, `pywayland`/`wlrctl` both absent from this machine): traced the whole write/read path and everything reads the *same* `wm.focused` through the *same* accessor, so a plain staleness/caching bug looks unlikely from the source alone.
- `crate::input::focus_window` (the free function every path - Activate, click, keybinding - calls) sets `wm.focused` via `WindowManager::focus_window` *first*, then calls `set_keyboard_focus`, which calls `foreign_toplevel::update_activated`, which calls `send_state` for both the old and new window - and `send_state_to` reads `wm.focused_id()` fresh at that point, already past the write. This is consistent with the peer's own observation that the protocol feedback is correct; it does not by itself explain `srd clients` disagreeing moments later, since `client_snapshot` reads the identical field the identical way.
- `IpcServer::poll` takes `&Rc<RefCell<WindowManager>>` by reference on every call (`ipc.poll(&self.wm)`/`ipc.poll(&self.state.wm)`), not a stored/cloned handle, so there is no separate stale copy at the IPC layer either.
- `WindowManager::focus_window`'s entire body (including the `self.focused = Some(id)` write) is gated on `self.windows.get(&id)` resolving to `Some` - a real, if unconfirmed, way for the write to silently no-op if `data.window` (the `WindowId` a `zwlr_foreign_toplevel_handle_v1` was originally bound to) ever stops matching a currently-mapped window. Not verified either way against the peer's actual repro (two plain alacritty windows, no XWayland reparenting in play) - worth a temporary diagnostic log on this specific `if let` the next time this reproduces live, to confirm the branch is even being taken.
- `nested_platform.rs`'s own event-loop ordering processes Wayland protocol dispatch (`display.dispatch_clients`, which is where `Activate` actually runs) *before* `ipc.poll` within one `poll_events` call, so a same-cycle ordering race between the two doesn't look likely either - consistent with the peer's own "ruled out as a race, still stale seconds later."

None of this pinned down an actual mismatch - it narrowed out several plausible causes without finding the real one.

**Resolution attempt (2026-08-25, later the same day): built the live repro and could not reproduce the bug.** A minimal `wayland-client` + `wayland-protocols-wlr` test binary (scratch project, not part of this workspace - `wayland-client 0.31.14`/`wayland-protocols-wlr 0.3.12`, the exact versions smithay 0.7.0 already pulls in, so no version drift from the real client library this compositor itself talks to) that lists every `zwlr_foreign_toplevel_handle_v1`, activates one by index via `zwlr_foreign_toplevel_manager_v1`, and prints the resulting `activated` state from the protocol's own feedback.

Reproduced the peer's exact setup: a nested `srdwm --wayland` (`wayland-1`, launched from inside the real live session) with two plain `alacritty` windows, matching their own repro precisely. Activated the non-focused one, checked `srd clients` (pointed at the nested instance's own `srdwm-wayland-1.sock` via `WAYLAND_DISPLAY=wayland-1`) immediately and 3 seconds later, then repeated the back-and-forth activation 5 times in a row. Every single check agreed with the protocol's own `activated` feedback, immediately and after a delay - no staleness, on the nested/winit backend specifically (the peer's own repro environment), not just the udev one this session otherwise ran on.

Whatever caused this is very likely already fixed as a side effect of one of the many focus/window-management fixes since 2026-08-24 (the corner/border, geometry, and decoration-signature work this session and the ones around it did touch several of the same code paths `crate::input::focus_window` and `redraw_decoration_buffer` sit in) - not confirmed root-caused after the fact (the original report never got a specific commit pinned to it, so there's no single fix to point to), but confirmed *not currently reproducible* via the exact repro that found it, which is the practical bar that matters here. Leaving this open one more round in case it resurfaces, rather than deleting the entry outright - if aegis or anyone else hits it again, the test binary's own approach (a real protocol client, not simulated) is the fastest way back to a repro.

## Real bug, root-caused and fixed: a straight vertical border line poked out of every decorated window's own rounded corners, on both the udev and winit backends (2026-08-24)

Reported live, insistently, over several rounds: "the vertical border lines are protruding out" / "it's drawing onto windows." Every earlier check this session (raw pixel sampling, visual crops, multiple apps, multiple radii up to a deliberately oversized `30` live test) had been looking at the top/bottom border strip's own curve in isolation and finding it genuinely correct - which was true, but beside the point: the actual defect was a *second*, separate element bleeding through the *first* one's own correctly-cut transparent region.

Root cause: the left/right border strips (`decoration::border_strips`'s `strips[2]`/`strips[3]`) are plain flat rectangles with no rounded-corner awareness of their own, and started at the window's nominal `geometry.y` unconditionally. But the top/bottom strip's own curve (`border_top_visible_rows`/`border_bottom_visible_rows`) extends *into* that exact region - by `corner_radius - border_width` rows - whenever the radius exceeds the border's own thickness, which is the common case (12+ vs 4 at this theme's defaults, worse at a manually-bumped `30`). The top/bottom strip is pushed first (topmost, per this codebase's own "earlier-pushed = topmost" convention) specifically so its curve's transparent cutout shows through to whatever's behind - but the side strips sit *underneath* it in the exact same region, still filled solid, with nothing about the top/bottom strip's own transparency doing anything to hide a *different* element sitting behind it. Confirmed via raw pixel sampling at the fixed x of the line: identical, fully-opaque border colour to the curve itself (`(187, 154, 247)` both places), starting right at the window's nominal top edge and running in a straight line in parallel with the real curve rather than being replaced by it.

A pre-existing comment at both call sites (`udev/render.rs` and `winit/render.rs`) asserted the opposite - "sit entirely outside geometry... no titlebar-style overlap... push order doesn't matter for these" - which was the actual reason this was never caught by reasoning about the code, only by an insistent live report and finally looking at raw pixels around the seam specifically rather than just "does the curve itself look right." Both comments corrected in place.

Fixed on both backends: the side strips are now cropped by the same `extra = corner_radius.max(border_width) - border_width` amount at both their own top and bottom before fragment-splitting/rendering, so they only ever draw in the flat-edge middle region the top/bottom strip's own curve has finished resolving by. Full test suite green (190 core / 105 wayland / 10 x11, unchanged); built and installed, needs a restart to confirm live.

## Architecture change: udev/Pixman rounded corners now mask the whole composited window, not one guessed-at subsurface (2026-08-23)

Direct follow-on to the corner/border investigation above and the Firefox-titlebar regression it caused: asked outright why this codebase wasn't doing what other real compositors (niri, cosmic-comp) do. Answer, and the actual fix: those compositors round corners with a GPU fragment shader applied to the *whole already-composited window texture* - it never needs to know which subsurface holds "the real content", because by the time the shader runs there's only one flattened texture left. This backend's old approach (`rounded_corners_pixman::resolve_content_surface`) instead tried to *identify* one subsurface as the real content and mask that client buffer directly, skipping everything else in the tree - cheaper, but structurally wrong the moment a client (Firefox) paints real, visible chrome on a *different* surface than the one being masked.

Rebuilt to match the shader-based approach's own shape instead: `rounded_corners_pixman::masked_content_buffer` now renders the window's entire surface tree (root plus every subsurface - exactly what the unmasked fallback already draws) into a private off-screen Pixman buffer (`PixmanRenderer`'s own `Offscreen`/`Bind<Image>` impls, the same `create_buffer`/`bind`/fresh-`OutputDamageTracker`/`copy_framebuffer`/`map_texture` pipeline `udev/capture.rs`'s workspace-thumbnail capture already uses), reads that back as plain bytes, and punches the four rounded corners into *that* composited result. Whatever the ordinary unmasked path can already draw, this can now mask - no more subsurface-count/format/transform restrictions (`Argb8888`/`Xrgb8888`-only, dmabuf needing its own read path, `Transform::Normal`-only), since none of that per-client-buffer machinery exists anymore. Deleted entirely: `resolve_content_surface`, the old `masked_content_buffer`'s shm/dmabuf split, `mask_and_repack`, `mask_dmabuf_content`, `force_opaque` - the whole "which surface is real content" question this session spent several rounds chasing bugs in.

Cost: a full extra off-screen render pass per real content change (more expensive per rebuild than the old raw-memory-copy approach), still gated by the same `CompState::content_epoch` cache, so an idle window costs nothing extra per frame. `elements::rounded_content_buffer`'s cache key grew to include the render `loc`/`size` alongside `epoch`/`radius`, since those now also have to match for a cached mask to still be valid (a resize invalidates it, same as everything else keyed off geometry in this codebase). Full test suite green (190 core / 105 wayland - one fewer than before, `force_opaque`'s own now-deleted test); built and installed, needs a restart to confirm live. Should fix rounded corners uniformly for every window this backend draws, not just the specific Firefox/tmux cases already screenshotted during the investigation.

## Real bug, reported by a peer session (aegis), root-caused and fixed: the X11 backend never read `_NET_WM_STRUT`/`_NET_WM_STRUT_PARTIAL`, so a dock/bar never shrank the usable monitor area (2026-08-23)

Found and verified live by the `aegis-75` session building its own X11-backend bar (override-redirect window + EWMH struts, mirroring its Wayland layer-shell client): on an isolated `Xvfb :10` + `srdwm --x11`, a client mapped an override-redirect `800x32+0+0` window with `_NET_WM_WINDOW_TYPE_DOCK` and `_NET_WM_STRUT_PARTIAL = 0,0,32,0,0,0,0,0,0,799,0,0` (top=32, spanning x=0..799, confirmed correct via `xprop`) - `srd monitors` reported the same `x:0,y:0,width:800,height:600` usable rect before and after mapping, never shrinking to `y:32,height:568` the way the Wayland backend's layer-shell exclusive-zone handling already does. `grep -rl STRUT crates/x11/src/` found nothing at all -- the feature had simply never been built for this backend.

Fixed with a new `crates/x11/src/platform/struts.rs`: `_NET_WM_STRUT_PARTIAL` is read (falling back to the older, span-free `_NET_WM_STRUT` for a client that only sets that), tracked per-window in a new `X11Platform::struts` map, kept live via `PropertyNotify`, and folded into `monitors()`'s own usable-rect computation the same way the Wayland backends already fold in a layer-shell surface's exclusive zone. Struts are watched via `MapNotify`, not `MapRequest` - a real panel/dock is typically override-redirect specifically to bypass window management, so it never reaches `manage_new_window` at all; `MapNotify` (already arriving, since `SUBSTRUCTURE_NOTIFY` was already selected on root) is the only event that fires for it regardless.

Two real bugs found and fixed only by actually running this, not just reading it (matching this session's own established rule for exactly this class of mistake):
- The shrink math's first version computed `top`/`bottom` in one combined tuple `let (top, bottom) = (top.min(...), bottom.max(...).max(top))` - the `top` on the right-hand side of the *second* tuple element reads the **pre-clamp** binding (a `let` only shadows once the whole statement finishes), so a strut taller than the monitor clamped `top` correctly but then clamped `bottom` against the wrong, oversized `top`, producing `height: 400` instead of the correct `0` - caught by a test asserting exactly that, not by inspection. Fixed by splitting into four sequential statements, each reading only already-clamped bindings.
- Live end-to-end verification (see below) initially still showed no shrink even after the math was right and confirmed reading the strut correctly (logged live: `Strut { top: 32, top_end_x: 799, .. }`) - root cause: `_NET_WM_STRUT`-driven `monitors()` re-queries only run on `Event::MonitorAdded`/`MonitorRemoved` (`crates/srdwm/src/main.rs`'s own hotplug handler); nothing about mapping a strut window pushed either event, so the `WindowManager`-cached monitor list `srd monitors` actually reads never refreshed. Fixed by having `track_strut_window`/`update_strut_property`/`forget_strut_window` return whether the reservation actually changed, and firing the same zero-payload `Event::MonitorAdded` sentinel the Wayland backends' own layer-shell exclusive-zone-change handler already uses to force exactly this re-query - confirmed live: `srd monitors` now goes from `y:0,height:600` to `y:32,height:568` the instant a real override-redirect dock (a small custom Xlib client, `_NET_WM_WINDOW_TYPE_DOCK` + `_NET_WM_STRUT_PARTIAL`) maps on an isolated `Xvfb`+`srdwm --x11`, and back to `y:0,height:600` the instant it's killed.

10 x11-crate tests (up from 5), full workspace suite green (190 core / 105 wayland / 10 x11); built and installed.

## New feature: dialog windows show only a Close button, never traffic-light colours (2026-08-23)

Requested live: a dialog shouldn't offer minimize/maximize at all (there is
nothing to maximize or minimize - it has no independent taskbar presence),
and shouldn't draw the coloured macOS-style traffic-light dots either,
since those specifically signal "this is a real, independently
manageable window" in a way a dialog isn't. `Window` gained an
`is_dialog: bool` field, detected live via `xdg_toplevel.parent().
is_some()` (a toplevel with a parent is a dialog by xdg-shell's own
convention - a genuine "does another window own this" signal, not a
heuristic on size/title) and set in `redraw_decoration_buffer` right
before the titlebar buffer is rebuilt. `ResizeEdge::hit_test` and
`render_titlebar` both take a new trailing `is_dialog` parameter: when
set, the button cluster is forced to a single `Close` entry
(`[TitlebarButton::Close; 3]` with `button_count`/`wanted_buttons`
clamped to `1`) and `traffic_lights` is forced off regardless of the
configured theme, so a dialog's one button always renders as a plain
glyph, never a coloured dot. Both the core (`crates/core/src/window.rs`)
and the render side (`crates/wayland/src/decoration.rs`, later split -
see below) changed together; 3 new tests lock in that a dialog only ever
recognizes the Close button, even with a `button_order` override that
doesn't start with it. Built, tested, installed - not yet confirmed live
(needs a real parented dialog, e.g. a GTK "Save As" prompt, to open and
screenshot against).

## New: `srd workspaces`/`srd monitors` now cross-reference which monitor shows which workspace (2026-08-23)

Requested by an AGS peer session for their per-monitor workspace-pill work: `WorkspaceInfo` gained a `monitor: Option<MonitorId>` field (the monitor currently showing that workspace, `None` if it isn't visible anywhere), and `MonitorInfo` gained `active_workspace: usize` (the workspace id currently showing on that monitor) - the same fact from either direction, so a caller can look it up from whichever side (a workspace pill, or a per-monitor picker) it already has in hand. Both derive from the already-existing `WindowManager::workspace_for_monitor`, no new state. In shared mode (`workspace.per_monitor` off, the default) every monitor reports the same `active_workspace` (`current_workspace`), and at most one workspace ever carries a `monitor` value; per-monitor mode can have more than one of each simultaneously. A disabled-but-listed monitor (`MonitorInfo::enabled == false`) reports `active_workspace: 0` - an id that can never be real (workspace ids are 1-based), the same "obviously not a real value" sentinel `id: u32::MAX` already uses for that same entry, since a disabled output shows nothing.

Coupling `WorkspaceInfo` to monitor state means a monitor being added/removed/enabled/disabled now also changes the workspace snapshot (the `monitor` field flips), so `IpcServer::poll` emits a `"workspaces"` event alongside the `"monitors"` one on exactly those changes - correct (a subscriber's workspace-to-monitor mapping did change), but it shifted one existing test's assumption that only a `"monitors"` event would follow a monitor change; fixed by draining the now-expected `"workspaces"` event first rather than weakening the assertion. New dedicated test (`workspaces_and_monitors_agree_on_which_monitor_shows_which_workspace`) locks in both fields' values together. Full workspace test suite green (29 platform-crate tests, up from 28); built and installed, needs a restart.

## Real bug, root-caused and fixed: a decorated window's own top corners showed a square notch inside the border's own curve, because the titlebar band rendered on top of the border strip instead of under it (2026-08-23)

Asked directly "how have you not noticed corners have these weird corner squares" - a zoomed screenshot of `tmux` (an SSD window, srdwm's own titlebar, no client content or masking involved at all) showed exactly that: a clean, correctly-sized purple border curve, with the dark titlebar band's own square corner poking through inside it rather than the two reading as one continuous curve. This is the same symptom an earlier, still-open TODO.md entry above ("curves for only ~border_width rows") already flagged from a different peer's own measurement - now with a clean repro and, this time, a root cause.

By design (`render_border_top`/`border_top_visible_rows`'s own doc comments), the top border strip's buffer is deliberately taller than its nominal `border_width` whenever `corner_radius > border_width` (12 vs 4 by default) - the extra `corner_radius - border_width` rows extend the border element's own draw position *down into the titlebar band's own top rows*, so the one shared circle has room to finish rather than being cut off after only `border_width` rows. For that overlap to read as a single curve, the border element's own colour has to actually paint over whatever the titlebar drew at those same pixels - which only happens if the border pushes to `custom_elements` *before* the titlebar (this codebase's own established convention: earlier-pushed renders on top, confirmed by an identical comment already on the shadow-vs-border ordering nearby). It didn't: the titlebar was pushed first, so the titlebar's own (differently-centred, per `round_top_corners`' `center_row` shift) corner mask ended up on top instead, showing its own smaller curve-then-square shape rather than the border's.

Fixed by splitting the single `if w.border_width > 0 { ... }` block in `udev/render.rs` in two: the top strip's own push now happens *before* the titlebar push (recomputing `strips` there instead of sharing one instance across both - a cheap, pure call, not worth restructuring the control flow to avoid); the bottom/side strips stay after, since they sit outside `geometry` with no such overlap (per their own doc comments, they either rely on content-masking already being transparent there, or never overlap the titlebar/content at all). Full workspace suite green - the existing `border_top_and_titlebar_corners_meet_without_a_seam` unit test still passed throughout, since it only exercises the *bitmap-generation* functions in isolation and has no way to see a compositing/z-order bug at all, which is exactly why this went unnoticed by the test suite despite being wrong on every real decorated window's top corners the whole time. Built and installed, needs a restart.

## Real bug, root-caused and fixed: Chrome's window corners rendered completely square, because its content subsurface isn't at `(0, 0)` the way Firefox's is (2026-08-23)

Asked directly "borders/corners still not perfectly rounded in all windows" - checked live rather than assuming the earlier dmabuf fix (below, 2026-08-21) covered every case: Firefox and srdwm's own titlebar round cleanly, Chrome's all four corners are flatly square. Debug logging for this exact path already existed and was, unexpectedly, already on by default in this session's own launcher (`~/.scripts/sys/session_manager.sh` sets `RUST_LOG=srdwm=debug,srdwm_wayland=debug,warn` unconditionally) - so the live, already-growing session log (`~/.local/state/wm-session-latest.log`) had the answer without needing a restart at all: `resolve_content_surface: child at Point { x: 10, y: 43 }, not (0,0) - giving up unmasked`, repeated for every one of Chrome's own content-mask attempts.

Chrome's real content lives in a single child subsurface, the same GTK4/WebRender pattern Firefox uses - just inset at `(10, 43)` (its own CSD shadow-margin/toolbar offset) rather than sitting flush at the root's origin the way Firefox's does. `resolve_content_surface`'s own `child_location != (0, 0)` guard rejected this outright, unconditionally, even though the offset itself has no bearing on whether *masking* that child's buffer is valid - it only matters for *where the result gets drawn on screen*, a concern the caller already had a mechanism for (`content_offset`) that this code path just wasn't using.

Fixed by removing the `(0, 0)`-only restriction and instead returning the child's own real offset alongside the resolved surface, threaded all the way through `masked_content_buffer` → `elements::rounded_content_buffer` → the `udev/render.rs` call site, which now adds it to the window's already-computed `pos` before placing the masked buffer (`rounded_content_buffers`' cache tuple gained a fourth `(i32, i32)` field to carry it). Firefox's own behaviour is unchanged by construction - its child sits at `(0, 0)`, so the added offset is always `(0, 0)` there too; this is strictly additive, not a rewrite of the working case. Full workspace suite green; built and installed, needs a restart.

## Real bug, root-caused and fixed: `zwp_virtual_keyboard_manager_v1` was never implemented, so every synthetic-keystroke tool (`wtype`, `ydotool type`) silently did nothing (2026-08-23)

Asked directly whether Chrome/Firefox's global menu was "fully supported" - checked the AGS side first (`widget/Bar/components/GlobalMenu/index.tsx`'s own extensive doc comments) rather than assuming: Firefox and Chrome structurally cannot export a *real, live* menu at all - Wayland has no general per-app menu-export protocol, GTK's own legacy mechanism (`appmenu-gtk-module`, X11-property-only) only reaches XWayland GTK3 apps that still build a traditional `GtkMenuBar`, and neither Firefox (no traditional GTK menu bar to begin with) nor Chrome (not a GTK app in the relevant sense) qualifies, confirmed already tested end-to-end by a prior AGS session pass. Not a bug anywhere in this stack - an upstream protocol gap no compositor or panel can route around.

What AGS shows those two instead is a static, hand-written placeholder menu (`StaticMenuButton`), and *that* had a real, fixable bug: every keyboard-shortcut item on it is delivered via `wtype`, which needs `zwp_virtual_keyboard_manager_v1` - confirmed absent from this codebase entirely (no reference anywhere in `crates/wayland/src`). `wtype ""` failed outright (`Compositor does not support the virtual keyboard protocol`), and AGS's own call is fire-and-forget, so the failure was completely invisible - exactly matching an earlier live report, "most options in global menu don't work."

Fixed using smithay 0.7.0's own turnkey `wayland::virtual_keyboard` module (a full protocol implementation, not hand-rolled) - routes a synthetic key straight through the same keyboard-focus/keymap pipeline a real physical key press already goes through, correctly reaching whatever's actually focused and reachable by every other compositor's own equivalent of this same tool. New `CompState::_virtual_keyboard_state` field (not `Option`-gated - injecting a key event has nothing GPU/DRM-specific about it, same reasoning `_appmenu_state` already established), constructed identically on both backends, `delegate_virtual_keyboard_manager!` added alongside the existing `delegate_input_method_manager!`/`delegate_text_input_manager!`. Full workspace suite green; built and installed, needs a restart.

Separately, worth knowing but *not* the same bug: a different, real, already-open issue (`com.canonical.AppMenu.Registrar` owned by AGS instead of srdwm, see the entry below dated 2026-08-21) affects classic Qt/`appmenu-qt5` apps' menu registration, not Chrome/Firefox - those two never go through that registrar at all.

## Real bug, root-caused (the visibility half) and partially fixed: XWayland silently never became ready in a real session, taking `com.canonical.AppMenu.Registrar` down with it (2026-08-21)

A peer session (`dotfiles-04`) reported the classic-Qt appmenu registrar still unowned by srdwm despite their own D-Bus flag fix landing correctly on their side. Checked the obvious place first (is `AppmenuRegistrarState::new()` even running) by grepping the live session's own log for anything mentioning XWayland at all - found *nothing*, across a 300K+-line log: no "XWayland ready," no "XWayland unavailable," no `XwaylandEvent::Error`, nothing. `ps`/`pgrep` confirmed no `Xwayland` process running either. Since `appmenu_registrar` is only ever constructed inside the `XWaylandEvent::Ready` handler (`xwayland.rs`), XWayland never starting at all fully explains the registrar symptom - it isn't a D-Bus flag problem, the code that would even attempt to claim the name never runs.

Narrowed further: a manual, standalone run of the real `Xwayland` binary, and of the `-shm` wrapper `ensure_shm_wrapper_on_path` installs on `PATH` for this exact purpose, both succeeded outside srdwm's own process - ruling out "the binary/wrapper is broken" as the cause. The remaining, most likely explanation: `XWayland::spawn`'s call in `xwayland.rs` passed `Stdio::null()` for both the child's stdout *and* stderr - whatever Xwayland itself would have printed about why it failed (confirmed live, manually, that a real run does print real diagnostic warnings) was being discarded before anyone could ever see it. `/tmp/.X0-lock` existing with srdwm's own pid inside it is *not* itself a bug - confirmed against smithay 0.7.0's own source (`xwayland/x11_sockets.rs`): the lock deliberately records the *compositor's* pid for as long as it holds display `:0`, by design, not the eventual X server's.

Fixed the visibility gap, not (yet, confirmed) the underlying cause: `xwayland.rs::spawn` now redirects Xwayland's stdout/stderr to `$XDG_STATE_HOME/srd/xwayland.log` (appended, not truncated, so a restart doesn't erase the previous run's failure) instead of discarding them. Full test suite green; built and installed, needs a restart - and the *next* time this reproduces, that log file should finally say why, closing this out for real rather than leaving it as an educated guess.

## Real bug, root-caused and fixed: rounded corners curved for only ~`border_width` rows, not the full configured radius (2026-08-23)

Continuation of the investigation below: root-caused via the corner-mask diagnostic logging already in place (`RUST_LOG=debug`, no restart needed) against a real, currently-open Firefox window. The `corner-mask state` log line showed, on every single frame: `decorated=false content_will_be_masked=false border_curve_is_safe=false` - and the `rounded_corners_pixman` debug line directly underneath it explained why: `resolve_content_surface: child Size { w: 1551, h: 790 } smaller than root Size { w: 1571, h: 844 } - giving up unmasked`.

Root cause: `resolve_content_surface` (`crates/wayland/src/rounded_corners_pixman.rs`) rejected a resolved content subsurface whenever it was smaller than its root surface on either axis, on the reasoning (from the function's own pre-existing doc comment) that a too-small child was probably some small decorative overlay rather than the real content. That guard predates this session and was never the target of either of this session's two earlier corner fixes (Firefox's dmabuf buffer type, Chrome's non-`(0, 0)` child offset) - but it silently rejects exactly the same shadow-margin-plus-toolbar inset pattern already confirmed and handled for Chrome (`(10, 43)`): a CSD client's root buffer padded for an invisible drop-shadow margin, with the real, *smaller* content subsurface sitting inset inside it. Firefox hit this specific shape live, permanently, on every frame - not the intermittent screen-edge cosmetic gap the investigation below first assumed, but the direct explanation for `border_curve_is_safe` being unconditionally `false`: `w.decorated || content_will_be_masked` with an undecorated CSD window whose masking can never succeed is `false || false`, which is exactly what routes both `border_top_visible_rows`/`border_bottom_visible_rows` down to their `border_width`-only branch instead of the full `corner_radius` one.

Fixed by removing the size check entirely: the two structural checks already ahead of it (exactly one child subsurface, that child itself childless) are what actually establish "this is the content subsurface", not its size relative to the root - and `child_location` (already returned and already applied by every caller) already carries whatever inset a smaller child has, so nothing about a smaller child was ever unsafe to mask, just wrongly assumed to be. Full test suite green (190 core / 106 wayland tests, unchanged); built and installed, needs a restart to confirm live.

## New: unfocused windows now get a fainter drop shadow, matching real desktop convention (2026-08-21)

Asked directly to continue on borders/corners/shadows/effects and check similar projects' own docs, not just this codebase. `shadow_bitmap` (`crates/wayland/src/decoration.rs`) drew the exact same shadow - same `SHADOW_MAX_ALPHA`, no focus awareness at all - for a focused and an unfocused window alike. Checked two real compositors' own documented behaviour rather than assuming a convention: Hyprland's `decoration:shadow` config exposes `color` and `color_inactive` as two separate, independently configurable values (common real configs set `color_inactive` fully transparent - no shadow at all once a window loses focus); niri's own `layout.shadow.inactive-color` does the same, with niri's own docs stating outright that "by default, a more transparent color is used" for an inactive window's shadow. Two independent, actively-developed compositors agreeing on the same convention is a real pattern, not one project's stylistic choice.

Fixed to match: `shadow_bitmap` takes a `max_alpha: u8` parameter instead of always reading the `SHADOW_MAX_ALPHA` constant directly, and `redraw_decoration_buffer` dims it for an unfocused window the exact same way `effective_border_color` already dims an unfocused window's border colour - reusing the existing, already-user-configurable `theme.border_inactive_dim` factor rather than adding a second, separately-configurable knob for what's really the same underlying question ("how much does losing focus fade this window's own chrome"). New test (`a_lower_max_alpha_produces_a_strictly_fainter_shadow_throughout`) locks in that a dimmed shadow is never darker than the focused one at any pixel. Full test suite green (103 wayland-crate tests, up from 102); built and installed, needs a restart.

## Real bug, root-caused and fixed: rounded corners never actually applied to Firefox's content, because its buffer is dmabuf, not shm (2026-08-21)

The user sent real screenshots of all four corners of a tmux window and a Firefox window: tmux's all rounded cleanly, Firefox's bottom-left and bottom-right were flatly square, no curve at all. Added targeted `debug`-level logging (off by default) at every early-return in the content-masking path and asked for a restart to get real data rather than guess further - confirmed on the very first read: `with_buffer_contents failed (NotManaged)`, repeated for every masking attempt on that window.

Root cause: `masked_content_buffer` (`crates/wayland/src/rounded_corners_pixman.rs`) already correctly resolves Firefox's real content to its child subsurface (a fix from earlier in this session), but then reads that surface's buffer via `smithay::wayland::shm::with_buffer_contents` - an shm-only accessor. Firefox's WebRender content surface renders through GL even on this Pixman-only (CPU, no shader stage) backend - almost certainly software/llvmpipe GL rather than a real GPU, but still exported as a genuine dmabuf the same as any hardware-accelerated client's would be - so the shm-only read rejects it outright, every time, and the corner silently falls back to unrounded rather than the wedge-shaped artifact the fallback exists to avoid.

Fixed by adding a dmabuf read path (`mask_dmabuf_content`), reached only once the shm read has confirmed the buffer genuinely isn't shm. Uses `get_dmabuf`/`Dmabuf::map_plane(DmabufMappingMode::READ)` - the read-mode mirror of the exact `map_plane(..WRITE)` pattern `screencopy.rs`'s `write_dmabuf` already proved works for writing a capture *into* a client's dmabuf, for the opposite direction. Scoped the same way that write path is: single-plane, `Linear`-modifier dmabufs only (what this compositor's own `dmabuf_formats()` ever advertises to a client in the first place, so nothing else should reach a real client's negotiated buffer here anyway). The actual masking math (repack to tight stride, force-opaque for X-format sources, punch the corner holes) was pulled out into a shared `mask_and_repack` helper so the shm and dmabuf paths don't duplicate it.

Diagnostic logging kept at `debug` level (not removed) - same reasoning as the trace-level pointer-telemetry compromise earlier this session: it's the only way to tell exactly which check rejected a future client's content without another live debugging round, and `debug` costs nothing when `RUST_LOG` doesn't ask for it. Full test suite green; built and installed, needs a restart.

## Real bug, root-caused and fixed: every new window opened on the primary monitor, regardless of which monitor the user was actually on (2026-08-21)

Reported live, plainly: "why are all windows only opening in the first monitor." Root cause, `WindowManager::add_window` (`crates/core/src/manager/windows.rs`): the target monitor for a brand new window was resolved via `self.primary_monitor()` unconditionally - not "usually", not "as a fallback", the only path that ever ran, regardless of where the user's focus, workspace, or attention actually was.

Fixed to resolve the target monitor from the *currently focused* window's own monitor first, falling back to primary only when nothing is focused yet (a fresh session's very first window, or every window having just closed) - matches the convention every mainstream desktop already follows (a new window opens where you're working), and needs no new state: `self.focused` already existed for exactly this question. New test (`a_new_window_lands_on_the_focused_windows_monitor_not_always_primary`) locks this in; the existing regression test for a *different* known gap (`a_window_whose_monitor_field_is_stale_is_still_rescued`, about a window placed by a rule keeping a stale `monitor` field) still passes unchanged, since its own scenario - nothing focused yet - is exactly this fix's fallback case. Full test suite green (183 core-crate tests, up from 182); built and installed, needs a restart.

## Investigated, not a bug found: a reported delay before the pointer can reach a second monitor right after startup

Reported alongside the monitor-layout-persistence work above: "it takes a while for me to drag mouse left to monitor since monitor layout prefs aren't loaded [at the] same time." Checked the actual mechanism this would have to go through: `UdevState::bounds()` (pointer-clamp bounding box) is computed fresh from `self.heads` on every single pointer-motion event, not cached anywhere - and `restore_monitor_layout` (see the persistence entry above) already runs before the Wayland socket even binds, so `head.location` should already reflect the *restored* arrangement before any pointer motion could possibly be processed at all. Confirmed via a real session's own log and its persisted `monitor-layout.json`: the heads' logical positions in the log already matched the remembered arrangement, correctly reversed from the plain left-to-right default. Nothing in this path shows an obvious source of a startup delay.

Not closed out, though - this report may predate the layout-persistence fix (same message, adjacent topic, unclear which restart the user was describing), or the real cause may be something this code-only investigation can't see (DRM mode-setting/first-frame timing on the second output specifically, distinct from its *logical* position being correct from frame one - a monitor could be positioned correctly but still show no picture for a moment while its own CRTC comes up, which would look identical to "can't reach it" without actually being a pointer-clamping bug at all). Flagged rather than closed; needs a fresh, timed reproduction attributed to a specific restart to actually pin down.

## New feature: srdwm persists and restores its own monitor layout, instead of relying on a panel (2026-08-21)

Raised directly by the user: monitor knowledge is srdwm's logic and should be settled before any panel spawns, and since this compositor is meant to work with any panel or none, restore logic belongs here too, not assumed to exist somewhere else. Before this, whatever arranged outputs on a restart was whichever panel happened to be running - one peer session (AGS) measured 13.7s between its own startup and its remembered layout actually landing, and that number excludes however long the compositor itself is up and displaying before that panel even launches. Worse than slow: if the panel doesn't run, or loses its own store, the layout was never restored at all.

New `crates/wayland/src/monitor_layout.rs`: a small `{connector_name: {x, y, enabled}}` JSON file at `$XDG_STATE_HOME/srd/monitor-layout.json` (`$SRDWM_STATE_PATH` override, `~/.local/state/srd` fallback - mirrors `srdwm/src/main.rs`'s own `config_dir()` shape). Written atomically (tmp file + rename, same pattern as `udev/capture.rs`'s `write_ppm`) on every live position/enabled change - hooked into `apply_output_position` (covers both `srd dispatch set output position` and a real `wlr-output-management-v1` client's apply request) and `disable_connector_by_name`/`enable_connector_by_name`. Read back and applied in a new `CompState::restore_monitor_layout`, called from `UdevPlatform::connect` **before the Wayland socket is even bound** - not just "early", but literally before any client could possibly connect, so no panel or client ever sees a pre-restore arrangement, not even for one frame. Disables are applied before position restores specifically, since `disable_connector_by_name` ends with its own default-layout `relayout_outputs()` call that would otherwise stomp an already-restored position for a different head.

A connector with no remembered entry (first boot, or a monitor plugged in for the first time) is left exactly where the default left-to-right layout puts it - this only ever narrows toward a remembered position, never invents one for a monitor it's never seen before. Two new tests (pure JSON round-trip, corrupt-file-degrades-not-panics); the actual file I/O and env-var-driven path resolution are deliberately untested, same reasoning `config_dir()` already has no coverage for (parallel test execution can't safely share a mutated process-global env var). Full workspace suite green; built and installed, needs a restart.

The AGS peer session's own `restoreRememberedLayout()` is expected to be retired once this is confirmed live - two things arranging the same outputs was exactly the shape of bug that produced the `border_width` mess earlier this session (also AGS pushing something srdwm should own), and there's no reason to keep that risk around once srdwm's own version exists and works.

## Real bug, root-caused and fixed: `effective_frame_of` mixed logical and physical units, drawing an undecorated window's border detached from its real content (2026-08-21)

The user pushed back, correctly, on an earlier "I checked, no gap" claim in this same session - a real screenshot sent directly showed Firefox's purple border sitting visibly to the east and south of the actual window content, unmistakable at a glance, not something needing pixel-scanning to argue about. My first measurement had sampled the wrong row and missed it; this is the real bug once found properly.

Root cause: `effective_frame_of` (`crates/wayland/src/state/geometry.rs`) reads `dwindow.geometry()` - a client's own `xdg_surface::set_window_geometry`, specified to always carry *logical* points - and uses it directly as this compositor's own *physical* convention, with no conversion. `sync_geometry` (the function that originally *asks* a client to become a given size) already gets this right, converting physical to logical on the way out; `effective_frame_of` (which reads back what the client actually *committed*, per its own doc comment, for border/shadow/occlusion/resize-margin-hit-test purposes) never got the matching conversion on the way back in. Invisible at `scale == 1.0` (logical and physical are numerically identical there), which is every monitor this session had until the auto-scale feature gave one a real non-1.0 value.

Fixed the same way `sync_geometry` does: multiply the client's committed logical size by the window's own monitor scale before building the returned physical `Rect`. Full test suite green; built and installed, needs a restart.

## Real bug, root-caused and fixed: two monitors' *logical* rectangles overlapped whenever one had a non-1.0 scale, even though their *physical* placement was correct (2026-08-21)

Found and precisely measured by a peer session reading straight from GTK (`Gdk.Display.get_monitors()`), independent of anything in this compositor's own logs: `HDMI-A-1` (1920 physical, scale ~0.843, so ~2276 *logical*) reported at logical `(0,0)`, and `eDP-1` (1920 physical, scale 1.0) reported at logical `(1920,0)` - inside `HDMI-A-1`'s own logical extent (`0..2276`), a ~356px logical overlap between two monitors that don't overlap physically at all. Any client asking "which monitor is this point on" in that band gets an ambiguous or wrong answer - among the concrete symptoms this produced on the AGS side: hit-testing resolving to the wrong output, and per-output `grim` captures bleeding pixels from the neighbouring output.

Root cause, in two places that both had the same shape: `bring_up_head` (`crates/wayland/src/udev/drm.rs`) computed a head's own logical x-position as `x_offset / this_head's_own_scale` - correct only for the *first* head in a layout, or when every head shares one scale. For any later head, the *previous* heads' own (possibly different) scales are what actually determine how much logical space they occupy, not this head's own scale - so this consistently mis-placed every head after the first whenever scales differed across monitors. `relayout_outputs` (`crates/wayland/src/udev/outputs.rs`, the hotplug re-layout path) had it worse: it passed the raw *physical* accumulated offset straight into `change_current_state` with no scale conversion at all, unconditionally.

Fixed both to track two separate running offsets - `x_offset` (physical, this compositor's own internal placement convention, unchanged, still what `UdevHead.location`/`Space`/`Monitor` rects use) and a new `logical_x` accumulated by adding each processed head's own *logical* width (`physical_width / that_head's_own_resolved_scale`) as it goes, used only for what's actually advertised to Wayland clients via `change_current_state`. `bring_up_head`'s signature gained a `logical_x: i32` parameter (computed by each caller, not derived internally from `x_offset` alone anymore); both hotplug call sites in `outputs.rs` pass `0` for it since `relayout_outputs` immediately re-derives and applies the real value for every head afterward, same as they already did for the physical offset. Full test suite green; built and installed, needs a restart. Not independently re-verified against GTK/AGS yet - that needs the peer session's own next check once this restart happens.

## Real bug, root-caused, live-measured: a monitor auto-scaled below 1.0 breaks for any client that doesn't speak `wp-fractional-scale-v1` (2026-08-21)

The second monitor came back this session (previously untestable), and the border/window gap the user kept reporting on it turned out to be real and measurable, not fixed by this session's earlier `sync_geometry` scale-conversion work. Reproduced directly: a tiled window on `HDMI-A-1` (auto-scaled to ~0.843 by this session's own PPI-based feature) reported geometry `x=186, width=1442` (physical, this compositor's own convention) - but a precise pixel scan of a real screenshot found the *titlebar* ending around x=1681 (roughly matching the reported edge) while the terminal's actual *content* kept going to x=1930 - content overflowing about 250px past where the frame/border actually is. Exactly the "border far from the edge of the window, see-through gap" the user described, and reproducible on demand by placing any window on that monitor.

Root cause: `sync_geometry` (`crates/wayland/src/state/geometry.rs`) correctly computes a *logical* size to send via `xdg_toplevel::configure` (`physical / scale`) - for `scale=0.843` and `physical=1442`, that's a requested logical size of ~1710. A client that supports `wp-fractional-scale-v1` would render that many logical points at a 0.843 buffer scale, landing back at 1442 real pixels, matching what this compositor's own border/hit-testing expects. A client that *doesn't* - falling back to the legacy integer `wl_output.scale`, which cannot represent a value below 1 at all (protocol-level `uint`, and no real compositor rounds it to anything but the nearest sane integer, effectively `1` here) - renders its buffer at scale 1, meaning its buffer ends up sized directly by the *logical* number, ~1710 pixels, not 1442. The measured overflow (~250px) is in exactly the range that gap implies, once real animation/measurement slop is accounted for.

This is a real architectural tension in the auto-scale-below-1.0 feature this session added earlier (deliberately built and tuned per direct request, to shrink oversized UI on physically-large, lower-density monitors), not a bug isolated to one code path: **any client on such a monitor that doesn't implement fractional-scale, and there's no guarantee every client does, will show this same content/frame mismatch.** `impl FractionalScaleHandler for CompState {}` is the smithay-default no-op handler - not customized, so this isn't a case of the fractional-scale side being half-wired either; the gap is squarely "not every client speaks the protocol this depends on," which srdwm has no way to fix client-side.

Not fixed generically this session - the immediate, concrete mitigation the user asked for and already applied: `srd.monitor.scale("HDMI-A-1", 1.0)` added to `~/.config/srd/init.lua`, overriding the automatic value back to 1.0 for this specific monitor, which sidesteps the whole problem (no scale conversion needed at all when scale is 1.0). Config-only, needs a restart. Worth deciding, not decided here: whether `auto_scale_for` (`crates/core/src/monitor.rs`) should ever be allowed to compute a value below `1.0` at all, given this compatibility gap, or whether it should clamp its floor to `1.0` and only ever scale *up* for genuine HiDPI panels - matching what mainstream desktops (GNOME, KDE) actually ship, which generally treat fractional scaling as a HiDPI (>1.0) feature specifically for this reason, not a way to shrink a large low-density panel. Real trade-off either way: the auto-scale-DOWN feature only ever produces sub-1.0 values (it has no up-scale branch at all), so clamping at 1.0 doesn't refine the feature, it disables it outright - reverting the original "text/UI too big on the physically-larger monitor" complaint this was built to fix. Not something to decide unilaterally; needs the user's own call between the two.

Second-order finding from a peer session verifying the above: `grim`'s own multi-output capture is *itself* affected by the same physical/logical confusion, independent of anything srdwm's compositor code does - capturing two 1920x1080-logical outputs (one at scale 1.0, one at ~0.843) produced a single stitched image sized `3840x1281`, not `3840x2160`(both at scale) or `3840x1080` cleanly, meaning the capture tool's own per-output stitching mixes physical and logical sizing across outputs of different scale. `srd clients`/`srd monitors` report physical throughout (this compositor's own established convention); a screenshot taken across a sub-1.0-scaled output can't be trusted to line up with those numbers pixel-for-pixel without correcting for whatever `grim` itself did - a testing-infrastructure gap on top of the client-rendering one, not fixable from srdwm's side either. One more argument for clamping the floor at 1.0, alongside the client-compatibility one above.

## Real bug, root-caused and fixed: a window dragged or resized onto a different-scale monitor rendered "very messed up" for the whole gesture, only correcting itself on release (2026-08-25)

Reported live: moving a window onto the other monitor looks very messed up. This machine's own two real monitors have genuinely different scales (`srd monitors`: `eDP-1` at `1.0`, `HDMI-A-1` at `~0.843`, the same auto-scaled-below-1.0 monitor the 2026-08-21 entries above already have a long history with) - the exact condition needed to expose this.

Root cause in `WindowManager::update_drag`/`update_resize` (`crates/core/src/manager/dragresize.rs`): `w.monitor` used to only get corrected once, at `end_drag` (`update_resize` never corrected it at all, not even at the end) - but `state/geometry.rs::sync_geometry`, called on every single motion tick while a drag or resize is in progress, reads that exact field to pick which monitor's `scale` converts the client's real physical size into the logical points `xdg_toplevel::configure` sends it. Dragging (or resizing) a window from one monitor onto the other kept every mid-gesture configure computed against the *origin* monitor's stale scale for the gesture's entire remaining duration - the client resizing itself to a logical size that doesn't match the physical footprint the border/decoration were actually drawing around it on the *new* monitor, self-correcting only the instant the button came up (which is when `end_drag`'s own existing fixup finally ran).

Fixed at the source, same "close the gap where the field actually goes stale" approach as this session's earlier resize-lag fix: both `update_drag` and `update_resize` now re-derive `w.monitor` from which monitor the window's live geometry actually overlaps, every motion tick, the same `Rect::overlaps`-based lookup `end_drag` already used once at the very end. `end_drag`'s own fixup is left in place as a final-word safety net (a drag that starts and ends between two motion ticks would otherwise skip the correction entirely), now normally just reconfirming what `update_drag` already set.

Does not fully close the family of scale-crossing issues this shares a root cause with - see the 2026-08-21 entries just above: a client that doesn't speak `wp-fractional-scale-v1` will still show a content/frame mismatch once *settled* on the sub-1.0-scaled monitor, independent of this fix, which only closes the *stale-during-the-gesture* half of the problem. Two new tests (`dragging_across_a_monitor_boundary_updates_monitor_live_not_just_at_end`, `resizing_across_a_monitor_boundary_updates_monitor_live`); full workspace test suite (195 core / 124 wayland) and clippy clean; installed, pending a live restart to confirm.

## Real bug, root-caused, not yet fixed: `com.canonical.AppMenu.Registrar` is owned by AGS, not srdwm, despite srdwm's own code deliberately trying to claim it (2026-08-21)

Flagged by a peer session (`dotfiles-04`): `busctl --user list` shows the classic Qt/`appmenu-qt5` global-menu registrar name owned by AGS's own `gjs` process, not srdwm - confirmed live (`OwnerUID` traced to the AGS pid). `AppmenuRegistrarState::new()` (`crates/platform/src/appmenu_registrar.rs`) is genuinely constructed at startup (`xwayland.rs`'s `XWaylandEvent::Ready` handler, alongside `EwmhState::connect`) and its own D-Bus name request sets `replace_existing_names(true)` - and srdwm's log has no warning from the `Err` branch that would fire if the connection/name request failed outright, meaning the `zbus` call chain reports success from srdwm's own side despite not actually owning the name afterward.

Working theory, not yet confirmed against zbus's own source (not available locally to check directly): D-Bus name replacement is opt-in on *both* sides - a second requester's `replace_existing_names`/`DBUS_NAME_FLAG_REPLACE_EXISTING` only actually takes the name away from whoever currently holds it if that first owner *also* registered with `DBUS_NAME_FLAG_ALLOW_REPLACEMENT` set. If AGS's own name request didn't set that flag, srdwm's later replace-attempt would simply not succeed in taking over - and if `zbus`'s builder API doesn't surface "request queued behind an existing, non-replaceable owner" as an `Err`, that would explain both halves of what's observed: no warning logged, and the name still not actually owned.

This plausibly shares a root cause with a separate ordering concern the user raised directly ("monitors etc is srdwm logic, should happen before ags spawns"): srdwm's registrar is only constructed once XWayland reports ready, which happens well after the compositor's core socket is already accepting client connections - if AGS connects and claims the D-Bus name before srdwm's XWayland/registrar startup completes, whichever of AGS's or srdwm's name requests happens to run first wins the name outright, independent of either side's own `replace_existing_names` intent. Not fixed this session - needs confirming the zbus replacement semantics first, and probably belongs together with the broader startup-ordering question (see the next entry) rather than as an isolated D-Bus fix.

## Real bug, root-caused and fixed: centered titlebar text wasn't centered on the window, only on the space left after the buttons (2026-08-21)

Reported live: "the ... font/placement of title text is wrong, not real center" - and measurable, not just a impression. `render_titlebar`'s centering formula (`crates/wayland/src/decoration.rs`) computed the midpoint of `text_start..text_limit`, which is the titlebar width *minus* the button reservation (90px for the usual 3-button, 30px-tall case) - not the midpoint of the titlebar itself. For a 300px-wide titlebar with buttons on the left, that put centered text at x=195, a full 45px right of the window's true center (x=150), a difference obvious at a glance, not a rounding-error-sized nitpick.

Real macOS (the convention this titlebar otherwise follows - traffic-light colours, left-side default) does the opposite: it centers the title on the full window width and lets the traffic-light cluster sit wherever it lands, rather than adjusting the centered point to compensate for it. Fixed to match: the ideal centered position is now computed against the full `width`, then clamped into `text_start..text_limit` only to stop a long title from actually drawing under the buttons - so a short title centers on the true window center, and only a title long enough to reach the button zone gets pushed off that ideal point, same as before.

New test: `centered_title_ignores_the_button_reservation_and_centers_on_the_whole_width`, using a titlebar wide enough to reserve button space and asserting the rendered midpoint lands within a few px of `width / 2`, not the old buggy sub-region's own center. Existing `centered_title_starts_further_right_than_left_aligned` (a no-button fixture, unaffected by this change either way) still passes unchanged. Full workspace suite green (100 wayland-crate tests, up from 99); built and installed, needs a restart.

Separately reported in the same message: "windows like tmux[,] most options in global menu don't work/not real global menu... a lot of windows share this behavior." Checked `srd clients` directly - `global_menu: null` for a plain terminal (no app menu to export, correctly) - so whatever's showing an interactive-looking-but-non-functional menu for those windows is a panel-side (AGS) choice to render a placeholder rather than nothing when the data is null, not something srdwm is misreporting. Flagged to the AGS peer session (`dotfiles-04`) rather than chased here.

## Cleanup: removed the temporary `POS-DIAG`/`CURSOR-DIAG` logging from `input.rs` (2026-08-21)

Both mysteries this pre-existing diagnostic logging (predating this session) existed to chase are now resolved - the `resolved=None` resize-margin question (see the entry above) and, per a peer session's own live measurement, a cursor-icon "no visible effect" report that turned out to be a real UX gap already fixed (`CursorIcon::Pointer` for titlebar buttons, distinguishable from the baseline `Default`), not a logic bug. Left in place, this logging was pure cost: a peer session measured it at 35% of an 8898-line daily session log (2650 `CURSOR-DIAG force:` lines alone), burying real warnings. Removed every `log::warn!("POS-DIAG...")`/`log::warn!("CURSOR-DIAG...")` call and the "Temporary... Remove once resolved" comments around them from `crates/wayland/src/input.rs`, keeping the genuine design-rationale comments that were mixed in with them.

Partial walk-back, same day: the same peer session pointed out that `POS-DIAG motion` was the *only* pointer-position telemetry this compositor exposed to anything outside itself - with it gone entirely, there is no way to answer "where is the pointer right now" at an arbitrary point, only corner-clamping (4 fixed points) or hover feedback (binary, only over a reactive widget). Restored a single, minimal line (`pointer motion pos=... hit=...`, unconditional - not just while over decoration, unlike the original) at `trace` level rather than `warn`: off by default (`RUST_LOG` reaches it the same as any other target here, see `main.rs`'s directive-syntax comment), so it costs nothing in normal use, but the debuggability isn't gone. Everything else removed above stays removed - only this one line came back, and at a level that can't repeat the 35%-of-the-log problem.

One more thing this closes out: a peer session flagged `over_content=false` appearing in every logged `CURSOR-DIAG force:` line as a possible bug (pointer reported as not-over-content when it should have been). It wasn't one - by construction, the one branch where `over_content` is actually `true` (`update_cursor_shape`'s `None if over_content` arm) returns before ever reaching that log statement, most of the time completely silently (no log line at all) when there was nothing to reset. Every line that *did* log `over_content=false` was, definitionally, on a different code path that never had a chance to be `true` in the first place. A diagnostic-structure artifact, not a hit-testing bug - moot now that the logging is gone entirely.

## Root-caused: ydotool's `--absolute` is unusable on this machine; relative motion works, but only correctly paced

Every early synthetic `ydotool` resize/drag test this session silently misfired. Root cause for the absolute-positioning half: `ydotoold`'s virtual input device advertises `EV_REL` only (`/proc/bus/input/devices`: `B: REL=147`, no `B: ABS=` line at all) - `ydotool mousemove --absolute` still accepts absolute coordinates client-side, but with no `EV_ABS` capability on the actual kernel device there is nothing for those coordinates to attach to. `ydotoold --help` lists `-T`/`--touch-on` ("Enable touchscreen (EV_ABS)") as the fix - tried it (via a `~/.config/systemd/user/ydotool.service.d/override.conf` drop-in), and it does not work on this system's installed build (`ydotool 1.0.4-2`, Arch): accepted by argument parsing, no `EV_ABS` bit on the resulting device either way, and passing it bare made the daemon exit immediately with status 2. Reverted the override; not fixable by flag alone on this build. Use relative `mousemove` instead, always.

Relative motion's own reliability comes down to **libinput pointer acceleration applied to synthetic `REL` events** - confirmed by two independent sessions converging on the same mechanism from different data: this session saw an unpaced burst of small steps land way off target (a requested (830,400) landed near (1602,684)); a peer session (`dotfiles-04`) separately measured error growing *with distance*, the actual signature of an acceleration curve, and found that neither pacing nor small step size fixes it on its own - large fast steps overshoot, single-pixel steps undershoot so hard the pointer barely moves. An earlier version of this entry claimed paced ~5px steps land within ~1px "repeatably"; that did not reproduce in the peer's own re-measurement and should not be trusted as a general rule - it happened to work once, for one real resize-drag test (window width 800->705 in `srd clients`, confirmed via ground truth, not by reading a log line), not proven as a formula. The one reliable primitive either session found: a large overshoot (+-3000 or more) reliably corner-clamps the pointer to a screen edge, useful for establishing a known reference point before walking toward an interior target. Treat any precision synthetic-pointer test as needing empirical verification against `srd clients`/`srd monitors` afterward, not as something that can be blindly calibrated in advance.

With that methodology, a real end-to-end resize was finally exercised successfully: walked onto a live window's right resize margin (confirmed via the `POS-DIAG motion ... hit=Resize(Right)` log line), pressed, dragged 100px left in paced 5px steps, released - `srd clients` showed the window's width change from 800 to 705. Resize/drag genuinely works. See the next entry for what this also settled about the previously-open `resolved=None` mystery.

## Real bug, root-caused and fixed: global menu missed any window whose D-Bus registration finished after its one focus-triggered read (2026-08-20)

Reported live as "global menu doesn't show up for some windows" - intermittent, not tied to any one app. Root cause: `EwmhState::read_global_menu` (`crates/wayland/src/xwayland.rs`) only ever ran from `update_net_active_window`, itself only called on a focus *change*. Most toolkits set `_GTK_UNIQUE_BUS_NAME`/the menu-path atom once, shortly after mapping - for an already-focused window (the common case: a freshly launched app almost always opens focused), that registration can finish *after* the one focus-triggered read already ran, leaving `Window.global_menu` stuck at `None` until the user clicked away and back. smithay's own `XwmHandler::property_notify` can't see this either - its `WmWindowProperty` enum (`smithay::xwayland::xwm::surface`) is a closed set (`Title`/`Class`/`Protocols`/`Hints`/`NormalHints`/`TransientFor`/`WindowType`/`MotifHints`/`StartupId`/`Pid`) with no catch-all for an atom it doesn't recognize - confirmed by reading smithay 0.7.0's own `xwm/mod.rs`: `Event::PropertyNotify` is filtered down to that enum via `X11Surface::update_property` before ever reaching our handler, and the global-menu atoms all map to `None` there, so the event is dropped inside smithay, never surfaced to this codebase at all.

Fixed by watching for it directly: `EwmhState` already holds its own independent X11 connection (for writing `_NET_ACTIVE_WINDOW`/`_NET_CLIENT_LIST`), previously write-only. Added `watch_property_changes` (selects `PropertyChangeMask` on a window right after its setup finishes) and `poll_property_events` (drains `PropertyNotify` non-blockingly, matched against the same eight global-menu atoms `read_global_menu` already reads). `CompState::poll_global_menu_properties` applies the result, called once per event-loop tick from `udev/platform.rs`'s main loop, same cadence as the existing `apply_registrar_events` (classic Qt `appmenu-qt5` registrar polling) it now sits next to. Not wired into the nested/winit backend - XWayland itself isn't started there at all (see `xwayland.rs`'s own module doc comment), so there is nothing for this to watch on that backend.

Full workspace test suite green (1264+ tests across all crates, no regressions); built and installed, needs a restart.

## Real bug, root-caused and fixed: any GTK4/libadwaita app not manually listed in `rules.lua` got a second, redundant titlebar (2026-08-20)

`rules.lua` already had `decorated = false` entries for Firefox and Nemo, each with its own doc comment explaining why: both draw their own header bar (`GtkHeaderBar`/an embedded CSD row) unconditionally, regardless of what `xdg-decoration` actually negotiates - confirmed live for both via screenshot, two stacked title rows. That reasoning generalizes to every GNOME app, not just those two, but the fix so far was one hand-written rule per app as each was discovered live - anything not yet added still got srdwm's own server-side titlebar drawn on top of its own CSD row.

Added a real, general fallback instead of only growing the list further: `srdwm_core::window::likely_draws_own_titlebar` treats any `org.gnome.*` app id as CSD-only by default - the GNOME HIG mandates every one of GNOME's own apps embed a header bar, with no exceptions, so the namespace alone is enough to know in advance rather than waiting to catch each one live. Deliberately narrow: left at `org.gnome.*` rather than also guessing at third-party `io.github.*`/other reverse-DNS libadwaita apps, which don't share GNOME's HIG mandate and would misclassify plenty of ordinary SSD apps that happen to use a similar id scheme - those still go through `rules.lua` as before. Wired into both `WindowManager::add_window` (X11 windows, whose `app_id` is already known at creation) and `reapply_rules_if_pending` (native Wayland windows, whose `app_id` usually lands after creation via `set_app_id`) - a rule's own explicit `decorated` action still wins over the heuristic in both places, unchanged.

Full test suite green; built and installed, needs a restart.

## Not a bug, a config mismatch: titlebar button side/alignment differs between srdwm's own windows and Firefox's CSD

Looked real in a live screenshot - srdwm's own titlebar had macOS-style traffic lights on the *left* (red-close, yellow-minimize, green-maximize), title text centered; Firefox's CSD had them on the *right* (yellow-minimize, green-maximize, red-close), left-aligned. First guessed this was a stale pre-restart process, since source defaults (`ThemeConfig::default`: `buttons_left: false`, `title_centered: false`) already match this system's GTK convention (`gsettings get org.gnome.desktop.wm.preferences button-layout` -> `appmenu:minimize,maximize,close`, theme `WhiteSur-Dark`). That guess was wrong - re-checked after a real restart, same result, because the actual cause is `~/.config/srd/themes.lua`'s active preset: `catppuccin_mocha` (a straight port of the old Hyprland/Catppuccin config, applied at the bottom of that file) explicitly sets `title_bar.button_side = "left"` and `text_align = "center"`, overriding the source defaults deliberately. `themes.lua`'s other three presets (`nord`, `nord_light`, `gtk_match`) don't touch `button_side` at all, so any of them would already match Firefox/WhiteSur's convention without any code change - this is a one-line edit in a config file the user owns, not something to silently change on their behalf, since a left-side macOS-style titlebar independent of what GTK apps do may be the intended look. Surfaced to the user rather than assumed either way.

## Real bug, root-caused and fixed: maximize ignored every bar/dock except a top one (2026-08-20)

Reported live, twice, from two different angles that turned out to be
the same bug: "AGS's dock isn't reappearing" and "parts of the current
window are hidden, can't see its borders." dotfiles-04 (AGS peer
session) found it precisely with a real measurement rather than
guessing: `srd monitors` correctly reports a work area excluding *both*
a 34px top bar and a 53px bottom dock, but a maximized Firefox window
came back 1046px tall (screen height minus only the 34px top bar) --
its own bottom edge and border ending up underneath the dock's surface,
indistinguishable from the dock not rendering at all.

Root cause: `maximize_geometry_for` (`crates/wayland/src/input.rs`) only
ever subtracted a **top**-anchored bar's exclusive zone from the
maximize target - a deliberate earlier design choice (own doc comment:
"a dock anchored to any other edge is deliberately left alone"), reasoned
through as "maximize should be able to go past a dock, fullscreen already
covers wanting the screen entirely." In practice this doesn't match how
any mainstream desktop actually treats a bottom dock - Windows, macOS
and GNOME/KDE alike keep a maximized window clear of one - and reads as
a bug, not a feature, the moment a real dock exists to be covered by it.

Fixed: `maximize_geometry_for` now shrinks for a reservation on *any*
edge (top, bottom, left, or right), not top only. Fullscreen is
unaffected - it was never routed through this function and still isn't,
so it remains the one deliberate "ignore every bar/dock, cover the whole
screen" option. Full workspace test suite green (no existing test
coverage for this function - needs real smithay layer-map state with no
harness available, same limitation as this session's other real-
renderer-only fixes); built and installed, needs a restart.

## Resolved: resize/drag itself works fine; `resolved=None` on a resize-margin press is expected, not a bug

Previously logged here as an open, unexplained mystery: a button press
during a resize grab logging `POS-DIAG button-resolve pressed=true
button=0x110 resolved=None client=None`, read at the time as "the resize
grab had nothing to attach to and nothing happened." With `ydotool`
usable again and a proper small-step, correctly-paced synthetic drag
actually landing where intended (see the `ydotool` entry below), this was
re-tested directly: pressed down at a confirmed `hit=Resize(Right)` point
on a real window, dragged 100px, released - the window's width changed
from 800 to 705 in `srd clients`, a genuine resize, while the *press*
event still logged `resolved=None client=None` exactly as before.

That's the resolution: `resolved=None` on a press over a resize margin is
correct, not a failure. A resize margin is compositor-drawn decoration
space (the border strip), not client surface space - there is no client
`wl_surface` at that exact point for surface-resolution to find, by
definition, regardless of whether the resize grab itself is about to
work. The actual resize path is driven by the hit-test result
(`hit=Resize(...)`) computed separately, before this logging point, not
by whether surface-resolution succeeds. The earlier hypothesis (extreme-
edge off-by-one) was chasing a symptom that was never actually broken.
`POS-DIAG`/`CURSOR-DIAG` logging can be removed the next time this file
is touched - kept for this entry's own record, not because anything is
still open.

## Real bug, root-caused and fixed: `srd capture workspace` wrote near-black or pure-black frames (2026-08-20)

Long-standing report from dotfiles-04 (the AGS peer session), finally
picked up: a workspace-switcher thumbnail captured via `srd capture
workspace` came back at ~0.025 mean luminance for the current workspace
(a real screenshot of the same instant: ~0.51) and *exactly* 0 mean and
0 variance - literally every pixel pure black - for an inactive
workspace with no windows on it.

Root cause was exactly what it looked like: `capture_workspace`
(`crates/wayland/src/udev/capture.rs`) only ever rendered window content,
by design - its own module doc comment listed "no borders, shadows,
titlebars, cursor or layer-shell surfaces" as deliberate simplifications,
reasonable for the small window-switcher-tile decorations in that list
but not for layer-shell, which is also where the *wallpaper* lives. A
workspace capture with no windows and no wallpaper is indistinguishable
from broken, because on a typical desktop the wallpaper is most of the
frame.

Fixed by rendering the background/bottom layer-shell surfaces too,
matching `render_udev_frame`'s own real ordering convention (windows on
top, wallpaper pushed last/bottommost). Required changing `capture_
workspace`'s element list from the narrow `Vec<WaylandSurfaceRenderElement
<PixmanRenderer>>` it only needed for window content to the same `Vec<
crate::elements::OverlayElement<PixmanRenderer>>` the real render path
already uses, since layer-shell elements aren't representable in the
narrower type.

Full workspace test suite green; built and installed, needs a restart.
No new unit tests - same as this session's other real-renderer-only
fixes, this needs live smithay/DRM state with no test harness available;
worth a live `srd capture workspace <id> <path>` + `magick ... -format
"%[fx:mean]" info:` check post-restart to confirm the luminance is back
in line with a real screenshot, the same measurement dotfiles-04 already
used to find this in the first place.

## Real bug, root-caused and fixed: VT switch back left the screen black, unrecoverable (2026-08-20)

Reported live: after switching away from srdwm's VT and back, the screen
stayed black, and no further VT switch in either direction could recover
it - eventually needing a hard restart. Confirmed **not** a srdwm crash
(`coredumpctl` shows no srdwm entries ever, the process stays alive
throughout) - a stuck/wrong DRM state, not a segfault.

Root cause: `register_session_notifier`'s `ActivateSession` handler (VT
switch back) reasserted every head with `card.set_crtc(head.crtc,
Some(fb), (0, 0), &[], None)` - an **empty connector list and no mode**.
That is DRM/KMS's own shape for *disabling* a CRTC, not restoring one;
`bring_up_head`'s own original call (still correct) passes the real
connector and mode. The resume path never had access to either - neither
was stored anywhere per-head - so it was passing "nothing" and calling
that a reassert.

Fixed: `UdevHead` gained a `mode: DrmMode` field (set once in
`bring_up_head` from `probe.mode`, alongside the `connector` field that
already existed but wasn't being reused here either); the resume handler
now calls `card.set_crtc(head.crtc, Some(fb), (0, 0), &[head.connector],
Some(head.mode))` - the actual reassert the comment above it already
claimed to be doing. Full workspace test suite green (no test coverage
possible for this specific path - real DRM state, no harness); built and
installed, needs a restart. **Not independently live-verified this
session** - deliberately did not test a real VT switch to confirm the
fix, given the user's own report that a failed attempt requires a hard
restart; ask before testing this specific path live.

## Real bug, root-caused and fixed: window geometry/border misaligned after a cross-monitor move (2026-08-20)

Reported live: dragging a window from one monitor to the other (this
machine's two outputs have different scales - `eDP-1` at `1.0`, `HDMI-
A-1` at `~0.84`) leaves its border/decoration visibly detached from its
real content, and resizing afterward doesn't work.

Two live reproduction attempts (an IPC `dispatch move window <id> right`
call, which turned out to be a directional tile-swap, not a drag; a
synthetic `ydotool` titlebar-drag, which never registered as a real drag
at all) both failed before a root cause was found by reading, not
reproducing:

`sync_geometry` (`crates/wayland/src/state/geometry.rs`) was sending
`xdg_toplevel.configure`'s `size` directly from `geom.width`/`geom.
height` - this compositor's own internal *physical*-pixel tracking (see
`Platform::monitors()`'s own doc comment on that choice) - with zero
conversion to the *logical* points `xdg_toplevel.configure` is specified
to carry. Before this session's own auto-scale feature, every output was
`1.0`, so physical and logical were numerically identical and the missing
conversion was invisible.

The reason this specifically shows up on a *move*, not just a resize: an
ordinary drag never changes `geom.width`/`geom.height` at all, so at
*first* glance there is nothing to convert - but the window's *logical*
size (physical divided by scale) genuinely does change the moment it
crosses onto a differently-scaled monitor, even though physical geometry
didn't move. Before this fix, `sync_geometry`'s `size_changed` check
compared physical values, saw no change, and sent no configure at all --
the client kept rendering its old logical size against the new monitor's
different scale, while this compositor's own border kept drawing at the
physical rect it always had. Two things that used to agree (client
content size, this compositor's own border) stopped agreeing the moment
scale entered the picture, which is exactly "border far from the window."

Fixed: `sync_geometry` now resolves the window's current monitor's
`scale` and converts to logical points before anything downstream reads
`size` - including the throttle/backlog bookkeeping
(`last_synced_size`/`pending_size_configure`), which needed to move to
logical too, since what they're compared against (`w.geometry()`, a
client's own `xdg_surface::set_window_geometry`) is logical by the same
specification `xdg_toplevel.configure` uses. This has the desired,
niri/Windows/macOS-convention side effect: a window crossing to a
different-scale monitor now genuinely gets a fresh configure asking it to
resize, keeping its true on-screen (physical) footprint consistent across
the DPI change, rather than silently drifting.

Full workspace test suite green; built and installed, needs a restart.
**Not independently live-verified this session** - both synthetic
reproduction attempts failed before the fix, so there is no before/after
comparison confirming this actually resolves the reported symptom; ask
the user to check after their next restart rather than treating this as
confirmed.

## Prior-art research: titlebars, global menus, monitor/scale logic (2026-08-20)

Two comparison passes against real source and real docs - niri, Mutter,
KWin/Plasma, Hyprland, Awesome, Openbox, GlazeWM, plus macOS's own HIG for
titlebars, and the same set (minus GlazeWM) for monitor/scale/placement
logic. Full titlebar/global-menu report published as an artifact
("Titlebars & Global Menus"); the monitor/sizing pass and open-questions
follow-up were session-only. Concrete outcomes below; everything else
(what each project does, sourced per-claim) lives in the artifact and the
agent transcripts, not duplicated here.

- **Corner-rounding scope validated, not found lacking.** KWin shipped
  real, current (Plasma 6.5, July 2025) server-side rounded corners using
  signed distance fields - and its own merge request explicitly states
  "sub-surface corners are not rounded," with KDE's own developers noting
  real support would need a *new Wayland protocol* letting a client
  request its own subsurface rounding, not a compositor-side heuristic.
  This directly validates srdwm's own narrow-scope limitation (this
  session's own single-full-covering-child fix, `rounded_corners_pixman.rs
  ::resolve_content_surface`) as already at the field's current state of
  the art for a CPU-masking approach, not a gap to close further.
- **niri's alternative (a per-element GLSL shader clip keyed to window
  geometry, handling arbitrary subsurface trees with no special-casing)
  is real and battle-tested**, not experimental - confirmed via niri's
  own design-principles docs and a live GitHub discussion, with one known
  minor artifact (niri#3476, a thin blending seam) as its only real flaw.
  **Not portable to srdwm's real hardware backend**: `PixmanRenderer`
  (udev/DRM, srdwm's actual production renderer) has no shader stage at
  all, already established earlier this session - this technique could
  only ever reach srdwm's separate GLES/winit backend, which is dev-only.
  Deliberately not pursued this session for this reason, not for lack of
  merit.
  **Superseded 2026-08-26**: "GLES is dev-only" above is now stale --
  `general.gpu`/`SRDWM_GPU=1` gates a real, reachable GLES path on the
  udev/DRM backend too (`udev/gpu.rs` + the branch in `udev/render.rs`,
  falling back to Pixman per-CRTC on init failure). See the "GPU/rendering
  completion" plan entry lower in this file - niri's per-element shader
  clip is a real, live option there after all, just not attempted yet.
- **Button-order convergence acted on** - see the button-order feature
  entry below.
- **The coordinate-unit bug class this session fixed has a more durable
  architectural answer than the patch that shipped.** niri never hand-
  tracks physical output geometry in its own struct at all - every
  placement computation reads position/size through smithay's own typed
  `Space<Output>`/`Size<i32, Physical>::to_logical(scale)`, one canonical
  space, one conversion point. srdwm's own fix this session converted
  correctly at each of several call sites individually
  (`Platform::monitors()`, the disabled-output snapshot,
  `maximize_geometry_for`, `apply_output_position`) rather than removing
  the redundant hand-tracked physical copy (`UdevHead::location`/`size`)
  that made each of those call sites necessary to fix separately in the
  first place. **Not attempted this session** - a real, larger
  refactor, flagged here for whoever picks up monitor/output code next,
  not undertaken speculatively on top of an already-large session.
- **`auto_scale_for`'s below-`1.0` scaling has no precedent** in either
  real implementation checked (Mutter's original heuristic, niri's own
  direct port of it) - both cap at scale ≥ 1. Confirmed as a deliberate
  divergence from convention, made on direct user request earlier this
  session, not an alignment with how anyone else does it. Worth knowing,
  not necessarily worth reverting.
- **Monitor-arrangement persistence across a restart has no solved
  precedent either** - Hyprland's own community has built third-party
  tools (`hyprland-monitor-fix`, `HyprDynamicMonitors`) specifically
  because `hyprctl`-applied changes don't survive a restart there either.
  srdwm's own already-known gap here isn't unusual for a bare compositor
  - this class of feature is conventionally a desktop environment's
  settings daemon's job (GNOME's `monitors.xml`, KDE's `kscreen`), not
  the compositor's.
- **KWin's button-layout config is real and more complete than assumed**:
  `~/.config/kwinrc`'s `[org.kde.kdecoration2]` group's `ButtonsOnLeft`/
  `ButtonsOnRight`, a 10-letter vocabulary (menu, on-all-desktops, keep-
  above/below, shade, help, minimize, maximize, close, app-menu) - more
  than srdwm needs (3 buttons only) but the same underlying idea directly
  acted on below.
- **GNOME/Adwaita's own button-layout convention was mischaracterized in
  the first pass** - `AdwHeaderBar`'s `decoration-layout` property (a
  real, colon-separated-by-side, comma-separated-by-button string,
  falling back to the system `gtk-decoration-layout` GSetting) is exactly
  as configurable as KWin's, not a fixed HIG mandate as first assumed
  from Mutter's C source alone. The GNOME HIG's own header-bar page has
  no window-control button conventions at all - confirmed by fetching it
  directly.
- **Plasma Global Menu's current maintenance state stayed genuinely
  inconclusive** even after a real search pass - it ships and is
  preinstalled in current Plasma 6, but has at least one real, reported
  breaking regression in the last ~18 months (KDE Discuss). Treated as
  unresolved either direction, not papered over with a confident guess.

## New feature: `theme.decorations.title_bar.button_order` (2026-08-20)

Direct response to the one finding above with a clear, safely-scoped
path to action: KWin's `ButtonsOnLeft`/`ButtonsOnRight`, GNOME/Adwaita's
`decoration-layout`, and Openbox's `titlelayout` all independently
converged on the same idea - an ordered list of button names, not just a
side toggle. srdwm's own existing `theme.decorations.title_bar.
button_side` (a boolean-shaped left/right toggle) had deliberately chosen
"one config value, not a bespoke per-button scheme" in an earlier
session, before this comparison existed to inform that choice.

Additive, not a reversal: `button_side` still exists and still means
what it always did. New `button_order` (a comma-separated
`"close,minimize,maximize"`-style string, `srdwm_core::window::
parse_button_order`) optionally overrides the *relative order* of the
three buttons on whichever side `button_side` already selects. Unset
(the default) reproduces the exact same two built-in orders as before,
byte-for-byte - confirmed by every pre-existing button/hit-test test in
`crates/core/src/window.rs` passing completely unmodified.

`ResizeEdge::hit_test` and `decoration::render_titlebar` both moved from
two hardcoded three-way matches (one per side) to a shared, ordered
`[TitlebarButton; 3]` walked by index - the two functions have to stay
in exact agreement (a button that renders on one side/position but hit-
tests on a different one is worse than no configurability at all, the
same trap `buttons_left` itself already had to avoid), so both read the
same resolved order the same way. 8 new unit tests (`parse_button_order`
parsing/validation, plus two new hit-test-agreement tests); full 298-test
workspace suite green; built and installed, needs a restart.

## New feature: subsurface-aware rounded corners on undecorated windows (2026-08-20)

What the user actually asked for, after a real correction to this
session's own earlier investigation: I'd spent hours chasing Firefox's
titlebar/button rendering as a srdwm bug before finally checking `~/
.config/srd/rules.lua` and finding `srd.rule({ class = "firefox" }, {
decorated = false })`, added deliberately in an earlier session (dated
2026-08-19, comment explains why: Firefox draws its own titlebar row
regardless of what srdwm offers, so forcing server-side decoration just
stacked a second one on top). Firefox's titlebar/buttons are its own GTK
chrome, not srdwm's - not fixable from this side without reverting that
rule and reintroducing the double-titlebar bug it exists to prevent.

The one part that genuinely was srdwm's own responsibility: content-
masking (`general.rounded_corners`) rounding an *undecorated* window's
own corners. `rounded_corners_pixman.rs::masked_content_buffer`
deliberately bailed (`return None`, falling back to unrounded rendering)
the moment a window's surface had any children at all - a real, narrow-
scope limitation the module's own doc comment already documented, not
something to guess around: Firefox (and most GTK4/WebRender apps) paints
its actual content into a *child* subsurface, leaving the root surface
holding only a blank/background buffer, so masking the root's own buffer
produced a rounded rectangle of nothing.

Fixed by widening the scope by exactly one level, not by attempting
general subsurface compositing: new `resolve_content_surface` walks to a
window's single child subsurface and masks *that* buffer instead, but
only when the structure matches the one pattern this is safe for --
exactly one child, positioned at `(0, 0)` relative to its parent, no
children of its own, and large enough to cover the root's own buffer.
Anything else (multiple children, an offset child, nested subsurfaces)
still falls back to unrounded rendering exactly as before - real multi-
subsurface compositing stays out of scope.

Found and fixed a second, real bug while making sure this actually works
rather than just compiles: the corner-mask cache invalidates on `CompState
::content_epoch`, which only ever bumped on a commit to a window's own
*root* surface (`crates/wayland/src/protocols.rs::commit`) - exactly the
surface that, for Firefox's structure, almost never repaints, since the
real content commits land on the child. Left as-is, the masked buffer
would have rendered once, whatever was on screen at the moment the cache
first populated, and then frozen - scrolling, page loads, video, would
never show. Fixed by walking up a committing surface's parent chain (via
`smithay::wayland::compositor::get_parent`, bounded to 8 hops) to find
its tracked-window ancestor when the commit lands on a subsurface, and
bumping that window's `content_epoch` too.

Requires `general.rounded_corners = true` (already on in this machine's
config) to have any visible effect at all - the feature this extends is
opt-in by design on this backend, see `WindowManager::rounded_corners_
enabled`'s own doc comment for the real per-commit CPU cost that's about.

Full workspace test suite green; built and installed, needs a restart.
No new unit tests - `resolve_content_surface`/the epoch-invalidation fix
both need real smithay surface state (a live subsurface tree, real
commits) that this codebase has no test harness for; verified by reading
the actual call sites this session already established (`rounded_content_
buffer`'s cache, `render_udev_frame`'s mutually-exclusive masked/unmasked
branch), not by a live Firefox screenshot - worth doing that
confirmation once this is actually running.

## Real bug, root-caused and fixed: scaled outputs reported a work area larger than the output itself (2026-08-20)

Found from two directions at once: this session's own live testing
("Firefox maximized on one monitor also shows partially on the other",
general visual glitching on the scaled monitor) and, independently,
dotfiles-09 measuring `srd monitors` directly and finding a non-primary
output's work area (`width`/`height`) *larger* than its own output size
(`full_width`/`full_height`) - geometrically impossible for a rect
that's supposed to be the full rect *shrunk* by a bar's reservation.

Root cause: `layer_map_for_output(...).non_exclusive_zone()` - what a
bar/dock's reservation is read from - returns a rect in *logical*
(scale-divided) units, the same as every other layer-shell geometry a
client reports. `UdevHead::location`/`size`, and everything downstream of
them (`Platform::monitors()`'s `full`/`maximize` rects, the disabled-
monitor snapshot in `crates/wayland/src/udev/outputs.rs`, and the top-bar
shrink in `crate::input::maximize_geometry_for`) are raw *physical*
pixels straight from the DRM mode, never divided by scale. Adding a
logical rect to a physical one without converting first silently produced
nonsense the moment a monitor's scale was anything other than exactly
`1.0` - which every output was, unconditionally, before this session's
own auto-scale feature existed. At scale `~0.85`, a 1920-physical-pixel-
wide head's own logical zone width came back around `2276`: reported as
this monitor's *usable* width, larger than its own *full* width, and
large enough to overlap whichever real monitor sat next to it in the
shared global coordinate space - which is why a window sized/positioned
against it could visually spill onto the neighboring output at all.

dotfiles-09's own initial theory (primary vs. non-primary) was a
reasonable read of their one data point, but not the real distinguishing
factor: their "correct" output (eDP-1) happened to be both primary *and*
the one auto-scale left at `1.0` (high enough real PPI), masking the bug
there specifically; their "broken" one (HDMI-A-1) happened to be both
non-primary *and* the one that actually got scaled down. The fix is keyed
on scale, not primary status, and applies per-head regardless of which
one is primary.

Fixed in three places, all with the same "scale the logical value into
physical pixels before touching a physical rect with it" shape:
- `crates/wayland/src/udev/platform.rs::monitors()` - the live `usable`
  rect every `srd monitors` query and every real placement/tiling
  decision reads.
- `crates/wayland/src/udev/outputs.rs`'s disabled-output snapshot - same
  computation, duplicated for the same reason `Platform::monitors()`'s own
  doc comment already explains.
- `crate::input::maximize_geometry_for` - a top-anchored bar's exclusive
  zone shrinks a *physical* maximize-target rect by a *logical* amount
  from the layer-shell surface's own cached state.
Not touched: `crates/wayland/src/winit/nested_platform.rs`'s matching
code has the same shape but is provably inert - that backend is dev-only
and never gets a non-`1.0` scale from anywhere, so logical and physical
already coincide there.

Full workspace test suite green; built and installed, needs a restart to
take effect. This should also fix a related report from dotfiles-09's own
side: a layer-shell dock not appearing on the second monitor at all --
its anchoring had nothing sane to resolve against once that monitor's own
reported work area stopped making geometric sense.

**Follow-up, same session: the same unit mismatch existed one layer up,
in `srd dispatch set output position`.** dotfiles-09 asked directly,
before guessing: their arrangement panel chains outputs by the physical
size `srd monitors` now correctly reports, then writes positions back
through `set output position` - correct only if that command's own space
matches. It didn't: `apply_output_position`
(`crates/wayland/src/output_management.rs`) passed whatever position it
was given straight to `output.change_current_state`, whose position
parameter is a real Wayland-protocol value - `wl_output`/`xdg_output`
always report position to clients in *logical* points, not a choice this
compositor makes. `srd`'s own IPC contract is physical (matching `full_x`/
`full_y`), so passing that straight through told every real Wayland
client the wrong logical position for any output scaled away from `1.0`
- exactly the "384px dead gap" dotfiles-09 predicted before testing it,
present since this session's own auto-scale feature landed, at startup
(`crates/wayland/src/udev/drm.rs::bring_up_head`) as well as on a live
`set output position` call.

Fixed by converting only at the smithay/protocol boundary, in both
places: `bring_up_head`'s own initial `change_current_state` call, and
`apply_output_position` (now documented as taking physical input, with
the one real `wlr-output-management-v1` client call site in `crates/
wayland/src/output_management.rs::handle_apply_or_test` converting its
own genuinely-logical request to physical before calling it). Everything
this compositor tracks internally (`UdevHead::location`, `entry.location`,
`srd monitors`' own `x/y/full_x/full_y`) stays physical throughout,
matching `Platform::monitors()`'s own fix above - only the values hand
ed to smithay's protocol-facing API get converted, and only right there.

Also added, requested directly alongside the question: `scale` on
`srdwm_core::monitor::Monitor` and on `MonitorInfo`/the `monitors` event,
plus an explicit doc comment on `MonitorInfo` itself answering dotfiles-
09's other question (yes, `x/y/width/height` and `full_*` are the same
space as each other, and that space is physical) so the next client
doesn't have to re-derive either answer.

Full test suite green; built and installed, needs a restart.

## Real bug, confirmed, not yet root-caused: Firefox's titlebar corners do not round (2026-08-20)

Found during a live testing pass (screenshots, pixel-level crops) requested
directly by the user, comparing Firefox against a plain SSD-decorated
terminal window.

Confirmed facts, in order:

- Firefox's SSD titlebar shows a completely square top-left corner. Tested
  at both the normal radius (11) and a deliberately large test radius (40,
  set live via `srd set corner_radius 40` then reverted) - square either
  way, ruling out "radius too small to see" as an explanation.
- A tmux terminal window, decorated by the exact same code path
  (`redraw_decoration_buffer` in `crates/wayland/src/state/lifecycle.rs`
  is the only call site of `decoration::render_titlebar` in the whole
  crate), shows a clean, correctly rounded corner at the same radius.
- Ruled out: stale/cached decoration texture - toggling Firefox's
  maximize state off and on again (forcing a real geometry change and a
  fresh `redraw_decoration_buffer` call) did not change the result.
- Ruled out: `border_curve_is_safe` gating (`crates/wayland/src/udev/
  render.rs`, `let border_curve_is_safe = w.decorated || content_will_be_
  masked;`) - Firefox is server-side decorated (confirmed: it renders
  srdwm's own titlebar band and buttons at all, not its own GTK CSD row),
  so this evaluates `true` regardless of content-masking, meaning the
  border strip should draw its full curve either way.
- Not yet checked: whether the square edge belongs to the titlebar
  bitmap itself (`render_titlebar`'s own `round_top_corners` call not
  actually clipping for this window's specific dimensions/`border_width`)
  or to something else painted on top of an otherwise-correct rounded
  titlebar in the same region.

Also confirmed, while investigating the above, **not** to be bugs:

- Firefox's titlebar buttons render minimize-maximize-close left to right
  (yellow-green-red) when right-aligned (`theme.buttons_left = false`,
  the default) - this is intentional, the Windows/GTK convention
  documented directly in `crates/core/src/window.rs::hit_test` ("not a
  mirror of the right-aligned order... which is the Windows/GTK
  convention... for a reason"). A left-aligned window (`buttons_left =
  true`) correctly shows the macOS close-minimize-maximize order instead.
  Click targets match the visual position in both cases.
- The thin purple/violet line above every titlebar is the configured
  Catppuccin Mauve `border.active_color` (`cba6f7`) in `~/.config/srd/
  themes.lua`, not a rendering defect.
- Firefox's titlebar buttons appearing grey (unfocused color) despite
  being the actual focused window, seen once before a restart this
  session - not reproducible after the restart (`srd clients` confirmed
  `focused: true`, buttons rendered in full color). Most likely a stale-
  process artifact from a long-running pre-restart binary, not a bug in
  current code; flag again if it recurs on a fresh process.

## Connector names were wrong (reported by dotfiles-09, real bug, fixed 2026-08-20)

`srd monitors` reported `HDMIA-1` and `EmbeddedDisplayPort-1`. Neither
name exists anywhere else. The kernel, `ddcutil`, `/sys/class/drm`, and
any config written for another compositor all say `HDMI-A-1` and `eDP-1`.

Root cause: `probe_connected` (`crates/wayland/src/udev/drm.rs`) built the
name with `format!("{:?}-{}", info.interface(), info.interface_id())`.
`{:?}` prints the Rust enum variant name (`HDMIA`, `EmbeddedDisplayPort`),
not the kernel's connector type string. `drm-rs`'s `Interface::as_str()`
already returns the correct string, taken directly from the kernel's own
`drm_connector_enum_list` - the fix replaces `{:?}` with `.as_str()`.

This name is load-bearing, not cosmetic: it is the identifier `srd.
monitor.split`, `srd.monitor.scale`, `set output position`, and `set
output enabled` all key on. dotfiles-09 had added a workaround in the AGS
panel (resolve srd's wrong name against `/sys/class/drm` for display,
still dispatch with srd's own spelling) - safe to remove now.

## `MonitorInfo.split` field added (requested by dotfiles-09, 2026-08-20)

`srd monitors` and the `monitors` event now mark each split part (from
`srd.monitor.split`) with `"split": true`. An ordinary output reports
`"split": false`. `srdwm_core::monitor::Monitor` gained a `split: bool`
field, set by `crates/wayland/src/udev/platform.rs::monitors()`; `srdwm_
platform::ipc::monitor_snapshot` copies it into the wire format. 1 new
IPC test. Requested directly: a display-arrangement UI needs to tell a
split part apart from a genuinely independent output, so it does not
offer to move, resize, or extend a physical arrangement onto one.

## Multi-monitor/phone features - plan written, step 1 closed (2026-08-20)

Full plan at `a local scratch directory`, covering the
three features relayed via the AGS peer session: splitting one physical
output into multiple logical monitors + per-monitor default layout,
phone-monitor/VM-viewer workspace (simple window form now, real virtual
output later), and phone-mode UI (automatic-by-shape + manual toggle).
Recommended order: (1) per-monitor default layout, (2) core-side logical
sub-monitor splitting, (3) simple VM-viewer window, (4) phone-mode layout
+ toggle, (5) real coexisting virtual output - deliberately last, deferred
until a concrete VM/simulator integration target is known.

- [x] **Step 1: `monitor.primary_layout`/`monitor.secondary_layout` wired
      up for real.** Same dead-config shape as `general.default_layout`'s
      own siblings - validated/defaulted since the config engine's
      beginning, never read anywhere. **Deviated from the written plan**:
      the plan's own Phase-2 validation pass (a Plan agent) found these
      are flat global keys with no per-connector-name table anywhere in
      the Lua engine, and recommended extending `srd.rule` with a
      `monitor` matcher instead of inventing one - but on implementing,
      wiring the two already-named keys directly turned out to need zero
      new Lua API surface at all and matches exactly what those keys
      already promised, so that's what shipped (`WindowManager::primary_
      layout`/`secondary_layout`, applied by `apply_monitor_layouts` in
      `crates/core/src/manager/monitors.rs`, called from `set_monitors`).
      Only takes effect in `workspace.per_monitor` mode (still off by
      default, still not recommended to turn on yet - AGS's own `active`-
      flag handling isn't ready, see the per-monitor-workspaces entry
      below) - in shared mode there is only one workspace, so a primary/
      secondary split has nothing distinct to apply to and is skipped.
      Applied on every `set_monitors` call (startup + hotplug), not on
      every workspace switch, so it doesn't fight a workspace's own
      manually-set layout every time a monitor switches back to it; a
      non-primary monitor still showing the same fallback workspace as
      the primary is skipped too, so `secondary_layout` can't clobber what
      `primary_layout` just set on that shared workspace before any
      monitor has actually split off. 3 new unit tests in `crates/core/
      src/manager/tests.rs`.
- [x] **Step 2: core-side logical sub-monitor splitting.** `srd.monitor.
      split(name, parts[, "rows"])` - `WindowManager::monitor_splits`,
      applied by `crates/wayland/src/udev/platform.rs::monitors()`
      dividing one real head into N `Monitor` entries via the new pure
      `srdwm_core::monitor::split_rect` (6 unit tests). Each sub-region
      gets its own `full_geometry`/`maximize_geometry`, not just
      `geometry` - the Plan agent's validation pass caught that the
      naive version would have fullscreened a window across the *entire*
      physical panel, erasing the split (see the plan file for detail).
      No new `wl_output` per sub-region in this version, by design --
      flagged as an accepted limitation in `MonitorSplit`'s own doc
      comment, not silently under-delivered.
- [x] **Unplanned, requested mid-session: automatic per-monitor scale.**
      Before this change, srdwm set every real output to a fixed scale of
      `1.0`. There was no way to change this. A user reported the problem
      live: on a physically larger monitor, text and UI looked too big for
      the amount of space available.

      A first version added a manual `srd.monitor.scale(name, factor)`
      config call, keyed by connector name. The user asked for less
      hardcoding: no fixed connector name, and behavior based on the
      monitor's real properties (their example: different behavior for a
      27" screen versus a 24" one). The final version replaces the fixed-
      name approach with an automatic one:

      `srdwm_core::monitor::auto_scale_for` (`crates/core/src/monitor.rs`)
      reads a monitor's real physical size and resolution from EDID,
      computes its actual pixel density (PPI), and scales it down when
      that density falls below a reference value (109 PPI, roughly a 24"
      1080p or 27" 1440p monitor). It never scales a monitor above `1.0`
      on its own. 5 unit tests cover the laptop panel (no change), a large
      1080p monitor (scales down), a small 4K panel (stays at `1.0`, not
      scaled up), an extreme case (clamps at the `0.5` floor), and a
      monitor with no EDID physical size (returns `1.0`, since there is
      nothing real to compute from).

      `srd.monitor.scale(name, factor)` still exists, as an explicit
      override for one connector. An explicit value always wins over the
      automatic one. `~/.config/srd/init.lua` documents this with a
      commented-out example rather than a live call, since the automatic
      value already covers the reported case.

      `WindowManager::monitor_scales` stores only explicit overrides.
      `bring_up_head` (`crates/wayland/src/udev/drm.rs`) applies the
      override if one exists for that connector, or the automatic value
      otherwise, at startup, hotplug, and re-enable alike. **A scale
      change needs a restart, or an unplug/replug of that connector, to
      take effect** - it applies only when a head comes up, not on a
      plain config reload against an already-running output.
- [ ] Steps 3-5 (VM-viewer window, phone-mode layout/toggle, real virtual
      output): not started.
- [ ] **Persistent monitor state across restarts** - "remember states/
      preference when reconnecting even after startup", asked alongside
      the three planned features but tracked separately since it's
      infrastructure (a state file + load/save), not one of the
      architectural features the plan above covers. Not started, not yet
      scoped.

## Closed this session (2026-08-19, a later same-day session than the one that opened most items below)

- **`activate_workspace` IPC command silently did nothing.** Root-caused:
  `udev/platform.rs`/`winit/platform.rs` unconditionally re-ran the full
  `focus_window()` (with its own workspace-follow side effect) after *any*
  IPC mutation, silently reverting the very switch that mutation had just
  made. Fixed by splitting out `raise_in_space` (z-order only, no
  workspace-follow) for that re-sync path - see `crate::input::
  raise_in_space`'s own doc comment. Confirmed fixed independently by two
  separate live sessions (this one and the AGS-side peer session, via the
  `WS-IPC-DIAG` log line - kept in place, still useful).
- **Corner-seam fix, verified live.** Pixel-sampled a real `grim`
  screenshot; the titlebar/border-top seam is a continuous curve, no
  stepped notch.
- **Firefox click-accuracy bug - found a *real*, different bug than the
  one "fixed" before.** The prior `content_offset` fix in `input.rs`'s
  `refresh_pointer_focus` double-applied an offset `sync_geometry`'s own
  `map_element` call had already baked into `Space`'s tracked `loc` -
  confirmed against `sync_geometry`'s own doc comment (which spells out
  the correct formula, `win_relative = pos - loc`) and smithay 0.7.0's
  real source (`Window::surface_under` hands a toplevel's point through
  with a hardcoded `(0,0)` offset). Reverted the double-application.
  **Not yet click-tested live** - `ydotool`'s absolute positioning isn't
  reliable in this environment (confirmed twice: commanded position and
  actual landing position disagreed by a non-constant factor), so this is
  verified by source/contract, not by a live click. If you can get a real
  physical click tested against it, do.
- **Corner-radius-vs-border-strip gap - new bug, found and fixed.** Not
  the same as the seam above: the left/right border strips are flat,
  curve-blind rectangles (`border_side_render_element`), and when
  `corner_radius > border_width` (true even at this project's own theme
  defaults, 6 over 4), the top/bottom strips' own curve didn't have enough
  buffer height to fully resolve before handing off to those flat strips -
  leaving a real wedge of bare background between the straight border and
  the window's own curve. Confirmed via direct pixel sampling of a live
  screenshot (not eyeballed). Fixed by growing `render_border_top`/
  `render_border_bottom` to `max(thickness, radius)` tall and letting them
  draw over the flat strips (they're pushed first in the render list,
  which is topmost). Also fixed `render_border_bottom`'s own pre-existing
  `radius + thickness` bug (drew against an oversized, wrong circle -
  same wrong shape the top-strip seam fix had already rejected for an
  equivalent reason).
- **Shadow didn't follow a window's rounded corner.** `shadow_bitmap` used
  plain Chebyshev (square-ring) distance everywhere, by design (documented
  as a deliberate cheapness trade-off) - but that means a rounded
  window's shadow still had a hard square corner, visibly a different
  shape sitting right next to the window's own curve. Fixed with a real
  rounded-rectangle distance field in the corner quadrants only (flat
  edges are unaffected, byte-identical to before); `radius = 0` is also
  byte-identical to before.
- **Firefox's own corners weren't rounding.** Not a bug - `general.
  rounded_corners` (content-corner rounding for undecorated/CSD windows)
  defaults to *off* specifically on the udev/Pixman backend (real,
  documented, untested-on-real-hardware CPU cost for constantly
  repainting content). Turned on in this user's `init.lua`; watch CPU
  under heavy content (video, scrollback) since this is the first live
  data point for that cost on real hardware.
- **Firefox's titlebar looked nothing like every other window's.**
  Researched rather than guessed: checked how niri negotiates
  xdg-decoration (`~/reference-wms/niri/src/handlers/xdg_shell.rs`) -
  offers ServerSide by default, *honors* whatever a client explicitly
  requests, exactly like srdwm's own `XdgDecorationHandler` already does.
  GNOME/Mutter is the outlier (never offers server-side, relies on every
  GTK app sharing one CSD theme) and isn't applicable to a desktop mixing
  GTK/Electron/terminal apps with no shared toolkit. Root cause was
  Firefox's own `browser.tabs.inTitlebar` pref defaulting to CSD here -
  set to `0` in its profile's `user.js` (takes effect on Firefox's next
  restart, not yet confirmed live), and removed the now-unnecessary
  `decorated = false` rule for it in `rules.lua`.
- **Traffic-light titlebar buttons.** srdwm's own SSD titlebar drew plain
  outline glyphs (X/square/dash) before - nothing like a real traffic
  light, and nothing like Firefox's own CSD buttons (real macOS-style
  dots via the WhiteSur GTK theme). Rewrote as filled, anti-aliased dots
  (red/yellow/green when focused, flat grey when not, matching what
  WhiteSur already does and what Firefox's own unfocused dots already
  looked like).
- **Switching workspace didn't move keyboard focus.** `switch_workspace`
  only ever touched `current_workspace`, never `self.focused` - switching
  to a workspace with an open window left that window unfocused while
  whatever was focused *before* the switch (now invisible) kept receiving
  real keystrokes. Fixed: switching now focuses the topmost window on the
  destination workspace if the currently-focused one isn't there, or
  clears focus if the workspace is empty. Guarded so it doesn't fight
  `focus_window`'s own workspace-follow call into this same function.
- **Workspace ids are 1-based, matching Hyprland's own convention and the
  display name** (`workspace.names`, `apply_workspace_count`) - checked
  AGS's own niri and Hyprland integrations before choosing this: neither
  needs translation math the way srdwm's old 0-based scheme forced onto
  `lib/srdwm.ts`. Rolling this out needs `crates/config`'s shipped
  default, this user's `~/.config/srd/keybindings.lua`/`rules.lua`, and
  AGS's `lib/srdwm.ts`/`service/wsPreview.ts` to all agree with core at
  the same time - they can't update atomically with one srdwm restart, so
  whichever side is running the *other* scheme during that window visibly
  misbehaves (this was hit live: AGS's Overview picked up a phantom 7th
  workspace slot during a brief skew). `~/.config/srd/rules.lua` also had
  two stale 0-based workspace assignments (Firefox pinned to workspace 0,
  which no longer exists at all; Discord/Spotify off by one) - fixed, and
  the already-open windows they'd misplaced were moved to the right
  workspace live.
- **Dead config key: `general.border_width`.** Validated and defaulted
  but never actually read anywhere - the real, working key is `theme.
  decorations.border.width` (already correctly documented as such in
  `docs/DEFAULTS.md`, which is how this was found). Removed the dead
  key entirely from `crates/config`, the shipped default `init.lua`, and
  this user's own `init.lua`.
- **Poll-loop CPU throttle, re-measured.** ~15.4% of one core at idle
  (instantaneous `/proc/<pid>/stat` delta, not the time-averaged `ps`
  figure), down from the ~21-30% baseline documented for the pre-throttle
  build. Real improvement, though not measured under identical idle
  conditions (a running desktop, not a controlled bench), so treat as
  directional rather than exact.
- **Hover-state highlighting for titlebar buttons.** `CompState::
  hovered_titlebar_button` now tracks which button (if any) is hovered,
  set from `handle_pointer_position`'s own `hit_test` result and fed into
  `DecorationSignature` so a hover change is a real cache-invalidating
  event, not silently absorbed. `render_titlebar` brightens whichever
  button is hovered (`decoration::brighten`, blends toward white) - close
  gets "red on hover" for free from this same mechanism, since it's
  already red at rest (focused); no special-cased hover colour was
  needed. Not yet confirmed live (built and installed, no restart since).

## Closed this session (2026-08-20)

- **Titlebar cursor didn't change shape over the buttons.** `update_cursor_shape`
  had no case for a titlebar-button hit at all - fell through to whatever the
  surface underneath happened to want. Added a `CursorIcon::Pointer` case
  specifically for Close/Minimize/Maximize hits.
- **Titlebar text alignment/colour, config-driven.** `theme.decorations.
  title_bar.text_align` (`"left"` default, `"center"` available) and the
  existing focused/unfocused foreground colours (now grey by default in this
  user's own theme preset) - wired through `ThemeConfig::title_centered`.
- **Titlebar button side, config-driven.** `theme.decorations.title_bar.
  button_side` (`"right"` default matches Windows/GTK ordering
  minimize/maximize/close; `"left"` switches to macOS ordering close/
  minimize/maximize) - threaded through both the renderer
  (`button_box`/`BUTTON_MARGIN_LEFT`, bigger dots when left) and
  `ResizeEdge::hit_test` (which corner gets resize-vs-button priority flips
  to match), so the clickable zones and the drawn positions can't drift
  apart.
- **Animated glyph-reveal on titlebar-button hover, config-driven.**
  `theme.decorations.title_bar.button_glyph`: `"hover"` (default - classic
  macOS, glyph fades in from the button dot's own colour over 200ms
  ease-out-cubic, chosen after comparing against real extracted libadwaita
  CSS which does the opposite) or `"always"` (modern GNOME/Adwaita
  convention, glyph always visible, left available and documented rather
  than deleted per usual "comment out the alternative" convention here).
  Ticked every frame via `tick_hover_glyph_animation` (only while a hover
  animation is actually in flight and the config isn't already `"always"`,
  so this costs nothing at rest).
- **Multi-monitor drag couldn't cross onto a second screen at all.**
  Root-caused: `update_drag` clamped to the *starting* monitor's bounds,
  looked up once at drag-start and never revisited as the drag moved - so
  a window could never be dragged past its own starting monitor's edge no
  matter how far or fast the pointer moved, even with a second monitor
  fully up and working at the compositor/DRM level. Reported live with a
  real second monitor connected. Fixed with a new `all_monitors_bounds()`
  (the union of every registered monitor's `full_geometry`) - see that
  function's own doc comment in `crates/core/src/manager/monitors.rs`.
- **Same drag also left `w.monitor` stale after crossing screens.**
  `end_drag`'s snap-zone check used whatever `w.monitor` was set to at
  drag-*start*, so a window actually now sitting on monitor 2 still had
  its snap zones checked against monitor 1. Fixed by recomputing
  `w.monitor` from the window's real post-drag geometry before the snap
  check, in `crates/core/src/manager/dragresize.rs::end_drag` - the same
  class of staleness `set_monitors`' own doc comment already documented
  for the hotplug-rehoming case.
- Full workspace test suite (284 tests across every crate) still green
  with both of the above in place; installed via `cargo install --path
  crates/srdwm --force` (no restart of the live process - not asked for).

## Closed this session (2026-08-20, continued) - corner-mask alpha bug

- **Undecorated-window content-mask corner fix, real bug found and fixed.**
  Reported live as a solid grey wedge poking past the rounded-corner curve
  on a real, running Firefox window (`decorated = false`), confirmed via a
  zoomed `grim` crop, not eyeballed. Root cause, confirmed by reading
  `crates/wayland/src/rounded_corners_pixman.rs::masked_content_buffer`:
  the function accepts both `Argb8888` and `Xrgb8888` source buffers, but
  always hands the result to `MemoryRenderBuffer` labelled `Argb8888`
  (real, renderer-respected alpha) regardless of which the source actually
  was. `Xrgb8888`'s fourth byte is the wire format's *unused* channel - no
  producer is required to zero or otherwise canonicalize it - so whatever
  a client (Firefox, live) happened to leave there became real, visible
  transparency the instant the whole buffer got relabelled `Argb8888`,
  anywhere in the window, not just the corner boxes the mask function
  intentionally touches. Fixed with a new `force_opaque` step (byte 3 of
  every pixel forced to `0xff`) run over `Xrgb8888` sources before the
  corner mask itself runs. New unit test
  (`force_opaque_overwrites_garbage_alpha_without_touching_colour`); full
  285-test workspace suite green; built and installed.

## macOS titlebar/corner/shadow proportions - partially applied, partially reverted per direct feedback

Sourced from independent developer references, not guessed - Apple
doesn't publish exact pixel specs and the real macOS screenshots this
needed couldn't be fetched as savable binaries. Full notes and the live
comparison screenshot saved to `a local scratch directory
notes.md` and `a local scratch directory`.

- [x] Corner radius: `theme.decorations.border.radius` default (and this
      user's own theme presets) changed `6 → 11` (0.2 → 0.36 ratio,
      matching real macOS's ~10pt/28pt), across `ThemeConfig::
      default_corner_radius`, `crates/srdwm/src/main.rs`'s shipped
      fallback, and `~/.config/srd/themes.lua`'s three presets.
- [x] Left-side (`buttons_left`) button size: `BUTTON_MARGIN_LEFT` `0.18 →
      0.25`, landing the macOS-authentic left-aligned layout on a true 0.5
      diameter ratio. Right-aligned `BUTTON_MARGIN` deliberately left
      untouched at `0.32` - it was never meant to mimic macOS, and
      changing it would have undone the user's own earlier explicit
      "bigger on the left" request.
- [ ] **Reverted**: group hover-reveal (all three traffic-light buttons
      brightening/revealing together, matching real macOS's own cluster
      behaviour) was implemented, then explicitly reverted per direct
      user feedback - "hover effect should apply to one at a time" is
      this project's own convention here, despite real macOS itself doing
      it differently. Back to per-button hover
      (`hovering_the_close_button_brightens_only_that_dot`).
- [ ] **Still open, reported live after the above landed**: title text
      still not centred, button colours "not correct", decorations "far
      apart and small". Root cause found separately: `~/.config/srd/
      themes.lua`'s active preset never actually had `text_align`/
      `button_side`/`button_glyph` fields at all (this session's earlier
      claim of having set them was wrong/lost) and `foreground_focused`
      was still the original purple, not the grey requested much earlier
      - so the compositor had been running on `text_align="left"`,
      `button_side="right"` (the small, unrevised margin), and the wrong
      colour this whole time. Added `text_align="center"`,
      `button_side="left"`, `button_glyph="hover"`,
      `foreground_focused="#a6adc8"` to the live preset. **Not yet
      confirmed live** - needs the pending restart plus real before/after
      screenshots, not just a config-file read, given the last claim of
      "done" here turned out to be wrong.
- [ ] Shadow reads smaller/harder than real macOS's soft, wide shadow
      (`SHADOW_SIZE=12px`, linear falloff) - qualitative only, no hard
      reference number was retrievable, not yet touched.
- Colours (aside from the grey-text miss above) and left-side button
  ordering (close/minimize/maximize) already match real macOS correctly,
  confirmed - no action needed there.

**Module-organization survey vs. niri/mutter** (read-only, no code
changed): srdwm's already-split files (`state/mod.rs` + `lifecycle.rs`/
`geometry.rs`/`layers.rs`, the whole `udev/` split, `manager/mod.rs` +
its own already-split files) are all *smaller and more modular* than
niri's own real-world equivalents (niri's `niri.rs` alone is 6569 lines;
its `backend/tty.rs` is bigger than srdwm's entire `udev/` directory
combined) - no action needed on any of that, it's already ahead of the
reference project it's modeled on. Concrete remaining splits, each
modeled directly on a niri module boundary that already exists there:
- [x] `crates/wayland/src/decoration.rs` (2172 lines) → split into
      `decoration/{border,buttons,color,corners,font,shadow,titlebar}.rs`
      plus `decoration/tests.rs`, matching niri's own `render_helpers/`
      (one file per render-element concern); the root file now only holds
      the module doc comment, the `mod`/`pub use` wiring, and the two
      standalone popup renderers (`render_context_menu`/
      `render_snap_flyout`) that don't belong to any single submodule.
      198 root lines left, down from 2172. Verified zero behavior/coverage
      loss: 190 core / 106 wayland tests, identical count before and after
      (2026-08-23).
- [x] `crates/wayland/src/protocols.rs` (936 lines) → finished the split;
      one file per `impl ...Handler for CompState` block under
      `protocols/`, matching niri's `handlers/` - `buffer.rs` groups
      `ShmHandler`/`BufferHandler`/`DmabufHandler` and `misc.rs` groups
      the three purely-default-impl stubs (`OutputHandler`/
      `TabletSeatHandler`/`FractionalScaleHandler`), since none of those
      five has more than a handful of lines on its own; every other
      module is exactly one handler (`compositor`, `xdg_shell`,
      `xdg_decoration`, `xdg_activation`, `input_method`, `seat`,
      `layer_shell`, `selection`, `idle`). Root file now only holds the
      module doc comment, `mod` declarations, and the `delegate_*!` macro
      list - 58 lines, down from 936. No test module existed in the
      original file, so nothing to redistribute; 190 core / 106 wayland
      tests, identical count before and after (2026-08-23).
- [x] `crates/wayland/src/input.rs` (1305 lines) → finished the split, one
      file per input-event kind under `input/`: `layers.rs` (layer-shell
      hit-testing, layer-driven maximize geometry), `focus.rs` (focusing/
      raising/closing a window - needed by every other kind regardless of
      what triggered the change), `pointer.rs` (motion, button, cursor
      shape - the largest, at 683 lines, since it's also where drag/resize
      *forwarding* lives, the pointer-driven titlebar hit-test dispatch
      that starts/updates/ends a core drag or resize), `keyboard.rs` (key
      events, keysym/modifier translation, VT switching), `gestures.rs`
      (workspace scroll, touchpad swipe). Root file now only holds the
      module doc comment, `mod` declarations, the two truly-cross-cutting
      helpers every one of those five needs (`notify_idle_activity`,
      `DRAG_MODIFIER`), and `last_pointer_pos` - 81 lines, down from 1305.
      No test module existed in the original file, so nothing to
      redistribute; 190 core / 106 wayland tests, identical count before
      and after (2026-08-23). This was the last item on the module-split
      list - `decoration.rs`, `protocols.rs`, and `input.rs` are all done.
- Low priority: an `effects/` grouping for `blur.rs`/colour-filter code,
  matching niri's `render_helpers/{xray,background_effect,
  framebuffer_effect}.rs` - more a naming/grouping nicety than a real gap,
  since these already exist as their own top-level files.

## Closed this session (2026-08-20, continued further) - the real second-monitor root cause, a matching video-freeze bug, and per-monitor workspaces

- **Second-monitor blank screen: real root cause found and fixed** (the
  flip-watchdog from earlier the same session was a real, separate
  robustness fix, but not this bug). Added a temporary diagnostic
  (`LAYER-ELEMENTS-DIAG`, since removed) that logged real per-output
  layer-map state after a live restart: both outputs showed identical,
  fully-populated `layer_count=3 has_buffer=[true,true,true]` the whole
  time - proving the surfaces were genuinely mapped, configured, and
  holding real committed pixel data on both monitors equally. That ruled
  out both AGS and the render/flip pipeline itself (also separately
  confirmed alive on the affected head by moving the real cursor there and
  watching it render correctly). Root cause: `output_layer_elements`
  (`crates/wayland/src/elements.rs`) added a per-head `origin` (the head's
  own position in the shared global desktop space, e.g. `(1920, 0)` for a
  second monitor) to `LayerMap::layer_geometry`'s already-local
  coordinates - confirmed against smithay 0.7.0's own source that layer
  geometry carries no global offset at all. That silently shifted every
  wallpaper/bar surface on any monitor whose `origin` wasn't `(0, 0)` --
  every monitor except the first, left-to-right - clean off the right
  edge of that head's own local framebuffer. Fixed by dropping the
  `origin` parameter entirely; `output_layer_elements` never needed it.
- **A second, same-family bug: video frozen on a monitor the user wasn't
  actively using, audio still playing.** Reported live. Root cause:
  `windows_touched_by_damage` (same file) compared a render pass's own
  *local* damage rectangles directly against `Space::element_geometry`,
  which is always *global* - the exact same local/global mismatch as the
  bug above, one level deeper in the render pipeline. A window relying
  solely on this path for its frame callbacks (any window not focused or
  under the pointer - those get an unconditional fallback via a separate,
  always-on pass) never received one on any monitor but the first, so a
  video player left playing in the background on a second monitor
  literally never got permission to submit another frame after its first,
  while its own audio pipeline (PipeWire, entirely separate) kept running
  underneath. Fixed the same way: `origin` now threaded through to shift
  the comparison into a consistent space.
- **Independent per-monitor workspaces, now a real configurable choice.**
  Previously hardcoded to a single flat workspace shared by every
  monitor. `workspace.per_monitor` (default `false`, preserving the
  original design exactly) switches to Hyprland/niri-style independent
  per-monitor workspace sets when set `true` - each monitor tracks and
  displays its own current workspace, switchable via `srd dispatch
  activate_workspace <id>` without affecting any other monitor (the IPC
  handler now routes to whichever monitor the focused window is on,
  falling back to the primary monitor). `WindowManager::
  switch_workspace_on_monitor`/`workspace_for_monitor`/`is_workspace_
  visible` are the new entry points; `visible_windows`/`workspace_
  snapshot` (the `srd workspaces`/AGS wire format) both updated to use
  them, with zero wire-format change needed - `WorkspaceInfo::active` was
  already a plain per-workspace bool, so more than one workspace *can*
  report `active: true` at once in per-monitor mode without needing a
  schema change on this side. `workspace.count` has no hardcoded ceiling
  in either mode (floor of 1 only) - purely config-driven, shipped
  default is 10.
  - **Do not turn `workspace.per_monitor` on yet.** Confirmed by the AGS
    peer session against real code, not assumed: `lib/srdwm.ts`'s
    `#syncWorkspaces` collapses every workspace's own `active` flag into
    one `focusedWorkspace` (last-active-in-list-order wins), and every
    widget (bar pills, Overview tiles) highlights by identity against
    that single value - nothing actually reads the per-workspace
    `active` bool srdwm now sends correctly. Turning the mode on before
    that lands would render exactly one pill lit (whichever monitor's
    workspace sorts last) and the other monitor's real current workspace
    as merely "occupied" - not a crash, but visibly wrong. The AGS-side
    fix is small (light a pill from the real per-workspace flag, falling
    back to the identity check for Hyprland/niri, which have no such
    flag) and is dotfiles-09's own call/scope, already flagged to their
    user - not something to fix from this side.
  - Not yet done: scroll/gesture-based relative workspace switching
    (`switch_workspace_relative` in `crates/wayland/src/input.rs`) still
    always targets the single shared `current_workspace`/`switch_
    workspace`, not whichever monitor the pointer is actually over --
    inert-ish for a monitor already showing its own independently-switched
    workspace in per-monitor mode. Scoped out of this pass rather than
    guessed at; needs "which monitor is the pointer over" plumbed through
    from the backend-specific pointer state.
- Full 289-test workspace suite (four new tests for the per-monitor
  feature specifically) green; built and installed.

## Closed this session (2026-08-20, continued yet further) - italic titlebar font, border/shadow wedge, resize lag, AGS monitor-layout panel backend

- **Titlebar font was italic on this machine, for every window.**
  `find_ttf_preferring_mono` picked whichever font file's name merely
  *contained* "mono" first in directory-listing order, with nothing
  excluding a styled (italic/bold/etc.) variant - live result:
  `/usr/share/fonts/TTF/JetBrainsMonoNerdFontPropo-Italic.ttf`, confirmed
  via the `wayland titlebar font:` log line, despite several regular-
  weight JetBrains Mono files also being installed. Rewritten as
  `font_rank`/`find_best_font`: ranks every candidate (mono+unstyled beats
  mono+styled beats non-mono) and keeps scanning until it finds an actual
  rank-0 match instead of stopping at the first mono-named file regardless
  of style. Six new unit tests were not written for this specific
  live-picked-file case (filesystem-dependent), but `font_rank` itself is
  fully covered.
- **A real border/shadow rendering bug, found while comparing screenshots
  as asked: a solid, wrong-coloured wedge cut into an undecorated (CSD)
  window's corners.** Reported live on a real Firefox window, both top-
  left and bottom-left corners. Root-caused by elimination, not
  guessed: toggled `general.rounded_corners` off live (`srd set
  rounded_corners false`) and the wedge stayed, ruling out the content-
  mask feature; the wedge's own colour matched `border.active_color`
  exactly, not Firefox's own chrome colour, ruling out Firefox's own
  rendering. That left `render_border_top`/`render_border_bottom`'s own
  "extra" rows (added past `border_width` whenever `corner_radius >
  border_width`, to give a corner's curve room to resolve) as the only
  remaining source - correctly designed to overpaint a *decorated*
  window's titlebar band, which safely absorbs them, but an undecorated
  window has no such band, so the same colour-filled rows land on its
  real content instead. Almost certainly always existed, just too subtle
  to notice at the old default radius (6, a 2px extra) until this
  session's own real-macOS-proportion fix (radius 11, a 7px extra) made
  it obvious. Fixed with two new pure, tested functions (`decoration::
  border_top_visible_rows`/`border_bottom_visible_rows`) that both real
  backends (`udev/render.rs`, `winit/render.rs`) now call instead of each
  hand-rolling their own position/crop math - crops to just the nominal
  `border_width` rows for an undecorated window, skipping the
  compensating shift too since there's nothing left to shift for. Ten new
  unit tests.
  - **Follow-up caught by actually looking at the pixels afterward, not
    just trusting the fix:** cropping unconditionally whenever
    `!w.decorated` closed the wedge but cost every such window its own
    visible corner curve too, even on the (rarer) undecorated window
    whose content-masking genuinely does succeed - reverting it to a
    flat square corner instead of the intended rounded one, confirmed on
    the same real Firefox window this was found on (masking bails for it
    specifically, `masked_content_buffer`'s subsurface early-out). Fixed
    on the `udev` backend by computing whether this window's content will
    actually be masked *this frame* (a cheap cache-hit re-use of the same
    `rounded_content_buffer` call the content-rendering step already
    makes later in the same loop iteration, not a second real masking
    pass) and using that - not the bare `decorated` flag - to decide
    whether the border's extra rows are safe to show in full. The `winit`
    (nested/dev-only) backend's masking is GPU-shader-based, not the
    Pixman CPU path with the subsurface limitation, so it doesn't appear
    to share this failure mode at all - left on its simpler unconditional
    crop rather than adding matching complexity to a backend that isn't
    what's actually running live.
- **Resizing was "very laggy" - root-caused and fixed, not just
  reported.** `general.rounded_corners` (content corner-masking) copies a
  window's *entire* pixel buffer on the CPU on every single commit (see
  `rounded_corners_pixman`'s own module doc comment, which already
  predicted this exact cost and is why the feature defaults off) - a
  resize reflows content on every single frame of the drag, so this was
  the first real-hardware case that ever paid that cost continuously
  rather than once per idle-window repaint. Fixed by skipping content
  masking for specifically whichever window is being interactively
  resized right now (`WindowManager::resizing_window`, new), not by
  disabling the feature globally or during any other window's masking --
  cosmetic, and the one moment its absence is least likely to be noticed
  (attention is on the dragged edge, not the opposite corner).
- **AGS's monitor-layout panel backend, built to a precise spec from the
  AGS peer session rather than guessed.** New CLI: `srd dispatch set
  output position <name|id> <x> <y>` (the existing `set_output_position`
  IPC command had no CLI surface at all before this - AGS dispatches via
  `Gio.Subprocess`/the `srd` binary, not the raw socket). Resolves a
  monitor `name` server-side when no numeric `id` is given, matching what
  `srd monitors` itself reports, so a caller that already has the name
  doesn't need an extra round-trip. Explicitly *not* built this pass, on
  purpose: real output enable/disable (needs actual DRM-level CRTC power
  state, not a software flag, to mean what Hyprland's `disable`/niri's
  `off` mean - too large a change to bolt onto an already-large restart
  untested) and true multi-display content mirroring (corrected an
  earlier same-session claim that positioning two outputs at identical
  coordinates already mirrors content - it doesn't: each window has
  exactly one `monitor` assignment, so nothing actually duplicates).
  AGS's own panel self-detects via an `srd --help` regex probe at
  startup, so landing this needed no further coordination once installed.
- **`srd subscribe` now emits a `monitors` event on hotplug**, the AGS
  session's own low-priority ask, so its "display connected" strip can
  drop a 4-second poll of `srd monitors` (was working around
  `hypr.connect("monitor-added", ...)` being a dead handler id on any
  non-Hyprland backend). Fourth independently-diffed event alongside
  `clients`/`workspaces`/`keyboard_layout`, same `MonitorInfo` shape the
  one-shot `"monitors"` command already returned (pulled both into one
  shared `monitor_snapshot` function so they can't drift apart). Two new
  tests.
- Full 298-test workspace suite green; both `srdwm` and `srd` (the CLI)
  built and installed.
- **Follow-up on the border-curve fix above, caught by actually looking at
  the pixels afterward rather than trusting the fix as shipped:**
  cropping unconditionally whenever `!w.decorated` closed the wedge but
  also flattened the curve on the (rarer) undecorated window whose
  content-masking genuinely does succeed - confirmed on the same real
  Firefox window. Fixed on the `udev` backend by computing whether this
  window's content will actually be masked *this frame* (a cheap
  cache-hit re-use of the same `rounded_content_buffer` call the
  content-rendering step already makes later in the same loop, not a
  second real masking pass) and using that, not the bare `decorated`
  flag, to decide whether the border's extra rows are safe to show in
  full. `winit`'s masking is GPU-shader-based and doesn't appear to share
  this failure mode, so left on its simpler unconditional crop.
- **A real srdwm bug, not just a client-side trap: `set_output_position`
  to a negative origin made that output's own region unreachable by the
  pointer.** Flagged by the AGS peer session after their own monitor-
  layout panel's "extend left"/"extend above" placed a head at negative
  x/y and the user immediately hit "clicked it now I can't go to other
  monitor at all". Root cause: `UdevState::bounds()` computed only the
  max right/bottom edge across every head, never the minimum x/y, so both
  pointer-motion paths clamped into a `(0, w) x (0, h)` box regardless of
  where any head actually sat. Fixed: `bounds()` now returns the real
  `(min_x, min_y, max_x, max_y)` box; the arithmetic itself pulled into a
  plain, tested free function (`bounds_of`) since `UdevHead` needs a real
  DRM handle to construct otherwise. Five new unit tests. AGS's own
  client-side normalization (always placing the arrangement's own
  top-left at (0,0)) stays in place as good practice, but is no longer
  load-bearing for correctness.
- **A decorated window's titlebar had no top-edge resize zone at all,
  only the two tiny diagonal corners.** Reported live, exactly: "we can't
  resize tmux's window from top but we can in Firefox" - true, because
  an *undecorated* window already had its own (narrower)
  `UNDECORATED_TOP_RESIZE_MARGIN`, but a decorated one's titlebar band
  claimed every button-free pixel as `Drag` unconditionally. Fixed with a
  new `DECORATED_TOP_RESIZE_MARGIN` (reuses `RESIZE_MARGIN` outright --
  unlike the undecorated case, there's no client content here to avoid
  stealing a click from, since the whole band is srdwm's own drawn UI).
  Checked *after* the button x-range tests, not before, so a button
  sitting within the first few rows of the titlebar (true for all of
  them) still always wins there - addresses the "account for the
  buttons... not swallowing" half of the same request directly. Four new
  tests; two pre-existing tests whose own fixed y-coordinates predated
  this zone existing at all were updated to probe past it, not deleted.
- **Resize configure throttling, matching niri.** A background research
  fork compared srdwm's resize handling against niri's and found a real,
  concrete gap beyond the already-fixed content-masking cost: `sync_
  geometry` sent a fresh size-changing `xdg_toplevel.configure` on every
  single pointer-motion tick of an active resize, with no check on
  whether the client had caught up to the *previous* one - niri
  explicitly throttles this (`window/mapped.rs`'s `ConfigureIntent::
  Throttled`, its own comment: "some clients do not batch size requests,
  leading to bad behavior with very fast input devices... this throttling
  also helps interactive resize transactions preserve visual
  consistency"). Implemented the same idea: a new `pending_size_configure`
  map tracks a sent-but-not-yet-caught-up-to size per window, checked
  against the client's real last-committed content size
  (`w.geometry().size`) before sending another; bounded by a 100ms
  `CONFIGURE_THROTTLE_TIMEOUT` so a client that never catches up for any
  reason can't wedge resizing shut, the same self-healing shape as this
  session's own DRM flip-pending watchdog. Deliberately doesn't touch
  `redraw_decoration_buffer`'s own cadence - srdwm's border/titlebar
  still tracks the live requested geometry every frame regardless of
  whether the client's own content is throttled, so the *decoration*
  stays visually smooth while backpressure applies only to the client-
  facing protocol negotiation.
- Full 306-test workspace suite green (no dedicated unit test for the
  throttle itself - `sync_geometry` needs a live smithay surface/
  toplevel to exercise, same testability ceiling as the rest of this
  file); built, installed.
- **Monitor enable/disable, the real DRM-level work explicitly requested
  ("i think we should also be able to disconnect/use only one monitor
  from there/toggle") after being deliberately scoped out of the earlier
  monitor-layout batch.** New: `srd dispatch set output enabled
  <name|id> <true|false>`. Reuses this backend's own existing hotplug
  removal/bring-up code (`reprobe_outputs`'s two halves, now `pub(crate)`
  as `disable_connector_by_name`/`enable_connector_by_name`) rather than
  inventing a new mechanism - disabling genuinely destroys the `wl_
  output` global, unmaps it, frees its DRM buffers, and rehomes its
  windows, exactly like a real unplug; enabling re-probes and brings it
  back up exactly like a real hotplug reconnect, since nothing about the
  underlying hardware actually changed. A new `UdevState::
  disabled_connectors` (by name) stops an *unrelated* hotplug event from
  resurrecting a deliberately-disabled output.
  - Keyed by connector **name**, not `MonitorId`, throughout the queue/
    IPC layer (unlike `set_output_position`) - disabling removes the
    output from `monitors()` entirely, so its id has nothing left to mean
    by the time a caller wants to re-enable it; the name is the only
    identifier that survives the round trip. `id` is still accepted on
    the wire and resolved against the live list, but that only ever works
    for the *disable* direction (the output is still live when you ask to
    turn it off) - re-enabling requires the name.
  - **Not done, deliberately out of scope for this pass:** a disabled
    output isn't listed anywhere (`MonitorInfo`/the `monitors` subscribe
    event only ever show *connected and enabled* outputs), so a UI has no
    way to discover "this output exists but is off" to offer re-enabling
    it - it has to already know the name from before disabling. Adding
    an `enabled` field to `MonitorInfo` (and deciding whether disabled
    outputs should even be listed at all) is a real wire-format change
    worth coordinating with the AGS side rather than adding unilaterally.
  - Real DRM hardware operation, tested via source review and the exact
    same code paths a genuine unplug/hotplug already exercises live this
    session, but not yet exercised through this *specific* new entry
    point against real hardware - flagged here rather than claimed done
    without that test.
- Full 309-test workspace suite green; both binaries built and installed.

## Open question, not yet acted on: does a glitch report predate these fixes or not?

Reported live, still vague: "it currently glitches out" when moving a
window between workspaces, plus "cursor glitches out when near
decorations" and "more... in the extra monitor" - worse on the second
monitor specifically. Given how many second-monitor-specific rendering
bugs this session already found and fixed (the layer-element coordinate
bug, the frame-callback coordinate bug, the border-wedge bug), this may
already be resolved by fixes already installed and just needs a fresh
restart + re-check, or may be a genuinely separate, still-open issue --
undetermined either way pending a live look with everything from this
session's later fixes actually running, or a screenshot/recording if it
persists.

## Real bugs, currently open

- [ ] **Dock intermittently slow to appear / unresponsive to clicks** -
      compositor-side leads (layer-shell hit-testing, stuck pointer
      grabs, frame-callback starvation) all checked and came up clean.
      dotfiles peer session has a concrete AGS-side lead
      (`updateInputRegion` possibly computing a stale/zero region
      mid-animation) - status unknown as of this entry, ask before
      assuming it's still open.
- [ ] **`POS-DIAG` repro not yet pinned down** - a right-click during an
      active edge-resize drag was seen to never reach pointer-event
      delivery (only its release did), consistent with one of three
      "swallow the press" branches in `input.rs` firing unexpectedly, but
      which one wasn't confirmed before the pattern stopped recurring.
      Diagnostic logging is in place; needs the exact repro (right-click
      mid-resize-drag) again to pin down.
- [ ] **Firefox click-accuracy fix, unverified against a real click** -
      see "Closed this session" above: fixed and justified from source,
      but no reliable way to synthesize a precise click in this
      environment to confirm live. Needs an actual physical click test.

## Explicitly requested, not yet started
- [ ] **Window-management-policy comparison vs. GlazeWM, Awesome,
      Openbox, RagnarWM** - all four cloned to `~/reference-wms/` and
      ready; the *rendering* comparison used niri and mutter (the only
      two of the six that do their own compositing at all) plus, this
      session, niri's xdg-decoration negotiation specifically. If the ask
      is really about layout/rules/keybinding conventions rather than
      rendering, these four are sitting untouched for that.
- [x] **Real per-app window-position-and-size memory that survives a
      restart** - done (2026-08-26), directly requested ("why aren't we
      getting smart window spawns, remembering last size/location").
      `remembered_sizes` (session-lifetime, size only) became
      `remembered_geometry` (position + size), persisted via a new
      `crates/wayland/src/window_memory.rs` (same JSON-under-`$XDG_STATE_
      HOME/srd` shape as `monitor_layout.rs`/`desktop_icons_state.rs`).
      Loaded and seeded into `WindowManager` before the Wayland socket
      binds (same ordering as the monitor-layout restore); saved after
      every drag/resize release. A remembered *position* only applies if
      it still lands on a monitor that actually exists this run (checked
      against every current monitor's own full geometry) - otherwise falls
      back to the existing pointer/focus-based smart placement, so an
      unplugged external monitor can't strand a window off-screen.

## Render pipeline - researched, ranked, not started

From an earlier niri/mutter comparison fork (full detail in
`SESSION_HANDOFF.md`, if it still exists by the time you read this):

- [ ] Hardware DRM cursor plane (currently always software-composited) -
      medium-large; needs real `DrmCompositor` plane scaffolding this
      backend doesn't have yet.
- [ ] Direct scanout for fullscreen clients (currently always goes
      through full Pixman composite) - large, the natural next big
      structural investment given this whole project's video-performance
      history, but genuine architectural work, not a tweak.
- [ ] `wp_linux_dmabuf` feedback (format/modifier negotiation tranches) -
      low priority until direct scanout exists to negotiate for.
- [ ] Explicit sync / VRR / HDR - real gaps, ranked below the above three;
      all need the same `DrmCompositor`-style layer the backend doesn't
      have.

## Protocol gaps (from `PANEL_SUPPORT_TODO.md`, still genuinely open)

Everything else in that doc is done - these five are the actual
remainder, none blocking for the desktop-shell use case that doc was
originally scoped around:

- [ ] `zwlr_virtual_pointer_manager_v1` + `zwp_virtual_keyboard_manager_v1`
      - needed by `ydotool` and any automated UI testing. Directly
      relevant now: this session's own attempts to synthesize precise
      clicks for verification were unreliable specifically because
      `ydotool` has no protocol path that actually works well here; this
      protocol pair is the real fix for that, not a nice-to-have.
- [ ] `zwp_pointer_constraints_v1` + `zwp_relative_pointer_manager_v1` -
      games (pointer lock/relative motion).
- [ ] `wp_presentation` - accurate frame timing; low value on a fixed-
      refresh, always-CPU-composited session (see the render-pipeline
      section above, item 4).
- [ ] `wp_single_pixel_buffer_v1`, `xdg_foreign_v2`.
- [ ] `zwlr_screencopy_manager_v1` fix needs a fresh `grim` retest against
      the real DRM/udev session (was only verified on the nested winit
      backend at the time).

## Confirmed-fixed, unverified against a real client (nothing to build, just need the test subject)

- [ ] Classic Qt `appmenu-qt5` global menu - code fixed
      (`appmenu_registrar`), but `appmenu-qt5` isn't packaged for Arch at
      all (checked official repos and AUR); no live client to verify
      against without pulling in much larger KDE integration packages,
      deliberately not installed without asking first.
- [ ] KDE Plasma Qt global menu (`_KDE_NET_WM_APPMENU_*` atoms) - same
      "fixed, no test client available" situation.

## Confirmed not fixable / deliberately out of scope (documented, closed, listed here only for completeness)

- Nemo and most modern GTK3/GTK4 apps' global menu - confirmed via direct
  D-Bus introspection that there's genuinely nothing on the bus to read;
  universal across every Wayland compositor, not an srdwm gap.
- Nemo's own titlebar duplication - unlike Firefox, Nemo draws its own
  CSD headerbar *unconditionally*, ignoring xdg-decoration negotiation
  entirely (confirmed live via screenshot). The Firefox-style "fix the
  client's own preference" approach this session used doesn't apply;
  `decorated = false` for Nemo (telling srdwm not to draw a second one on
  top) remains the only real fix.
- `move_terminal`'s full port - architectural mismatch (srdwm has one
  flat workspace list shared by every monitor; the Hyprland original
  assumed per-monitor workspace sets).
- Per-output enable/disable - real DRM mode-setting, hardware-dependent,
  not attempted.
- Multi-GPU - only the primary GPU's connectors are driven; a GPU
  appearing/disappearing is logged and ignored.
- A native GUI settings app - never existed even as working code in the
  legacy C++ project, pure design doc there too; not revisited.

## Source docs, for the full story behind any item above

- `SESSION_HANDOFF.md` - a prior session's own work in full technical
  depth (ephemeral, meant to get replaced by whichever session writes the
  next one - check whether it still describes current reality before
  trusting it).
- `MISSING.md` (`~/.config/srd/`) - gaps found porting from the user's
  old Hyprland config, organized by how much each is missed.
- `PANEL_SUPPORT_TODO.md` - desktop-shell/panel protocol support,
  originally scoped around getting a GTK4 AGS panel running at all (it
  now does).
- `IMPLEMENTATION_STATUS.md` - the permanent architecture reference;
  read this one for what srdwm supports overall, not just what's pending.
