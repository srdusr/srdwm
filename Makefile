# SRDWM Makefile
# Builds the project without requiring cmake

CXX = g++
CXXFLAGS = -std=c++17 -Wall -Wextra -Wpedantic -fPIC
INCLUDES = -Isrc -Isrc/core -Isrc/layouts -Isrc/input -Isrc/platform -Isrc/config -Isrc/utils

# Platform detection
UNAME_S := $(shell uname -s)
UNAME_M := $(shell uname -m)

# Platform-specific flags and libraries
ifeq ($(UNAME_S),Linux)
    PLATFORM = LINUX_PLATFORM
    CXXFLAGS += -DLINUX_PLATFORM
    
    # Check for Wayland support
    ifeq ($(shell pkg-config --exists wayland-client 2>/dev/null && echo yes),yes)
        ifeq ($(shell pkg-config --exists wlroots 2>/dev/null && echo yes),yes)
            CXXFLAGS += -DWAYLAND_ENABLED
            WAYLAND_LIBS = $(shell pkg-config --libs wayland-client wlroots 2>/dev/null)
            WAYLAND_CFLAGS = $(shell pkg-config --cflags wayland-client wlroots 2>/dev/null)
        endif
    endif
    
    # X11 libraries (always available on Linux)
    X11_LIBS = -lX11 -lXrandr -lXinerama -lXfixes -lXcursor
    X11_CFLAGS = $(shell pkg-config --cflags x11 xrandr xinerama xfixes xcursor 2>/dev/null || echo "")
    
else ifeq ($(UNAME_S),Darwin)
    PLATFORM = MACOS_PLATFORM
    CXXFLAGS += -DMACOS_PLATFORM
    LIBS = -framework Cocoa -framework Carbon -framework IOKit
    
else ifeq ($(UNAME_S),MINGW32_NT-10.0)
    PLATFORM = WIN32_PLATFORM
    CXXFLAGS += -DWIN32_PLATFORM
    LIBS = -luser32 -lgdi32 -ldwmapi
    
else ifeq ($(UNAME_S),MINGW64_NT-10.0)
    PLATFORM = WIN32_PLATFORM
    CXXFLAGS += -DWIN32_PLATFORM
    LIBS = -luser32 -lgdi32 -ldwmapi
    
else ifeq ($(UNAME_S),MSYS_NT-10.0)
    PLATFORM = WIN32_PLATFORM
    CXXFLAGS += -DWIN32_PLATFORM
    LIBS = -luser32 -lgdi32 -ldwmapi
    
else
    PLATFORM = UNKNOWN
    $(warning Unknown platform: $(UNAME_S))
endif

# Lua support (optional)
LUA_AVAILABLE := $(shell ldconfig -p | grep -q liblua5.4 && echo yes)
ifeq ($(LUA_AVAILABLE),yes)
    # Check for Lua headers
    LUA_HEADERS := $(shell find /usr/include -name "lua.h" 2>/dev/null | head -1)
    ifneq ($(LUA_HEADERS),)
        CXXFLAGS += -DLUA_ENABLED
        LUA_LIBS = -llua5.4
        LUA_CFLAGS = -I$(dir $(LUA_HEADERS))
        LUA_SOURCES = src/config/lua_manager.cc
        $(info Lua support enabled)
    else
        $(warning Lua runtime found but headers not found - Lua support disabled)
        LUA_SOURCES = 
    endif
else
    $(warning Lua not found - Lua support disabled)
    LUA_SOURCES = 
endif

# Source files
SOURCES = \
    src/main.cc \
    src/core/window.cc \
    src/core/window_manager.cc \
    src/layouts/layout_engine.cc \
    src/layouts/dynamic_layout.cc \
    src/layouts/tiling_layout.cc \
    src/layouts/smart_placement.cc \
    src/platform/platform_factory.cc

# Add Lua sources if available
ifneq ($(LUA_SOURCES),)
    SOURCES += $(LUA_SOURCES)
endif

# Platform-specific source files
ifeq ($(PLATFORM),LINUX_PLATFORM)
    SOURCES += src/platform/x11_platform.cc
    ifeq ($(shell pkg-config --exists wlroots 2>/dev/null && echo yes),yes)
        SOURCES += src/platform/wayland_platform.cc
    endif
else ifeq ($(PLATFORM),WIN32_PLATFORM)
    SOURCES += src/platform/windows_platform.cc
else ifeq ($(PLATFORM),MACOS_PLATFORM)
    SOURCES += src/platform/macos_platform.cc
endif

# Object files
OBJECTS = $(SOURCES:.cc=.o)

# Target executable
TARGET = srdwm

# Default target
all: $(TARGET)

# Build the executable
$(TARGET): $(OBJECTS)
	@echo "Linking $(TARGET)..."
	$(CXX) $(OBJECTS) -o $(TARGET) $(LIBS) $(X11_LIBS) $(WAYLAND_LIBS) $(LUA_LIBS)
	@echo "Build complete: $(TARGET)"

# Compile source files
%.o: %.cc
	@echo "Compiling $<..."
	$(CXX) $(CXXFLAGS) $(INCLUDES) $(X11_CFLAGS) $(WAYLAND_CFLAGS) $(LUA_CFLAGS) -c $< -o $@

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	rm -f $(OBJECTS) $(TARGET)
	@echo "Clean complete"

# Install (requires sudo)
install: $(TARGET)
	@echo "Installing $(TARGET)..."
	sudo cp $(TARGET) /usr/local/bin/
	sudo mkdir -p /usr/local/share/srdwm/config
	sudo cp -r config/* /usr/local/share/srdwm/config/
	sudo mkdir -p /usr/local/share/srdwm/docs
	sudo cp docs/*.md /usr/local/share/srdwm/docs/
	@echo "Installation complete"

# Uninstall (requires sudo)
uninstall:
	@echo "Uninstalling $(TARGET)..."
	sudo rm -f /usr/local/bin/$(TARGET)
	sudo rm -rf /usr/local/share/srdwm
	@echo "Uninstallation complete"

# Show build information
info:
	@echo "=== SRDWM Build Information ==="
	@echo "Platform: $(PLATFORM)"
	@echo "Compiler: $(CXX)"
	@echo "C++ Standard: C++17"
	@echo "Architecture: $(UNAME_M)"
	@echo "Sources: $(words $(SOURCES)) files"
	@echo "Target: $(TARGET)"
	@echo "Lua Support: $(if $(LUA_SOURCES),Enabled,Disabled)"
	@echo "================================"

# Show platform-specific information
platform-info:
	@echo "=== Platform Information ==="
	@echo "Platform: $(PLATFORM)"
ifeq ($(PLATFORM),LINUX_PLATFORM)
	@echo "Linux Platform: Enabled"
	@echo "X11 Support: Enabled"
ifeq ($(shell pkg-config --exists wlroots 2>/dev/null && echo yes),yes)
	@echo "Wayland Support: Enabled"
else
	@echo "Wayland Support: Disabled (wlroots not found)"
endif
else ifeq ($(PLATFORM),WIN32_PLATFORM)
	@echo "Windows Platform: Enabled"
else ifeq ($(PLATFORM),MACOS_PLATFORM)
	@echo "macOS Platform: Enabled"
endif
	@echo "============================="

# Dependencies
deps:
	@echo "=== Dependencies ==="
ifeq ($(PLATFORM),LINUX_PLATFORM)
	@echo "Checking Linux dependencies..."
	@echo "X11 libraries:"
	@echo "  - libx11-dev"
	@echo "  - libxrandr-dev"
	@echo "  - libxinerama-dev"
	@echo "  - libxfixes-dev"
	@echo "  - libxcursor-dev"
ifeq ($(shell pkg-config --exists wlroots 2>/dev/null && echo yes),yes)
	@echo "Wayland libraries:"
	@echo "  - libwayland-dev"
	@echo "  - libwlroots-dev"
else
	@echo "Wayland libraries: Not available"
endif
	@echo "Lua: $(if $(LUA_SOURCES),Available,Not available)"
else ifeq ($(PLATFORM),WIN32_PLATFORM)
	@echo "Windows dependencies:"
	@echo "  - MinGW-w64"
	@echo "  - Lua for Windows"
else ifeq ($(PLATFORM),MACOS_PLATFORM)
	@echo "macOS dependencies:"
	@echo "  - Xcode Command Line Tools"
	@echo "  - Lua (via Homebrew: brew install lua)"
endif
	@echo "==================="

# Help
help:
	@echo "=== SRDWM Makefile Help ==="
	@echo "Available targets:"
	@echo "  all          - Build the project (default)"
	@echo "  clean        - Remove build artifacts"
	@echo "  install      - Install to system (requires sudo)"
	@echo "  uninstall    - Remove from system (requires sudo)"
	@echo "  info         - Show build information"
	@echo "  platform-info - Show platform-specific information"
	@echo "  deps         - Show dependency information"
	@echo "  help         - Show this help"
	@echo ""
	@echo "Environment variables:"
	@echo "  CXX          - C++ compiler (default: g++)"
	@echo "  CXXFLAGS     - Additional compiler flags"
	@echo "  LIBS         - Additional libraries"
	@echo "========================"

.PHONY: all clean install uninstall info platform-info deps help
