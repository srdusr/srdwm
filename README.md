# SRDWM

SRDWM is a cross-platform window manager. You configure it entirely in
Lua. Every backend draws a real title bar. Every title bar supports drag,
resize, minimize, maximize, and close. No backend falls back to a plain
border instead.

This is a Rust rewrite of an earlier C++ prototype; see
[`docs/PRIOR_ART.md`](docs/PRIOR_ART.md) for what carried over, what got
fixed, and what other window managers informed the design. The C++
version itself has been removed from the tree (it was actively
misleading to keep alongside a fully working rewrite) - git history
still has it if anyone needs it.

## Status

All four backends are equal in design intent. Implementation depth
differs only because Linux is the only platform with a working
development machine right now.

| Backend | Status |
|---|---|
| Wayland | Daily-driver complete: real decorations (titlebar text, resize, snap), session lock, desktop icons with right-click menus, global menu (`com.canonical.AppMenu.Registrar`/dbusmenu), layer-shell, XWayland, an optional GPU (GBM+EGL) render path alongside the default software one, fake (headless) monitors, and a Phase-2 multi-cursor primitive (pinning a virtual pointer to one window). Verified end-to-end on real Linux hardware. |
| X11 | Working: a reparenting window manager with a drawn title bar (buttons, drag, resize, right-click window menu). Feature-audited for parity with the Wayland backend. Verified end-to-end on real Linux hardware. |
| Windows | Under development. Real, `cfg(windows)`-gated code exists for border tinting (`DwmSetWindowAttribute`), low-level input hooks, and monitor enumeration. This code has never been built or run on a Windows machine, because no Windows machine is available in the current development environment. |
| macOS | Under development. Real, `cfg(target_os = "macos")`-gated code exists for the Accessibility-API window control this backend needs. This code has never been built or run on a macOS machine, because no macOS machine is available in the current development environment. |

Full detail, including exactly what's real vs. stubbed and why, is in
[`docs/IMPLEMENTATION_STATUS.md`](docs/IMPLEMENTATION_STATUS.md) --
note that some of its own detail (test counts, specifically) predates
this rewrite's later growth; `docs/TODO.md` is the actively-maintained
list of what's pending, with `cargo test --workspace` as the actual
current count. Crate layout and design rationale are in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md); a survey against
comparable compositors and full desktop environments is in
[`docs/FEATURE_GAP.md`](docs/FEATURE_GAP.md).

## Building

```bash
cargo build --workspace
cargo test --workspace
```

Linux needs the usual X11/Wayland/EGL/GLES/xkbcommon/libinput/libseat
development headers (`x11rb` and `smithay` link against them); on Arch:

```bash
sudo pacman -S libx11 libxcb wayland wayland-protocols egl-wayland \
    mesa xkbcommon libinput libseat lua54
```

`mlua` builds Lua 5.4 from source (`vendored` feature), so no system Lua
package is required.

## Running

```bash
cargo run -p srdwm
```

Backend selection is automatic: Wayland if `WAYLAND_DISPLAY` or
`XDG_SESSION_TYPE=wayland` is set, X11 otherwise (override by unsetting
those env vars). To try it without touching your real session, run it
against a nested server:

```bash
Xephyr :99 -screen 1280x800 &
DISPLAY=:99 SRDWM_CONFIG_PATH="$PWD/config/srd" cargo run -p srdwm
```

## Configuration

Config lives at `$SRDWM_CONFIG_PATH`, or `$XDG_CONFIG_HOME/srd`, or
`~/.config/srd`. [`config/srd/`](config/srd/) in this repo is a
complete working example (`init.lua` loads `keybindings.lua`, `layouts.lua`,
`themes.lua`, `monitors.lua`, `rules.lua`, `startup.lua`). Full API and
default values: [`docs/DEFAULTS.md`](docs/DEFAULTS.md).

```lua
local srd = require("srd")

srd.bind("Mod4+Return", function() srd.spawn("alacritty") end)
srd.bind("Mod4+q", function() srd.window.close() end)
srd.bind("Mod4+h", function() srd.window.focus("left") end)

srd.layout.set("tiling")
srd.layout.configure("tiling", { master_ratio = 0.6, gaps = { inner = 8, outer = 16 } })

srd.theme.set_colors({ background = "#2e3440", accent = "#88c0d0" })
```

## License

MIT, see [`LICENSE`](LICENSE).
