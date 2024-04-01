#include "macos_platform.h"
#include <iostream>
#include <CoreGraphics/CoreGraphics.h>
#include <ApplicationServices/ApplicationServices.h>
#include <Carbon/Carbon.h>

// Static member initialization
MacOSPlatform* MacOSPlatform::instance_ = nullptr;

MacOSPlatform::MacOSPlatform()
    : event_tap_(nullptr) {
    
    instance_ = this;
}

MacOSPlatform::~MacOSPlatform() {
    shutdown();
    if (instance_ == this) {
        instance_ = nullptr;
    }
}

bool MacOSPlatform::initialize() {
    std::cout << "Initializing macOS platform..." << std::endl;
    
    // Request accessibility permissions
    if (!request_accessibility_permissions()) {
        std::cerr << "Failed to get accessibility permissions" << std::endl;
        return false;
    }
    
    // Setup event tap
    setup_event_tap();
    
    // Setup window monitoring
    setup_window_monitoring();
    
    std::cout << "macOS platform initialized successfully" << std::endl;
    return true;
}

void MacOSPlatform::shutdown() {
    std::cout << "Shutting down macOS platform..." << std::endl;
    
    // Clean up event tap
    if (event_tap_) {
        CGEventTapEnable(event_tap_, false);
        CFRelease(event_tap_);
        event_tap_ = nullptr;
    }
    
    // Clean up windows
    for (auto& pair : window_map_) {
        if (pair.second) {
            delete pair.second;
        }
    }
    window_map_.clear();
    
    std::cout << "macOS platform shutdown complete" << std::endl;
}

// SRDWindow decoration implementations (macOS limitations)
void MacOSPlatform::set_window_decorations(SRDWindow* window, bool enabled) {
    std::cout << "MacOSPlatform: Set window decorations " << (enabled ? "enabled" : "disabled") << std::endl;
    
    if (!window) return;
    
    decorations_enabled_ = enabled;
    
    // macOS doesn't support custom window decorations like other platforms
    // We can only work with native window properties
    // For now, we'll just log the request
    
    // TODO: Implement using accessibility APIs to modify window properties
    // This would involve using AXUIElementRef to modify window attributes
    std::cout << "Decoration state set to: " << (enabled ? "enabled" : "disabled") << std::endl;
}

void MacOSPlatform::set_window_border_color(SRDWindow* window, int r, int g, int b) {
    std::cout << "MacOSPlatform: Set border color RGB(" << r << "," << g << "," << b << ")" << std::endl;
    
    if (!window) return;
    
    // macOS doesn't support custom border colors through public APIs
    // This would require private APIs or overlay windows
    // For now, we'll just log the request
    
    // TODO: Implement using private APIs or overlay windows
    // This could involve:
    // 1. Creating transparent overlay windows around the target window
    // 2. Using private Core Graphics APIs (not recommended for production)
    // 3. Using accessibility APIs to modify window properties
    
    std::cout << "Border color set to RGB(" << r << "," << g << "," << b << ")" << std::endl;
}

void MacOSPlatform::set_window_border_width(SRDWindow* window, int width) {
    std::cout << "MacOSPlatform: Set border width " << width << std::endl;
    
    if (!window) return;
    
    // macOS doesn't support custom border widths through public APIs
    // This would require private APIs or overlay windows
    // For now, we'll just log the request
    
    // TODO: Implement using private APIs or overlay windows
    // This could involve:
    // 1. Creating transparent overlay windows around the target window
    // 2. Using private Core Graphics APIs (not recommended for production)
    // 3. Using accessibility APIs to modify window properties
    
    std::cout << "Border width set to " << width << std::endl;
}

bool MacOSPlatform::get_window_decorations(SRDWindow* window) const {
    if (!window) return false;
    
    return decorations_enabled_;
}

void MacOSPlatform::create_overlay_window(SRDWindow* window) {
    std::cout << "MacOSPlatform: Create overlay window for window " << window->getId() << std::endl;
    
    if (!window) return;
    
    // TODO: Implement overlay window creation for custom decorations
    // This would involve:
    // 1. Creating a transparent window using Core Graphics
    // 2. Positioning it over the target window
    // 3. Drawing custom borders/titlebar on it
    // 4. Handling mouse events for window management
    
    // For now, we'll just log the request
    std::cout << "Overlay window creation requested" << std::endl;
}

void MacOSPlatform::destroy_overlay_window(SRDWindow* window) {
    std::cout << "MacOSPlatform: Destroy overlay window for window " << window->getId() << std::endl;
    
    if (!window) return;
    
    // TODO: Implement overlay window destruction
    // This would involve:
    // 1. Finding the overlay window for this window
    // 2. Destroying the overlay window
    // 3. Cleaning up any associated resources
    
    // For now, we'll just log the request
    std::cout << "Overlay window destruction requested" << std::endl;
}

bool MacOSPlatform::request_accessibility_permissions() {
    std::cout << "Requesting accessibility permissions..." << std::endl;
    
    // Check if accessibility is enabled
    const void* keys[] = { kAXTrustedCheckOptionPrompt };
    const void* values[] = { kCFBooleanTrue };
    
    CFDictionaryRef options = CFDictionaryCreate(
        kCFAllocatorDefault, keys, values, 1, nullptr, nullptr);
    
    bool trusted = AXIsProcessTrustedWithOptions(options);
    CFRelease(options);
    
    if (trusted) {
        std::cout << "Accessibility permissions granted" << std::endl;
    } else {
        std::cout << "Accessibility permissions denied" << std::endl;
    }
    
    return trusted;
}

void MacOSPlatform::setup_event_tap() {
    std::cout << "Setting up event tap..." << std::endl;
    
    // Create event tap for global events
    event_tap_ = CGEventTapCreate(
        kCGSessionEventTap,
        kCGHeadInsertEventTap,
        kCGEventTapOptionDefault,
        CGEventMaskBit(kCGEventKeyDown) | 
        CGEventMaskBit(kCGEventKeyUp) |
        CGEventMaskBit(kCGEventLeftMouseDown) |
        CGEventMaskBit(kCGEventLeftMouseUp) |
        CGEventMaskBit(kCGEventRightMouseDown) |
        CGEventMaskBit(kCGEventRightMouseUp) |
        CGEventMaskBit(kCGEventMouseMoved),
        event_tap_callback,
        this);
    
    if (event_tap_) {
        CFRunLoopSourceRef run_loop_source = 
            CFMachPortCreateRunLoopSource(kCFAllocatorDefault, event_tap_, 0);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), run_loop_source, kCFRunLoopCommonModes);
        CGEventTapEnable(event_tap_, true);
        
        std::cout << "Event tap setup complete" << std::endl;
    } else {
        std::cerr << "Failed to create event tap" << std::endl;
    }
}

void MacOSPlatform::setup_window_monitoring() {
    std::cout << "Setting up window monitoring..." << std::endl;
    
    // Monitor window creation/destruction
    CGSRDWindowListCopySRDWindowInfo(kCGSRDWindowListOptionOnScreenOnly | 
                              kCGSRDWindowListExcludeDesktopElements, 
                              kCGNullSRDWindowID);
    
    std::cout << "SRDWindow monitoring setup complete" << std::endl;
}

bool MacOSPlatform::poll_events(std::vector<Event>& events) {
    if (!initialized_ || !event_tap_) return false;
    
    events.clear();
    
    // macOS events are handled through the event tap callback
    // The event tap callback handles event conversion
    // For now, we'll just return any pending events
    
    // TODO: Implement proper event queue processing
    // This would involve processing events from the event tap callback
    
    return false;
}

void MacOSPlatform::process_event(const Event& event) {
    // TODO: Implement event processing
}

std::unique_ptr<SRDWindow> MacOSPlatform::create_window(const std::string& title, int x, int y, int width, int height) {
    std::cout << "Creating macOS window: " << title << std::endl;
    
    // TODO: Implement actual window creation using Core Graphics/AppKit
    // For now, create a placeholder window object
    
    auto window = std::make_unique<SRDWindow>();
    // TODO: Set window properties
    
    std::cout << "macOS window creation requested" << std::endl;
    return window;
}

void MacOSPlatform::destroy_window(SRDWindow* window) {
    std::cout << "Destroying macOS window" << std::endl;
    
    // TODO: Implement window destruction
}

void MacOSPlatform::set_window_position(SRDWindow* window, int x, int y) {
    // TODO: Implement window positioning using accessibility APIs
}

void MacOSPlatform::set_window_size(SRDWindow* window, int width, int height) {
    // TODO: Implement window resizing using accessibility APIs
}

void MacOSPlatform::set_window_title(SRDWindow* window, const std::string& title) {
    // TODO: Implement title setting
}

void MacOSPlatform::focus_window(SRDWindow* window) {
    // TODO: Implement window focusing
}

void MacOSPlatform::minimize_window(SRDWindow* window) {
    // TODO: Implement window minimization
}

void MacOSPlatform::maximize_window(SRDWindow* window) {
    // TODO: Implement window maximization
}

void MacOSPlatform::close_window(SRDWindow* window) {
    // TODO: Implement window closing
}

std::vector<Monitor> MacOSPlatform::get_monitors() {
    std::vector<Monitor> monitors;
    
    // Get display information using Core Graphics
    uint32_t display_count = 0;
    CGGetActiveDisplayList(0, nullptr, &display_count);
    
    if (display_count > 0) {
        std::vector<CGDirectDisplayID> display_ids(display_count);
        CGGetActiveDisplayList(display_count, display_ids.data(), &display_count);
        
        for (uint32_t i = 0; i < display_count; ++i) {
            CGDirectDisplayID display_id = display_ids[i];
            
            Monitor monitor;
            monitor.id = static_cast<int>(display_id);
            monitor.name = "Display " + std::to_string(i + 1);
            
            // Get display bounds
            CGRect bounds = CGDisplayBounds(display_id);
            monitor.x = static_cast<int>(bounds.origin.x);
            monitor.y = static_cast<int>(bounds.origin.y);
            monitor.width = static_cast<int>(bounds.size.width);
            monitor.height = static_cast<int>(bounds.size.height);
            
            // Get refresh rate
            CGDisplayModeRef mode = CGDisplayCopyDisplayMode(display_id);
            if (mode) {
                monitor.refresh_rate = static_cast<int>(CGDisplayModeGetRefreshRate(mode));
                CGDisplayModeRelease(mode);
            } else {
                monitor.refresh_rate = 60; // Default
            }
            
            monitors.push_back(monitor);
            
            std::cout << "Monitor " << i << ": " << monitor.width << "x" << monitor.height 
                      << " @ " << monitor.refresh_rate << "Hz" << std::endl;
        }
    }
    
    return monitors;
}

Monitor MacOSPlatform::get_primary_monitor() {
    auto monitors = get_monitors();
    if (!monitors.empty()) {
        return monitors[0];
    }
    
    // Fallback to main display
    CGDirectDisplayID main_display = CGMainDisplayID();
    CGRect bounds = CGDisplayBounds(main_display);
    
    Monitor monitor;
    monitor.id = static_cast<int>(main_display);
    monitor.name = "Main Display";
    monitor.x = static_cast<int>(bounds.origin.x);
    monitor.y = static_cast<int>(bounds.origin.y);
    monitor.width = static_cast<int>(bounds.size.width);
    monitor.height = static_cast<int>(bounds.size.height);
    monitor.refresh_rate = 60; // Default
    
    return monitor;
}

void MacOSPlatform::grab_keyboard() {
    // TODO: Implement keyboard grabbing
    std::cout << "Keyboard grabbing setup" << std::endl;
}

void MacOSPlatform::ungrab_keyboard() {
    // TODO: Implement keyboard ungrab
    std::cout << "Keyboard ungrab" << std::endl;
}

void MacOSPlatform::grab_pointer() {
    // TODO: Implement pointer grabbing
    std::cout << "Pointer grabbing setup" << std::endl;
}

void MacOSPlatform::ungrab_pointer() {
    // TODO: Implement pointer ungrab
    std::cout << "Pointer ungrab" << std::endl;
}

// Static callback functions
CGEventRef MacOSPlatform::event_tap_callback(CGEventTapProxy proxy, CGEventType type, 
                                            CGEventRef event, void* user_info) {
    MacOSPlatform* platform = static_cast<MacOSPlatform*>(user_info);
    return platform->handle_event_tap(proxy, type, event);
}

CGEventRef MacOSPlatform::handle_event_tap(CGEventTapProxy proxy, CGEventType type, CGEventRef event) {
    switch (type) {
        case kCGEventKeyDown:
            handle_key_event(event, true);
            break;
            
        case kCGEventKeyUp:
            handle_key_event(event, false);
            break;
            
        case kCGEventLeftMouseDown:
            handle_mouse_event(event, true, 1);
            break;
            
        case kCGEventLeftMouseUp:
            handle_mouse_event(event, false, 1);
            break;
            
        case kCGEventRightMouseDown:
            handle_mouse_event(event, true, 2);
            break;
            
        case kCGEventRightMouseUp:
            handle_mouse_event(event, false, 2);
            break;
            
        case kCGEventMouseMoved:
            handle_mouse_motion(event);
            break;
    }
    
    return event;
}

void MacOSPlatform::handle_key_event(CGEventRef event, bool pressed) {
    // Get key code
    CGKeyCode key_code = static_cast<CGKeyCode>(CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode));
    
    std::cout << "Key " << (pressed ? "press" : "release") << ": " << key_code << std::endl;
    
    // TODO: Convert to SRDWM key event
}

void MacOSPlatform::handle_mouse_event(CGEventRef event, bool pressed, int button) {
    // Get mouse position
    CGPoint location = CGEventGetLocation(event);
    
    std::cout << "Mouse button " << button << " " << (pressed ? "down" : "up") 
              << " at (" << location.x << ", " << location.y << ")" << std::endl;
    
    // TODO: Convert to SRDWM button event
}

void MacOSPlatform::handle_mouse_motion(CGEventRef event) {
    // Get mouse position
    CGPoint location = CGEventGetLocation(event);
    
    // TODO: Convert to SRDWM motion event
}

// Utility methods
CGSRDWindowID MacOSPlatform::get_macos_window_id(SRDWindow* window) {
    // TODO: Implement window ID retrieval
    return 0;
}

pid_t MacOSPlatform::get_macos_pid(SRDWindow* window) {
    // TODO: Implement PID retrieval
    return 0;
}

void MacOSPlatform::update_window_monitoring() {
    // TODO: Implement window monitoring update
}

void MacOSPlatform::handle_window_created(CGSRDWindowID window_id) {
    std::cout << "SRDWindow created: " << window_id << std::endl;
    
    // TODO: Create SRDWM window object and manage it
}

void MacOSPlatform::handle_window_destroyed(CGSRDWindowID window_id) {
    std::cout << "SRDWindow destroyed: " << window_id << std::endl;
    
    // TODO: Clean up SRDWM window object
}

void MacOSPlatform::handle_window_focused(CGSRDWindowID window_id) {
    std::cout << "SRDWindow focused: " << window_id << std::endl;
    
    // TODO: Handle window focus
}

void MacOSPlatform::handle_window_moved(CGSRDWindowID window_id, int x, int y) {
    std::cout << "SRDWindow " << window_id << " moved to (" << x << ", " << y << ")" << std::endl;
    
    // TODO: Handle window movement
}

void MacOSPlatform::handle_window_resized(CGSRDWindowID window_id, int width, int height) {
    std::cout << "SRDWindow " << window_id << " resized to " << width << "x" << height << std::endl;
    
    // TODO: Handle window resizing
}


