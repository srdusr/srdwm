#!/usr/bin/env bash
set -euo pipefail

if ! command -v apk >/dev/null 2>&1; then
  echo "This script is intended for Alpine Linux (apk not found)." >&2
  exit 1
fi

sudo apk update
sudo apk add --no-cache \
  build-base cmake ninja git pkgconf \
  lua5.4 lua5.4-dev \
  libx11-dev libxrandr-dev libxinerama-dev libxfixes-dev libxcursor-dev \
  libxcb-dev xcb-util-keysyms-dev xcb-util-wm-dev \
  libxft-dev \
  wayland-dev wayland-protocols \
  libxkbcommon-dev seatd-dev \
  libdrm-dev mesa-egl mesa-gbm \
  libinput-dev \
  pixman-dev \
  wlroots-dev || true

echo "Dependencies installed."
