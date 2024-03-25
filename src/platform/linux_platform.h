#ifndef SRDWM_LINUX_PLATFORM_H
#define SRDWM_LINUX_PLATFORM_H

#include "platform.h"
#include <memory>
#include <string>

// Forward declarations for X11
#ifdef __linux__
struct _XDisplay;
typedef struct _XDisplay Display;
typedef unsigned long SRDWindow;
typedef unsigned long Atom;

// Forward declarations for Wayland
struct wl_display;
struct wl_registry;
struct wl_compositor;
struct wl_shell;
struct wl_seat;
struct wl_keyboard;
struct wl_pointer;
#endif

class LinuxPlatform : public Platform {
public:
    enum class Backend {
        Auto,
        X11,
        Wayland
    };

    explicit LinuxPlatform(Backend backend = Backend::Auto);
    ~LinuxPlatform() override;

    // Platform interface implementation
    bool initialize() override;
    void shutdown() override;
    
    bool poll_events(std::vector<Event>& events) override;
    void process_event(const Event& event) override;
    
    std::unique_ptr<SRDWindow> create_window(const std::string& title, int x, int y, int width, int height) override;
    void destroy_window(SRDWindow* window) override;
    void set_window_position(SRDWindow* window, int x, int y) override;
    void set_window_size(SRDWindow* window, int width, int height) override;
    void set_window_title(SRDWindow* window, const std::string& title) override;
    void focus_window(SRDWindow* window) override;
    void minimize_window(SRDWindow* window) override;
    void maximize_window(SRDWindow* window) override;
    void close_window(SRDWindow* window) override;
    
    std::vector<Monitor> get_monitors() override;
    Monitor get_primary_monitor() override;
    
    void grab_keyboard() override;
    void ungrab_keyboard() override;
    void grab_pointer() override;
    void ungrab_pointer() override;
    
    std::string get_platform_name() const override;
    bool is_wayland() const override;
    bool is_x11() const override;
    bool is_windows() const override;
    bool is_macos() const override;

private:
    Backend backend_;
    bool initialized_;
    
    // X11 specific members
    Display* x11_display_;
    SRDWindow x11_root_;
    Atom x11_wm_delete_window_;
    Atom x11_wm_protocols_;
    
    // Wayland specific members
    wl_display* wayland_display_;
    wl_registry* wayland_registry_;
    wl_compositor* wayland_compositor_;
    wl_shell* wayland_shell_;
    wl_seat* wayland_seat_;
    wl_keyboard* wayland_keyboard_;
    wl_pointer* wayland_pointer_;
    
    // Common members
    std::vector<Monitor> monitors_;
    
    // Private methods
    bool initialize_x11();
    bool initialize_wayland();
    void shutdown_x11();
    void shutdown_wayland();
    
    bool detect_backend();
    void setup_x11_atoms();
    void setup_wayland_registry();
    
    // Event processing
    void process_x11_event(XEvent& event, std::vector<Event>& events);
    void process_wayland_event(wl_display* display, std::vector<Event>& events);
};

#endif // SRDWM_LINUX_PLATFORM_H

