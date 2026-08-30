# srdwm

A cross-platform window manager configured in Lua.

Every backend draws a real title bar, and every title bar supports drag,
resize, minimize, maximize and close. No backend falls back to a plain
border.

## Features

- Tiling, dynamic, floating, centred and manual layouts, switchable at
  runtime
- Server-side decorations with working buttons on every backend
- Lua configuration with hot reload on write
- Multi-monitor, with per-monitor layout and workspace assignment
- Window rules matched on class, title and role
- Session lock, desktop icons with context menus, and a global menu over
  `com.canonical.AppMenu.Registrar`
- Layer-shell and XWayland on Wayland
- Optional GPU rendering path (GBM and EGL) alongside the software one
- Headless outputs for testing without hardware

## Status

| Backend | State |
| --- | --- |
| Wayland | Complete and used daily. Decorations, session lock, desktop icons, global menu, layer-shell, XWayland, optional GPU rendering, headless outputs, and a virtual pointer that can be pinned to one window. |
| X11 | Working. Reparenting window manager with a drawn title bar, buttons, drag, resize and a right-click window menu. Feature-matched against the Wayland backend. |
| Windows | Under development. Border tinting through `DwmSetWindowAttribute`, low-level input hooks and monitor enumeration are written and `cfg(windows)`-gated, but are not yet built or run on Windows. |
| macOS | Under development. Window control through the Accessibility API is written and `cfg(target_os = "macos")`-gated, but is not yet built or run on macOS. |

`docs/IMPLEMENTATION_STATUS.md` breaks down what is complete per backend,
`docs/TODO.md` tracks what is pending, `docs/ARCHITECTURE.md` covers the
crate layout, and `docs/FEATURE_GAP.md` compares srdwm against other
compositors and desktop environments.

## Building

```bash
cargo build --workspace
cargo test --workspace
```

Linux needs the X11, Wayland, EGL, GLES, xkbcommon, libinput and libseat
development headers, which `x11rb` and `smithay` link against. On Arch:

```bash
sudo pacman -S libx11 libxcb wayland wayland-protocols egl-wayland \
    mesa xkbcommon libinput libseat pixman pam clang
```

On Debian and Ubuntu:

```bash
sudo apt install libx11-dev libxcb1-dev libxcb-randr0-dev \
    libxcb-keysyms1-dev libwayland-dev wayland-protocols \
    libxkbcommon-dev libegl1-mesa-dev libgles2-mesa-dev libinput-dev \
    libudev-dev libgbm-dev libdrm-dev libseat-dev libpixman-1-dev \
    libpam0g-dev libclang-dev
```

Lua is built from source through `mlua`'s `vendored` feature, so no
system Lua package is needed.

## Running

```bash
cargo run -p srdwm
```

The backend is chosen automatically: Wayland when `WAYLAND_DISPLAY` or
`XDG_SESSION_TYPE=wayland` is set, X11 otherwise. Unset those variables
to force X11.

To try it without disturbing your session, run it nested:

```bash
Xephyr :99 -screen 1280x800 &
DISPLAY=:99 SRDWM_CONFIG_PATH="$PWD/config/srd" cargo run -p srdwm
```

## Configuration

srdwm reads `$SRDWM_CONFIG_PATH`, then `$XDG_CONFIG_HOME/srd`, then
`~/.config/srd`. [`config/srd/`](config/srd/) is a complete working
example: `init.lua` loads `keybindings.lua`, `layouts.lua`, `themes.lua`,
`monitors.lua`, `rules.lua` and `startup.lua`.

```lua
local srd = require("srd")

srd.bind("Mod4+Return", function() srd.spawn("alacritty") end)
srd.bind("Mod4+q", function() srd.window.close() end)
srd.bind("Mod4+h", function() srd.window.focus("left") end)

srd.layout.set("tiling")
srd.layout.configure("tiling", { master_ratio = 0.6, gaps = { inner = 8, outer = 16 } })

srd.theme.set_colors({ background = "#2e3440", accent = "#88c0d0" })
```

Every option and its default is listed in
[`docs/DEFAULTS.md`](docs/DEFAULTS.md).

## License

MIT, see [`LICENSE`](LICENSE).
