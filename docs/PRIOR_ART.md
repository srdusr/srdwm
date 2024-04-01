# Prior art

Two kinds of "prior art" shaped this rewrite: the legacy C++ codebase this
project itself started as, and the wider field of window managers/compositors
that solve pieces of the same problem.

## The legacy C++ codebase

`legacy-cpp/` (formerly the repo root) is preserved for reference. An
`Explore`-agent audit of it before the rewrite found it was mostly a design
skeleton rather than working software:

| Backend | Status | What was real |
|---|---|---|
| X11 (`x11_platform.cc`) | Partially functional | Reparenting decoration model, EWMH atom list, basic Xlib window ops. No drag/resize, hardcoded 800px titlebar width, RandR monitor bug (used physical mm instead of pixel mode), fake "another WM" detection. |
| Windows (`windows_platform.cc`) | Partially functional | DWM border-color tinting, global low-level hooks, real `EnumDisplayMonitors`. No virtual desktop support, no subclassing. |
| Wayland (`wayland_platform.cc`) | Architecture only | Created the wlroots backend/renderer/compositor/seat/xdg-shell objects in the right order, but never called `wl_signal_add` on a single one - no window was ever actually managed. |
| macOS (`macos_platform.cc`) | Mostly stub | Real Accessibility-permission request and `CGDisplay`-based monitor enumeration. Window move/resize were empty TODOs; the "overlay window" decoration idea was never implemented. |
| Lua config (`lua_manager.cc`) | Partially functional | Scalar config get/set worked. `srd.bind()` stored the key-combo string but not the actual Lua closure - keybindings could never fire. `srd.window.focused()` returned a hardcoded placeholder table with no methods, so the shipped example config's `window:close()` would have errored at runtime. |

The Rust rewrite fixes these rather than porting them: see
`docs/IMPLEMENTATION_STATUS.md` for what's real now, and the module-level doc
comments in `crates/x11/src/lib.rs` and `crates/wayland/src/lib.rs` for the
specific bugs each backend's replacement corrects.

## Comparable window managers/compositors

None of these were copied from - srdwm's Lua-config, single-binary,
cross-platform-trait design doesn't match any of them exactly - but each
informed a specific decision:

- **[niri](https://github.com/YaLTeR/niri)** (Rust, Wayland, smithay) - the
  closest architectural sibling to `crates/wayland`. Confirms smithay is a
  viable foundation for a real tiling compositor, not just toy examples.
- **[river](https://codeberg.org/river/river)** (Zig, Wayland, wlroots) --
  configured via an external CLI/IPC protocol rather than an embedded
  scripting language. srdwm deliberately goes the other way (embedded Lua,
  matching the project's own history and this rewrite's brief), but river is
  a useful reminder that "protocol, not library" is a legitimate alternative
  to what this project does.
- **[leftwm](https://github.com/leftwm/leftwm)** and
  **[penrose](https://github.com/sminez/penrose)** (Rust, X11) - both
  reimplement dwm/xmonad-style tiling in Rust; penrose in particular is a
  "bring your own `main.rs`" library rather than a turnkey binary. Reference
  points for idiomatic X11-in-Rust event loop structure (`crates/x11` uses
  `x11rb`, as both of these do).
- **[komorebi](https://github.com/LGUG2Z/komorebi)** and
  **[glazewm](https://github.com/glzr-io/glazewm)** (Rust, Windows) - both
  keep the *native* DWM frame and manage layout/focus around it rather than
  replacing decorations, communicating with a companion CLI over IPC. This
  directly informed `crates/windows`' documented plan: Windows doesn't get a
  custom titlebar by disabling the native frame and hand-drawing one (as X11
  does) but by controlling the existing frame (`DWMWA_BORDER_COLOR` etc.),
  since that's what's actually achievable without fighting DWM.
- **[yabai](https://github.com/koekeishiya/yabai)** and
  **[AeroSpace](https://github.com/nikitabobko/AeroSpace)** (macOS) - yabai
  uses private, partially-undocumented APIs and a signed "scripting addition"
  that requires disabling System Integrity Protection for full functionality;
  AeroSpace deliberately restricts itself to the public Accessibility API,
  trading some capability for not requiring SIP changes. `crates/macos`'
  documented design follows AeroSpace's approach (AX-only), consistent with
  the legacy C++ code's own choice to request Accessibility permission
  rather than pursue private APIs.

## What's genuinely novel here (not borrowed from anywhere)

- `srdwm_core::window::ResizeEdge::hit_test` - one hit-testing/decoration
  function shared verbatim by both the X11 and Wayland backends, so "drag
  the titlebar" and "grab the corner to resize" behave identically
  regardless of display server. Neither the legacy C++ nor any of the
  projects above shares decoration logic across backends this way (each
  reimplements per-backend, since none of them target both X11 reparenting
  *and* Wayland SSD from the same core).
- The `srd` Lua API's shape (`srd.window.close()` acting on the currently
  focused window, `srd.window.focus("left")` for directional navigation,
  `srd.workspace.next()`) matches what `docs/DEFAULTS.md` always
  *documented* - but the legacy engine never actually implemented that
  surface (see table above). This rewrite implements the documented API for
  real rather than inventing a new one.
