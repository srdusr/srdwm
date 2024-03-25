#!/usr/bin/env bash
set -euo pipefail

sudo dnf groupinstall -y "Development Tools" "Development Libraries"
sudo dnf install -y \
  cmake ninja-build pkgconf-pkg-config git \
  lua-devel \
  libX11-devel libXrandr-devel libXinerama-devel libXfixes-devel libXcursor-devel \
  libxcb-devel xcb-util-keysyms-devel xcb-util-wm-devel xcb-util-renderutil-devel xcb-util-image-devel \
  libXft-devel \
  wayland-devel wayland-protocols-devel \
  libxkbcommon-devel seatd-devel \
  libdrm-devel mesa-libEGL-devel mesa-libgbm-devel systemd-devel libinput-devel \
  pixman-devel \
  wlroots-devel || true

echo "Dependencies installed."
