#!/usr/bin/env bash
# SRDWM Bootstrap Installer
# Detects platform/distro, installs dependencies, builds, tests, and installs.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()  { echo -e "${BLUE}[INFO]${NC} $*"; }
success(){ echo -e "${GREEN}[OK]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERR]${NC} $*"; }

usage(){
  cat <<EOF
Usage: $0 [options]

Options:
  --deps                 Install dependencies only
  --build [Debug|Release]  Build only (default Release)
  --test                 Run tests after building
  --install [PREFIX]     Install (default /usr/local)
  --all                  Install deps, build, test, install
  --no-wayland           Disable Wayland support (X11 only)
  --real-wayland         Use real wlroots backend (requires full deps)
  --preset <name>        Use CMake preset (overrides generator/build flags)
  --generator <Ninja|Unix Makefiles>  Choose generator if no preset
  -h, --help             Show this help
EOF
}

platform="unknown"
case "$(uname -s)" in
  Linux*) platform="linux";;
  Darwin*) platform="macos";;
  CYGWIN*|MINGW*|MSYS*) platform="windows";;
  *) platform="unknown";;
esac

ACTION="build"
BUILD_TYPE="Release"
RUN_TESTS=false
INSTALL_PREFIX="/usr/local"
ENABLE_WAYLAND=ON
USE_WAYLAND_STUB=ON
CMAKE_PRESET=""
GENERATOR="Ninja"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --deps) ACTION="deps"; shift;;
    --build) ACTION="build"; BUILD_TYPE="${2:-$BUILD_TYPE}"; shift 2;;
    --test) RUN_TESTS=true; shift;;
    --install) ACTION="install"; INSTALL_PREFIX="${2:-$INSTALL_PREFIX}"; shift 2;;
    --all) ACTION="all"; shift;;
    --no-wayland) ENABLE_WAYLAND=OFF; shift;;
    --real-wayland) USE_WAYLAND_STUB=OFF; shift;;
    --preset) CMAKE_PRESET="${2}"; shift 2;;
    --generator) GENERATOR="${2}"; shift 2;;
    -h|--help) usage; exit 0;;
    *) warn "Unknown option: $1"; usage; exit 1;;
  esac
done

install_deps_linux(){
  if command -v apt-get >/dev/null 2>&1; then
    bash scripts/install_deps_ubuntu.sh
  elif command -v dnf >/dev/null 2>&1; then
    bash scripts/install_deps_fedora.sh
  elif command -v pacman >/dev/null 2>&1; then
    bash scripts/install_deps_arch.sh
  elif command -v zypper >/dev/null 2>&1; then
    bash scripts/install_deps_opensuse.sh
  elif command -v apk >/dev/null 2>&1; then
    bash scripts/install_deps_alpine.sh
  else
    error "Unsupported Linux distro. Install dependencies manually (see DEPENDENCIES.md)."; exit 1
  fi
}

install_deps_macos(){
  bash scripts/install_deps_macos.sh
}

check_deps_windows(){
  info "On Windows, use PowerShell: scripts\\install_deps_windows_vcpkg.ps1"
}

configure_build(){
  mkdir -p build
  if [[ -n "$CMAKE_PRESET" ]]; then
    info "Configuring with preset: $CMAKE_PRESET"
    cmake --preset "$CMAKE_PRESET"
    return
  fi

  local cache_file="build/CMakeCache.txt"
  local generator_arg=( )
  local effective_generator="$GENERATOR"
  if [[ -f "$cache_file" ]]; then
    local cached_gen
    cached_gen=$(grep -E '^CMAKE_GENERATOR:INTERNAL=' "$cache_file" | sed 's/^[^=]*=//') || true
    if [[ -n "${cached_gen:-}" ]]; then
      effective_generator="$cached_gen"
      info "Reusing existing CMake generator from cache: $effective_generator"
    fi
  fi

  if [[ ! -f "$cache_file" ]]; then
    generator_arg=( -G "$effective_generator" )
  else
    # Do not override generator if cache exists
    generator_arg=( )
  fi

  info "Configuring with generator=${effective_generator}, type=$BUILD_TYPE, ENABLE_WAYLAND=$ENABLE_WAYLAND, USE_WAYLAND_STUB=$USE_WAYLAND_STUB"
  cmake -S . -B build "${generator_arg[@]}" \
    -DCMAKE_BUILD_TYPE="$BUILD_TYPE" \
    -DENABLE_WAYLAND="$ENABLE_WAYLAND" \
    -DUSE_WAYLAND_STUB="$USE_WAYLAND_STUB"
}

build(){
  if [[ -n "$CMAKE_PRESET" ]]; then
    cmake --build --preset "$CMAKE_PRESET" -j
  else
    cmake --build build -j
  fi
}

test_run(){
  if [[ -n "$CMAKE_PRESET" ]]; then
    ctest --test-dir $(jq -r '.configurePresets[] | select(.name=="'"$CMAKE_PRESET"'") | .binaryDir' CMakePresets.json 2>/dev/null || echo build) --output-on-failure || true
  else
    ctest --test-dir build --output-on-failure || true
  fi
}

install(){
  if [[ "$platform" == "windows" ]]; then
    cmake --install build --prefix "$INSTALL_PREFIX"
  else
    sudo cmake --install build --prefix "$INSTALL_PREFIX"
  fi
}

info "Platform detected: $platform"
case "$ACTION" in
  deps)
    case "$platform" in
      linux) install_deps_linux;;
      macos) install_deps_macos;;
      windows) check_deps_windows;;
      *) error "Unsupported platform"; exit 1;;
    esac
    ;;
  build)
    [[ "$platform" == "linux" ]] && install -d build >/dev/null 2>&1 || true
    configure_build
    build
    $RUN_TESTS && test_run
    ;;
  install)
    configure_build
    build
    install
    ;;
  all)
    case "$platform" in
      linux) install_deps_linux;;
      macos) install_deps_macos;;
      windows) check_deps_windows;;
    esac
    configure_build
    build
    test_run
    install
    ;;
  *) usage; exit 1;;
esac

success "Bootstrap completed."
