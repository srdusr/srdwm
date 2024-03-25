#!/usr/bin/env bash
set -euo pipefail

sudo zypper refresh
sudo zypper install -y \
  gcc-c++ cmake ninja git pkg-config \
  lua54 lua54-devel \
  libX11-devel libXrandr-devel libXinerama-devel libXfixes-devel libXcursor-devel \
  libxcb-devel xcb-util-keysyms-devel xcb-util-wm-devel \
  libXft-devel \
  wayland-devel wayland-protocols-devel \
  libxkbcommon-devel seatd-devel \
  libdrm-devel Mesa-libEGL-devel Mesa-libgbm-devel libinput-devel \
  pixman-devel \
  wlroots-devel || true

echo "Dependencies installed."
