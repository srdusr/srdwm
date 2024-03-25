#!/usr/bin/env bash
set -euo pipefail

sudo pacman -Syu --noconfirm
sudo pacman -S --noconfirm \
  base-devel cmake ninja pkgconf git \
  lua \
  libx11 libxrandr libxinerama libxfixes libxcursor \
  libxcb xcb-util xcb-util-keysyms xcb-util-wm \
  libxft \
  wayland wayland-protocols \
  libxkbcommon seatd \
  libdrm mesa egl-wayland libgbm \
  libinput \
  pixman \
  wlroots

echo "Dependencies installed."
