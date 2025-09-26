#!/usr/bin/env bash
set -euo pipefail

sudo apt-get update
sudo apt-get install -y \
  build-essential cmake ninja-build pkg-config git \
  liblua5.4-dev \
  libx11-dev libxrandr-dev libxinerama-dev libxfixes-dev libxcursor-dev \
  libxcb1-dev libxcb-keysyms1-dev libxcb-icccm4-dev libxcb-ewmh-dev libxcb-randr0-dev \
  libxft-dev \
  libwayland-dev wayland-protocols \
  libxkbcommon-dev libseat-dev \
  libdrm-dev libegl1-mesa-dev libgbm-dev libudev-dev libinput-dev \
  libpixman-1-dev \
  libwlroots-dev || true

# Fallback for distros that use wlroots-dev instead of libwlroots-dev
if ! pkg-config --exists wlroots; then
  sudo apt-get install -y wlroots-dev || true
fi

echo "Dependencies installed."
