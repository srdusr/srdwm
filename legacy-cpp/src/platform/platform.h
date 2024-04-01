#ifndef SRDWM_PLATFORM_H
#define SRDWM_PLATFORM_H

#include <memory>
#include <vector>
#include <string>
#include <functional>

// Forward declarations
class SRDWindow;

// Include Monitor struct definition from layouts
#include "../layouts/layout.h"

// Platform-independent event types
enum class EventType {
    WindowCreated,
    WindowDestroyed,
    WindowMoved,
    WindowResized,
    WindowFocused,
    WindowUnfocused,
    KeyPress,
    KeyRelease,
    MouseButtonPress,
    MouseButtonRelease,
    MouseMotion,
    MonitorAdded,
    MonitorRemoved
};

// Event structure
struct Event {
    EventType type;
    void* data;
    size_t data_size;
};

// Platform abstraction interface
class Platform {
public:
    virtual ~Platform() = default;
    
    // Initialization and cleanup
    virtual bool initialize() = 0;
    virtual void shutdown() = 0;
    
    // Event handling
    virtual bool poll_events(std::vector<Event>& events) = 0;
    virtual void process_event(const Event& event) = 0;
    
    // Window management
    virtual std::unique_ptr<SRDWindow> create_window(const std::string& title, int x, int y, int width, int height) = 0;
    virtual void destroy_window(SRDWindow* window) = 0;
    virtual void set_window_position(SRDWindow* window, int x, int y) = 0;
    virtual void set_window_size(SRDWindow* window, int width, int height) = 0;
    virtual void set_window_title(SRDWindow* window, const std::string& title) = 0;
    virtual void focus_window(SRDWindow* window) = 0;
    virtual void minimize_window(SRDWindow* window) = 0;
    virtual void maximize_window(SRDWindow* window) = 0;
    virtual void close_window(SRDWindow* window) = 0;
    
    // Window decorations (cross-platform)
    virtual void set_window_decorations(SRDWindow* window, bool enabled) = 0;
    virtual void set_window_border_color(SRDWindow* window, int r, int g, int b) = 0;
    virtual void set_window_border_width(SRDWindow* window, int width) = 0;
    virtual bool get_window_decorations(SRDWindow* window) const = 0;
    
    // Monitor management
    virtual std::vector<Monitor> get_monitors() = 0;
    virtual Monitor get_primary_monitor() = 0;
    
    // Input handling
    virtual void grab_keyboard() = 0;
    virtual void ungrab_keyboard() = 0;
    virtual void grab_pointer() = 0;
    virtual void ungrab_pointer() = 0;
    
    // Utility
    virtual std::string get_platform_name() const = 0;
    virtual bool is_wayland() const = 0;
    virtual bool is_x11() const = 0;
    virtual bool is_windows() const = 0;
    virtual bool is_macos() const = 0;
};

// Forward declaration
class PlatformFactory;

#endif // SRDWM_PLATFORM_H

