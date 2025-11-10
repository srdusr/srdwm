# SRDWM - Cross-Platform Window Manager

SRDWM is a modern, cross-platform window manager that provides a unified window management experience across Linux (X11/Wayland), Windows, and macOS. It features smart window placement, togglable decorations, and easy switching between tiling and floating layouts.

## Features

### **Cross-Platform Support**
- **Linux X11**: Full server-side decoration control with frame windows
- **Linux Wayland**: Modern wlroots compositor with zxdg-decoration protocol
- **Windows**: Native DWM border integration with global hooks
- **macOS**: Platform-constrained but functional with overlay windows

### **Smart Window Placement**
- **Windows 11-style grid placement** with optimal cell sizing
- **Cascade placement** for overlapping windows
- **Snap-to-edge** functionality
- **Overlap detection** and free space finding algorithms

### **Window Management**
- **Togglable decorations** with custom border colors and widths
- **Easy tiling/floating toggle** with per-window state management
- **Multiple layout engines**: Tiling, Dynamic, and Floating modes
- **Smart placement integration** for floating windows

### **Lua Configuration**
- **Scriptable configuration** with full Lua API
- **Real-time configuration reloading**
- **Platform-specific keybindings** and settings
- **Theme and decoration customization**

## Requirements

### **Linux (X11)**
```bash
# Ubuntu/Debian
sudo apt install build-essential cmake pkg-config liblua5.4-dev \
    libx11-dev libxrandr-dev libxinerama-dev libxcb-dev \
    libxcb-keysyms1-dev libxcb-icccm4-dev

# Fedora
sudo dnf install gcc-c++ cmake pkgconfig lua-devel \
    libX11-devel libXrandr-devel libXinerama-devel libxcb-devel \
    libxcb-keysyms-devel libxcb-icccm-devel

# Arch Linux
sudo pacman -S base-devel cmake pkgconf lua \
    libx11 libxrandr libxinerama libxcb \
    xcb-util xcb-util-keysyms xcb-util-wm
```

### **Linux (Wayland)**
```bash
# Ubuntu/Debian
sudo apt install build-essential cmake pkg-config liblua5.4-dev \
    libwayland-dev wayland-protocols libwlroots-dev

# Fedora
sudo dnf install gcc-c++ cmake pkgconfig lua-devel \
    wayland-devel wayland-protocols-devel wlroots-devel

# Arch Linux
sudo pacman -S base-devel cmake pkgconf lua \
    wayland wayland-protocols wlroots
```

### **Windows**
- Visual Studio 2019 or later with C++17 support
- CMake 3.16 or later
- Lua 5.4 or later

### **macOS**
```bash
# Install dependencies via Homebrew
brew install cmake lua pkg-config

# Install Xcode Command Line Tools
xcode-select --install
```

## Installation

### **Quick Start (Linux/macOS)**
```bash
# Clone and bootstrap (deps + build + test + install)
git clone https://github.com/srdusr/srdwm.git
cd srdwm
bash scripts/bootstrap.sh --all

# X11-only build
# bash scripts/bootstrap.sh --all --no-wayland
# Real Wayland backend (requires wlroots deps)
# bash scripts/bootstrap.sh --all --real-wayland
```

### **Building from Source**

1. **Clone the repository**
```bash
git clone https://github.com/srdusr/srdwm.git
cd srdwm
```

2. **Create build directory**
```bash
mkdir build
cd build
```

3. **Configure and build**
```bash
cmake -S . -B build
cmake --build build -j$(nproc)
```

4. **Install**
```bash
sudo cmake --install build --prefix /usr/local
```

### **Platform-Specific Installation**

#### **Linux**
```bash
# Install to system
sudo cmake --install build --prefix /usr/local

# Or install to user directory
make install DESTDIR=$HOME/.local
```

#### **Windows**
```cmd
# Build with Visual Studio
cmake -G "Visual Studio 16 2019" -A x64 ..
cmake --build . --config Release

# Install
cmake --install . --prefix "C:\Program Files\SRDWM"
```

#### **macOS**
```bash
# Build and install
make -j$(sysctl -n hw.ncpu)
sudo make install
```

## Configuration

### **Basic Configuration**

Create your configuration file at `~/.config/srdwm/init.lua`:

```lua
-- Basic SRDWM configuration
print("Loading SRDWM configuration...")

-- Global settings
srd.set("general.decorations_enabled", true)
srd.set("general.border_width", 3)
srd.set("general.border_color", "#2e3440")

-- Layout settings
srd.set("general.default_layout", "dynamic")
srd.set("general.smart_placement", true)

-- Keybindings
srd.bind("Mod4+Return", function()
    -- Open terminal
    srd.spawn("alacritty")
end)

srd.bind("Mod4+q", function()
    -- Close focused window
    srd.window.close()
end)

srd.bind("Mod4+f", function()
    -- Toggle floating
    local window = srd.window.focused()
    if window then
        srd.window.toggle_floating(window.id)
    end
end)

-- Layout switching
srd.bind("Mod4+1", function() srd.layout.set("tiling") end)
srd.bind("Mod4+2", function() srd.layout.set("dynamic") end)
srd.bind("Mod4+3", function() srd.layout.set("floating") end)

print("Configuration loaded successfully!")
```

### **Platform-Specific Configuration**

#### **Linux X11**
```lua
-- X11-specific settings
if srd.get_platform() == "x11" then
    srd.set("general.border_width", 3)
    srd.set("general.decorations_enabled", true)
    
    -- X11-specific keybindings
    srd.bind("Mod4+x", function()
        local window = srd.window.focused()
        if window then
            srd.window.set_decorations(window.id, not srd.window.get_decorations(window.id))
        end
    end)
end
```

#### **Linux Wayland**
```lua
-- Wayland-specific settings
if srd.get_platform() == "wayland" then
    srd.set("general.border_width", 2)
    srd.set("general.decorations_enabled", true)
    
    -- Wayland-specific keybindings
    srd.bind("Mod4+w", function()
        local window = srd.window.focused()
        if window then
            srd.window.set_decorations(window.id, not srd.window.get_decorations(window.id))
        end
    end)
end
```

#### **Windows**
```lua
-- Windows-specific settings
if srd.get_platform() == "windows" then
    srd.set("general.border_width", 2)
    srd.set("general.decorations_enabled", true)
    
    -- Windows-specific keybindings
    srd.bind("Mod4+d", function()
        local window = srd.window.focused()
        if window then
            srd.window.set_border_color(window.id, 255, 0, 0) -- Red border
        end
    end)
end
```

#### **macOS**
```lua
-- macOS-specific settings
if srd.get_platform() == "macos" then
    srd.set("general.border_width", 1)
    srd.set("general.decorations_enabled", false) -- Limited support
    
    -- macOS-specific keybindings
    srd.bind("Mod4+m", function()
        print("macOS: Overlay window toggle requested")
    end)
end
```

## Usage

### **Starting SRDWM**

#### **Linux**
```bash
# X11
srdwm --platform x11

# Wayland
srdwm --platform wayland
```

#### **Windows**
```cmd
# Start from command line
srdwm.exe

# Or add to startup
```

#### **macOS**
```bash
# Start from command line
srdwm

# Or add to login items
```

### **Keybindings**

| Key | Action |
|-----|--------|
| `Mod4+Return` | Open terminal |
| `Mod4+q` | Close focused window |
| `Mod4+f` | Toggle floating |
| `Mod4+1/2/3` | Switch layouts (tiling/dynamic/floating) |
| `Mod4+b/n/r` | Change border colors (green/blue/red) |
| `Mod4+0` | Reset decorations |
| `Mod4+s` | Smart placement info |

### **Layouts**

- **Tiling**: Traditional tiling layout
- **Dynamic**: Smart placement with Windows 11-style algorithms
- **Floating**: Free-form window placement

## Testing

### **Running Tests**

```bash
# Build tests
cd build
make

# Run all tests
ctest

# Run specific tests
./tests/test_smart_placement
./tests/test_platform_factory
./tests/test_lua_manager
```

### **Platform-Specific Tests**

```bash
# Linux X11
./tests/test_x11_platform

# Linux Wayland
./tests/test_wayland_platform

# Windows
./tests/test_windows_platform

# macOS
./tests/test_macos_platform
```

### **One-command dependency installers**
```bash
# Ubuntu/Debian
bash scripts/install_deps_ubuntu.sh

# Fedora
bash scripts/install_deps_fedora.sh

# Arch Linux
bash scripts/install_deps_arch.sh

# openSUSE
bash scripts/install_deps_opensuse.sh

# Alpine Linux
bash scripts/install_deps_alpine.sh

# macOS (Homebrew)
bash scripts/install_deps_macos.sh
```

For full package lists per platform see `DEPENDENCIES.md`.

Wayland is enabled by default with a stub backend for compatibility. Switch to the real wlroots backend with `-DUSE_WAYLAND_STUB=OFF` (or `--real-wayland` in `scripts/bootstrap.sh`) once dependencies are installed.

## Documentation

- [API Documentation](docs/api.md)
- [Configuration Guide](docs/configuration.md)
- [Platform Implementation](docs/platforms.md)
- [Contributing Guide](CONTRIBUTING.md)

## Installed binary and desktop sessions

- The installed binary is named `srdwm` (a compatibility symlink `SRDWM` is also created in the same directory).
- Desktop session files are installed so you can pick SRDWM at login:
  - X11 session: `/usr/local/share/xsessions/srdwm.desktop`
  - Wayland session: `/usr/local/share/wayland-sessions/srdwm-wayland.desktop`
- If your display manager (GDM/SDDM/LightDM) doesn’t show them, ensure it reads sessions from `/usr/local/share/*sessions` or adjust your install prefix.

Install options (CMake variables):
- `-DSRDWM_INSTALL_X11_SESSION=ON|OFF` — install X11 session file (default ON)
- `-DSRDWM_INSTALL_WAYLAND_SESSION=ON|OFF` — install Wayland session file (default ON)

Example:
```bash
cmake -S . -B build -DSRDWM_INSTALL_WAYLAND_SESSION=OFF
cmake --build build -j
sudo cmake --install build --prefix /usr/local
```

## Using SRDWM from another CMake project

After installing SRDWM (e.g., `sudo cmake --install build --prefix /usr/local` on Linux/macOS), you can consume it in another CMake project via `find_package` and link to the exported target.

Minimal example `CMakeLists.txt`:

```cmake
cmake_minimum_required(VERSION 3.20)
project(MyApp LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(SRDWM REQUIRED) # provides SRDWM::SRDWM and pulls Lua automatically

add_executable(my_app main.cc)
target_link_libraries(my_app PRIVATE SRDWM::SRDWM)
```

Notes:
- On Linux/macOS, ensure the install prefix (default `/usr/local`) is in CMake’s package search path. You can hint it via `-DCMAKE_PREFIX_PATH=/usr/local` if needed.
- On Windows with vcpkg, pass your toolchain file when configuring your consumer project:
  ```powershell
  cmake -S . -B build -G Ninja -DCMAKE_TOOLCHAIN_FILE="C:/path/to/vcpkg/scripts/buildsystems/vcpkg.cmake"
  ```
- The exported package config internally requires Lua (handled by the SRDWM package config).

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details.

### **Development Setup**

```bash
# Clone with submodules
git clone --recursive https://github.com/srdusr/srdwm.git
cd srdwm

# Install development dependencies
sudo apt install build-essential cmake pkg-config liblua5.3-dev \
    libx11-dev libxrandr-dev libxinerama-dev libxcb-dev \
    libxcb-keysyms1-dev libxcb-icccm4-dev libwayland-dev \
    wayland-protocols libwlroots-dev libgtest-dev

# Build with tests
mkdir build && cd build
cmake -DBUILD_TESTS=ON ..
make -j$(nproc)

# Run tests
ctest
```

## Acknowledgments

- **Hyprland** for Wayland compositor inspiration
- **DWM** for X11 window management concepts
- **i3** for tiling layout ideas
- **Windows 11** for smart placement algorithms

## Support

- **Issues**: [GitHub Issues](https://github.com/srdusr/srdwm/issues)
- **Discussions**: [GitHub Discussions](https://github.com/srdusr/srdwm/discussions)
- **Wiki**: [GitHub Wiki](https://github.com/srdusr/srdwm/wiki)

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

