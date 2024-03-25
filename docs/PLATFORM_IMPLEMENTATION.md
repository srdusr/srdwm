# SRDWM Platform Implementation Guide

## Overview
This document outlines the proper implementation approach for each platform, recognizing that **Wayland/XWayland is fundamentally different from X11** and requires completely different technologies and approaches.

## Platform Architecture Differences

### Linux: X11 vs Wayland
- **X11**: Traditional X11 window management with Xlib/XCB
- **Wayland**: Modern display protocol requiring wlroots or similar compositor framework
- **XWayland**: X11 applications running on Wayland (requires special handling)

### Windows vs macOS vs Linux
- **Windows**: Win32 API with global hooks and window subclassing
- **macOS**: Core Graphics/AppKit with accessibility APIs and event taps
- **Linux**: X11 or Wayland with different event systems

## Linux Implementation

### X11 Backend
```cpp
// X11-specific implementation using Xlib/XCB
class X11Platform : public Platform {
private:
    Display* display_;
    Window root_;
    std::map<Window, Window*> window_map_;
    
public:
    bool initialize() override {
        display_ = XOpenDisplay(nullptr);
        if (!display_) return false;
        
        root_ = DefaultRootWindow(display_);
        setup_event_handling();
        return true;
    }
    
    void setup_event_handling() {
        // X11 event masks and handlers
        XSelectInput(display_, root_, 
            SubstructureRedirectMask | SubstructureNotifyMask |
            KeyPressMask | KeyReleaseMask |
            ButtonPressMask | ButtonReleaseMask |
            PointerMotionMask);
    }
    
    bool poll_events(std::vector<Event>& events) override {
        XEvent xevent;
        while (XPending(display_)) {
            XNextEvent(display_, &xevent);
            convert_x11_event(xevent, events);
        }
        return true;
    }
    
    void convert_x11_event(const XEvent& xevent, std::vector<Event>& events) {
        switch (xevent.type) {
            case MapRequest:
                handle_map_request(xevent.xmaprequest);
                break;
            case ConfigureRequest:
                handle_configure_request(xevent.xconfigurerequest);
                break;
            case KeyPress:
                handle_key_press(xevent.xkey);
                break;
            // ... other event types
        }
    }
};
```

### Wayland Backend (using wlroots)
```cpp
// Wayland implementation using wlroots
class WaylandPlatform : public Platform {
private:
    struct wl_display* display_;
    struct wlroots_backend* backend_;
    struct wlroots_compositor* compositor_;
    struct wlroots_output* output_;
    struct wlroots_input_device* input_device_;
    
public:
    bool initialize() override {
        // Initialize wlroots backend
        backend_ = wlroots_backend_create();
        if (!backend_) return false;
        
        // Create compositor
        compositor_ = wlroots_compositor_create(backend_);
        if (!compositor_) return false;
        
        // Setup output and input
        setup_output();
        setup_input();
        
        return true;
    }
    
    void setup_output() {
        // Create and configure output
        output_ = wlroots_output_create(compositor_);
        wlroots_output_set_mode(output_, 1920, 1080, 60);
        wlroots_output_commit(output_);
    }
    
    void setup_input() {
        // Setup input devices
        input_device_ = wlroots_input_device_create(compositor_);
        wlroots_input_device_set_capabilities(input_device_, 
            WLROOTS_INPUT_DEVICE_CAP_KEYBOARD | 
            WLROOTS_INPUT_DEVICE_CAP_POINTER);
    }
    
    bool poll_events(std::vector<Event>& events) override {
        // wlroots event loop
        wlroots_backend_dispatch(backend_);
        
        // Process wlroots events
        struct wlroots_event* event;
        while ((event = wlroots_backend_get_event(backend_))) {
            convert_wlroots_event(event, events);
            wlroots_event_destroy(event);
        }
        
        return true;
    }
    
    void convert_wlroots_event(struct wlroots_event* event, std::vector<Event>& events) {
        switch (wlroots_event_get_type(event)) {
            case WLROOTS_EVENT_NEW_SURFACE:
                handle_new_surface(event);
                break;
            case WLROOTS_EVENT_SURFACE_COMMIT:
                handle_surface_commit(event);
                break;
            case WLROOTS_EVENT_KEYBOARD_KEY:
                handle_keyboard_key(event);
                break;
            // ... other event types
        }
    }
};
```

### XWayland Support
```cpp
// XWayland support for running X11 apps on Wayland
class XWaylandManager {
private:
    struct wlroots_xwayland* xwayland_;
    struct wlroots_xwayland_server* xwayland_server_;
    
public:
    bool initialize(struct wlroots_compositor* compositor) {
        // Create XWayland server
        xwayland_server_ = wlroots_xwayland_server_create(compositor);
        if (!xwayland_server_) return false;
        
        // Setup XWayland
        xwayland_ = wlroots_xwayland_create(xwayland_server_);
        if (!xwayland_) return false;
        
        return true;
    }
    
    void handle_xwayland_surface(struct wlroots_surface* surface) {
        // Handle X11 windows running on Wayland
        // These need special treatment for proper integration
    }
};
```

## Windows Implementation

### Win32 API Integration
```cpp
// Windows implementation using Win32 API
class WindowsPlatform : public Platform {
private:
    HINSTANCE h_instance_;
    std::map<HWND, Window*> window_map_;
    HHOOK keyboard_hook_;
    HHOOK mouse_hook_;
    
public:
    bool initialize() override {
        h_instance_ = GetModuleHandle(nullptr);
        
        // Register window class
        if (!register_window_class()) return false;
        
        // Setup global hooks
        setup_global_hooks();
        
        return true;
    }
    
    bool register_window_class() {
        WNDCLASSEX wc = {};
        wc.cbSize = sizeof(WNDCLASSEX);
        wc.lpfnWndProc = window_proc;
        wc.hInstance = h_instance_;
        wc.lpszClassName = L"SRDWM_Window";
        wc.hCursor = LoadCursor(nullptr, IDC_ARROW);
        
        return RegisterClassEx(&wc) != 0;
    }
    
    void setup_global_hooks() {
        // Global keyboard hook
        keyboard_hook_ = SetWindowsHookEx(WH_KEYBOARD_LL, 
            keyboard_proc, h_instance_, 0);
        
        // Global mouse hook
        mouse_hook_ = SetWindowsHookEx(WH_MOUSE_LL, 
            mouse_proc, h_instance_, 0);
    }
    
    static LRESULT CALLBACK window_proc(HWND hwnd, UINT msg, 
                                       WPARAM wparam, LPARAM lparam) {
        switch (msg) {
            case WM_CREATE:
                // Handle window creation
                break;
            case WM_DESTROY:
                // Handle window destruction
                break;
            case WM_SIZE:
                // Handle window resizing
                break;
            // ... other messages
        }
        return DefWindowProc(hwnd, msg, wparam, lparam);
    }
    
    static LRESULT CALLBACK keyboard_proc(int nCode, WPARAM wparam, LPARAM lparam) {
        if (nCode >= 0) {
            KBDLLHOOKSTRUCT* kbhs = (KBDLLHOOKSTRUCT*)lparam;
            // Handle global keyboard events
            handle_global_keyboard(wparam, kbhs);
        }
        return CallNextHookEx(nullptr, nCode, wparam, lparam);
    }
    
    static LRESULT CALLBACK mouse_proc(int nCode, WPARAM wparam, LPARAM lparam) {
        if (nCode >= 0) {
            MSLLHOOKSTRUCT* mhs = (MSLLHOOKSTRUCT*)lparam;
            // Handle global mouse events
            handle_global_mouse(wparam, mhs);
        }
        return CallNextHookEx(nullptr, nCode, wparam, lparam);
    }
};
```

## macOS Implementation

### Core Graphics/AppKit Integration
```cpp
// macOS implementation using Core Graphics and AppKit
class MacOSPlatform : public Platform {
private:
    CGEventTap event_tap_;
    std::map<CGWindowID, Window*> window_map_;
    
public:
    bool initialize() override {
        // Request accessibility permissions
        if (!request_accessibility_permissions()) return false;
        
        // Setup event tap
        setup_event_tap();
        
        // Setup window monitoring
        setup_window_monitoring();
        
        return true;
    }
    
    bool request_accessibility_permissions() {
        // Check if accessibility is enabled
        const void* keys[] = { kAXTrustedCheckOptionPrompt };
        const void* values[] = { kCFBooleanTrue };
        
        CFDictionaryRef options = CFDictionaryCreate(
            kCFAllocatorDefault, keys, values, 1, nullptr, nullptr);
        
        bool trusted = AXIsProcessTrustedWithOptions(options);
        CFRelease(options);
        
        return trusted;
    }
    
    void setup_event_tap() {
        // Create event tap for global events
        event_tap_ = CGEventTapCreate(
            kCGSessionEventTap,
            kCGHeadInsertEventTap,
            kCGEventTapOptionDefault,
            CGEventMaskBit(kCGEventKeyDown) | 
            CGEventMaskBit(kCGEventKeyUp) |
            CGEventMaskBit(kCGEventLeftMouseDown) |
            CGEventMaskBit(kCGEventLeftMouseUp) |
            CGEventMaskBit(kCGEventMouseMoved),
            event_tap_callback,
            this);
        
        if (event_tap_) {
            CFRunLoopSourceRef run_loop_source = 
                CFMachPortCreateRunLoopSource(kCFAllocatorDefault, event_tap_, 0);
            CFRunLoopAddSource(CFRunLoopGetCurrent(), run_loop_source, kCFRunLoopCommonModes);
            CGEventTapEnable(event_tap_, true);
        }
    }
    
    static CGEventRef event_tap_callback(CGEventTapProxy proxy, CGEventType type, 
                                        CGEventRef event, void* user_info) {
        MacOSPlatform* platform = static_cast<MacOSPlatform*>(user_info);
        return platform->handle_event_tap(proxy, type, event);
    }
    
    CGEventRef handle_event_tap(CGEventTapProxy proxy, CGEventType type, CGEventRef event) {
        switch (type) {
            case kCGEventKeyDown:
                handle_key_event(event, true);
                break;
            case kCGEventKeyUp:
                handle_key_event(event, false);
                break;
            case kCGEventLeftMouseDown:
                handle_mouse_event(event, true);
                break;
            case kCGEventLeftMouseUp:
                handle_mouse_event(event, false);
                break;
            case kCGEventMouseMoved:
                handle_mouse_motion(event);
                break;
        }
        return event;
    }
    
    void setup_window_monitoring() {
        // Monitor window creation/destruction
        CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | 
                                  kCGWindowListExcludeDesktopElements, 
                                  kCGNullWindowID);
    }
};
```

## Platform Detection and Selection

### Automatic Platform Detection
```cpp
// Platform factory with automatic detection
class PlatformFactory {
public:
    static std::unique_ptr<Platform> create_platform() {
        #ifdef _WIN32
            return std::make_unique<WindowsPlatform>();
        #elif defined(__APPLE__)
            return std::make_unique<MacOSPlatform>();
        #else
            // Linux: detect X11 vs Wayland
            return detect_linux_platform();
        #endif
    }
    
private:
    static std::unique_ptr<Platform> detect_linux_platform() {
        // Check environment variables
        const char* wayland_display = std::getenv("WAYLAND_DISPLAY");
        const char* xdg_session_type = std::getenv("XDG_SESSION_TYPE");
        
        if (wayland_display || (xdg_session_type && strcmp(xdg_session_type, "wayland") == 0)) {
            // Try Wayland first
            auto wayland_platform = std::make_unique<WaylandPlatform>();
            if (wayland_platform->initialize()) {
                std::cout << "Using Wayland backend" << std::endl;
                return wayland_platform;
            }
            std::cout << "Wayland initialization failed, falling back to X11" << std::endl;
        }
        
        // Fall back to X11
        auto x11_platform = std::make_unique<X11Platform>();
        if (x11_platform->initialize()) {
            std::cout << "Using X11 backend" << std::endl;
            return x11_platform;
        }
        
        std::cerr << "Failed to initialize any platform backend" << std::endl;
        return nullptr;
    }
};
```

## Dependencies and Build System

### CMake Configuration
```cmake
# Platform-specific dependencies
if(WIN32)
    # Windows dependencies
    find_package(PkgConfig REQUIRED)
    set(PLATFORM_LIBS user32 gdi32)
    
elseif(APPLE)
    # macOS dependencies
    find_library(COCOA_LIBRARY Cocoa)
    find_library(CARBON_LIBRARY Carbon)
    find_library(IOKIT_LIBRARY IOKit)
    set(PLATFORM_LIBS ${COCOA_LIBRARY} ${CARBON_LIBRARY} ${IOKIT_LIBRARY})
    
else()
    # Linux dependencies
    find_package(PkgConfig REQUIRED)
    
    # X11 dependencies
    pkg_check_modules(X11 REQUIRED x11 xcb xcb-keysyms)
    
    # Wayland dependencies (optional)
    pkg_check_modules(WAYLAND wayland-client wayland-cursor)
    pkg_check_modules(WLROOTS wlroots)
    
    if(WAYLAND_FOUND AND WLROOTS_FOUND)
        add_definitions(-DWAYLAND_ENABLED)
        set(PLATFORM_LIBS ${PLATFORM_LIBS} ${WAYLAND_LIBRARIES} ${WLROOTS_LIBRARIES})
    endif()
    
    set(PLATFORM_LIBS ${PLATFORM_LIBS} ${X11_LIBRARIES})
endif()
```

### Package Dependencies
```bash
# Ubuntu/Debian
sudo apt install libx11-dev libxcb1-dev libxcb-keysyms1-dev
sudo apt install libwayland-dev libwlroots-dev

# Arch Linux
sudo pacman -S xorg-server-devel wayland wlroots

# Fedora
sudo dnf install libX11-devel libxcb-devel wayland-devel wlroots-devel
```

## Event Handling Differences

### X11 Event System
```cpp
// X11 events are synchronous and direct
void X11Platform::handle_map_request(const XMapRequestEvent& event) {
    Window* window = create_window(event.window);
    if (window) {
        window_map_[event.window] = window;
        // X11 window is now managed
    }
}
```

### Wayland Event System
```cpp
// Wayland events are asynchronous and callback-based
void WaylandPlatform::handle_new_surface(struct wlroots_event* event) {
    struct wlroots_surface* surface = wlroots_event_get_surface(event);
    
    // Create window for new surface
    Window* window = create_window_from_surface(surface);
    if (window) {
        surface_window_map_[surface] = window;
    }
}
```

### Windows Event System
```cpp
// Windows uses message-based event system
LRESULT WindowsPlatform::window_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam) {
    switch (msg) {
        case WM_CREATE:
            // Window creation
            break;
        case WM_DESTROY:
            // Window destruction
            break;
    }
    return DefWindowProc(hwnd, msg, wparam, lparam);
}
```

### macOS Event System
```cpp
// macOS uses event taps and accessibility APIs
CGEventRef MacOSPlatform::handle_event_tap(CGEventTapProxy proxy, CGEventType type, CGEventRef event) {
    switch (type) {
        case kCGEventKeyDown:
            // Handle key press
            break;
        case kCGEventMouseMoved:
            // Handle mouse movement
            break;
    }
    return event;
}
```

## Window Management Differences

### X11 Window Management
```cpp
// X11: Direct window manipulation
void X11Platform::set_window_position(Window* window, int x, int y) {
    XMoveWindow(display_, window->get_x11_handle(), x, y);
}

void X11Platform::set_window_size(Window* window, int width, int height) {
    XResizeWindow(display_, window->get_x11_handle(), width, height);
}
```

### Wayland Window Management
```cpp
// Wayland: Surface-based management
void WaylandPlatform::set_window_position(Window* window, int x, int y) {
    struct wlroots_surface* surface = window->get_wayland_surface();
    wlroots_surface_set_position(surface, x, y);
}

void WaylandPlatform::set_window_size(Window* window, int width, int height) {
    struct wlroots_surface* surface = window->get_wayland_surface();
    wlroots_surface_set_size(surface, width, height);
}
```

### Windows Window Management
```cpp
// Windows: Win32 API calls
void WindowsPlatform::set_window_position(Window* window, int x, int y) {
    HWND hwnd = window->get_win32_handle();
    SetWindowPos(hwnd, nullptr, x, y, 0, 0, 
                 SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
}

void WindowsPlatform::set_window_size(Window* window, int width, int height) {
    HWND hwnd = window->get_win32_handle();
    SetWindowPos(hwnd, nullptr, 0, 0, width, height, 
                 SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
}
```

### macOS Window Management
```cpp
// macOS: Core Graphics API calls
void MacOSPlatform::set_window_position(Window* window, int x, int y) {
    CGWindowID window_id = window->get_macos_window_id();
    CGPoint position = CGPointMake(x, y);
    
    // Use accessibility APIs to move window
    AXUIElementRef element = AXUIElementCreateApplication(
        window->get_macos_pid());
    
    if (element) {
        AXUIElementSetAttributeValue(element, kAXPositionAttribute, &position);
        CFRelease(element);
    }
}
```

## Testing and Validation

### Platform-Specific Testing
```cpp
// Test each platform independently
class PlatformTest {
public:
    static void test_x11_platform() {
        auto platform = std::make_unique<X11Platform>();
        assert(platform->initialize());
        // Test X11-specific functionality
    }
    
    static void test_wayland_platform() {
        auto platform = std::make_unique<WaylandPlatform>();
        assert(platform->initialize());
        // Test Wayland-specific functionality
    }
    
    static void test_windows_platform() {
        auto platform = std::make_unique<WindowsPlatform>();
        assert(platform->initialize());
        // Test Windows-specific functionality
    }
    
    static void test_macos_platform() {
        auto platform = std::make_unique<MacOSPlatform>();
        assert(platform->initialize());
        // Test macOS-specific functionality
    }
};
```

## Best Practices

### 1. **Platform Abstraction**
- Keep platform-specific code isolated
- Use common interfaces for cross-platform functionality
- Implement platform detection automatically

### 2. **Wayland vs X11**
- **Never mix X11 and Wayland APIs**
- Use wlroots for Wayland (don't implement from scratch)
- Handle XWayland as a special case within Wayland

### 3. **Event Handling**
- Respect each platform's event model
- Don't force synchronous behavior on asynchronous platforms
- Handle platform-specific quirks gracefully

### 4. **Window Management**
- Use platform-native APIs for best performance
- Don't try to emulate one platform's behavior on another
- Handle platform-specific window states properly

### 5. **Testing**
- Test each platform independently
- Use CI/CD with multiple platform targets
- Validate platform-specific features thoroughly

This implementation approach ensures that SRDWM works correctly on each platform while respecting the fundamental differences between X11, Wayland, Windows, and macOS.


