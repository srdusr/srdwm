# SRDWM

A cross-platform window manager, configured entirely in Lua, aiming to feel
like a native window manager on every platform it targets rather than a
compromise between them: real title bars with drag/resize/minimize/maximize/close
on every backend that can support them, not just a border.

This is a Rust rewrite of an earlier C++ prototype; see
[`docs/PRIOR_ART.md`](docs/PRIOR_ART.md) for what carried over, what got
fixed, and what other window managers informed the design. The C++
version itself has been removed from the tree (it was actively
misleading to keep alongside a fully working rewrite) - git history
still has it if anyone needs it.

## Status

Linux is the primary target and the only one verified end-to-end so far.
The Wayland backend is the daily-driver target and by far the more
complete of the two - a real `smithay` compositor with its own GPU
render path, session lock, desktop icons, global menu, layer-shell
(bars/docks), fully rendered decorations, and more:

| Backend | Status |
|---|---|
| Wayland | Daily-driver complete: real decorations (titlebar text, resize, snap), session lock, desktop icons with right-click menus, global menu (`com.canonical.AppMenu.Registrar`/dbusmenu), layer-shell, XWayland, an optional GPU (GBM+EGL) render path alongside the default software one, fake (headless) monitors, and a Phase-2 multi-cursor primitive (pinning a virtual pointer to one window) |
| X11 | Working - reparenting WM with a drawn title bar (buttons, drag, resize, right-click window menu), feature-audited for parity with the Wayland backend |
| Windows | Designed, not built (no Windows target in dev sandbox) |
| macOS | Designed, not built (no macOS target in dev sandbox) |

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
