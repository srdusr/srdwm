# SRDWM Dependencies

This file lists the required and optional dependencies per platform. Package names are provided for popular distros. If your distro is not listed, install equivalent packages and ensure `pkg-config` can find them.

## Common Tools

- C++17 compiler (gcc/clang or MSVC)
- CMake ≥ 3.20 recommended
- Ninja (recommended) or Make
- pkg-config
- Git
- Lua 5.4 + development headers

## Linux (X11)

Required libraries:
- X11: x11, xrandr, xinerama, xfixes, xcursor
- XCB: xcb, xcb-keysyms, xcb-icccm, xcb-ewmh, xcb-randr
- Optional: Xft for font rendering (`xft`)

Distro packages:
- Ubuntu/Debian: `libx11-dev libxrandr-dev libxinerama-dev libxfixes-dev libxcursor-dev libxcb1-dev libxcb-keysyms1-dev libxcb-icccm4-dev libxcb-ewmh-dev libxcb-randr0-dev libxft-dev`
- Fedora: `libX11-devel libXrandr-devel libXinerama-devel libXfixes-devel libXcursor-devel libxcb-devel xcb-util-keysyms-devel xcb-util-wm-devel xcb-util-renderutil-devel xcb-util-image-devel libXft-devel`
- Arch: `libx11 libxrandr libxinerama libxfixes libxcursor libxcb xcb-util xcb-util-keysyms xcb-util-wm libxft`
- openSUSE: `libX11-devel libXrandr-devel libXinerama-devel libXfixes-devel libXcursor-devel libxcb-devel xcb-util-keysyms-devel xcb-util-wm-devel libXft-devel`
- Alpine: `libx11-dev libxrandr-dev libxinerama-dev libxfixes-dev libxcursor-dev libxcb-dev xcb-util-keysyms-dev xcb-util-wm-dev libxft-dev`

## Linux (Wayland)

Wayland can be built in two modes:
- Stub backend (default): builds without linking to full wlroots at runtime. Set `-DUSE_WAYLAND_STUB=ON`.
- Real wlroots backend: full compositor link. Requires wlroots and additional libs.

Required packages for real backend:
- Wayland: `wayland-client`, `wayland-protocols`
- wlroots: `wlroots`
- Others: `pixman-1`, `libxkbcommon`, `libdrm`, `EGL/Mesa`, `gbm`, `libinput`, `seatd`, `udev`

Distro packages:
- Ubuntu/Debian: `libwayland-dev wayland-protocols libwlroots-dev libpixman-1-dev libxkbcommon-dev libdrm-dev libegl1-mesa-dev libgbm-dev libinput-dev libudev-dev libseat-dev`
- Fedora: `wayland-devel wayland-protocols-devel wlroots-devel pixman-devel libxkbcommon-devel libdrm-devel mesa-libEGL-devel mesa-libgbm-devel libinput-devel seatd-devel systemd-devel`
- Arch: `wayland wayland-protocols wlroots pixman libxkbcommon libdrm egl-wayland libgbm libinput seatd`
- openSUSE: `wayland-devel wayland-protocols-devel wlroots-devel pixman-devel libxkbcommon-devel libdrm-devel Mesa-libEGL-devel Mesa-libgbm-devel libinput-devel seatd-devel`
- Alpine: `wayland-dev wayland-protocols wlroots-dev pixman-dev libxkbcommon-dev libdrm-dev mesa-egl mesa-gbm libinput-dev seatd-dev`

## Windows

- Visual Studio 2019+ with C++ toolset or MSVC Build Tools
- CMake and Ninja recommended
- Lua 5.4 via vcpkg: `vcpkg install lua:x64-windows`
- Windows SDK (10.0.19041.0+)

Use: `scripts\install_deps_windows_vcpkg.ps1`

## macOS

- Xcode Command Line Tools
- Homebrew packages: `cmake ninja lua pkg-config git`

Use: `bash scripts/install_deps_macos.sh`

## Notes

- If `pkg-config` cannot find a dependency, ensure the `-dev` (headers) package is installed and `PKG_CONFIG_PATH` is set correctly.
- You can disable Wayland entirely with `-DENABLE_WAYLAND=OFF` if building for X11 only.
- The default configuration enables Wayland with a stub backend for broader compatibility. Switch to the real backend with `-DUSE_WAYLAND_STUB=OFF` once `wlroots` and related deps are available.
