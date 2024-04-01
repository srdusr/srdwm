#include "wayland_platform.h"
#include <iostream>
#include <cstring>
#include <cstdint>

#ifndef USE_WAYLAND_STUB
#include <wayland-server-core.h>
extern "C" {
  // Minimal wlroots declarations to avoid pulling C99-only headers into C++
  struct wlr_backend;
  struct wlr_renderer;
  struct wlr_compositor;
  struct wlr_seat;
  struct wlr_xdg_shell;

  // Logging
  int wlr_log_init(int verbosity, void* callback);

  // Backend
  struct wlr_backend* wlr_backend_autocreate(struct wl_display* display, void* session);
  bool wlr_backend_start(struct wlr_backend* backend);
  void wlr_backend_destroy(struct wlr_backend* backend);
  struct wlr_renderer* wlr_backend_get_renderer(struct wlr_backend* backend);

  // Compositor and seat
  struct wlr_compositor* wlr_compositor_create(struct wl_display* display, uint32_t version, struct wlr_renderer* renderer);
  struct wlr_seat* wlr_seat_create(struct wl_display* display, const char* name);

  // xdg-shell
  struct wlr_xdg_shell* wlr_xdg_shell_create(struct wl_display* display, uint32_t version);
  struct wlr_renderer* wlr_renderer_autocreate(struct wlr_backend* backend);
  void wlr_renderer_init_wl_display(struct wlr_renderer* renderer, struct wl_display* display);

  // Wayland display helpers (for C++ compilation)
  struct wl_display* wl_display_create(void);
  void wl_display_destroy(struct wl_display* display);
  int wl_display_dispatch_pending(struct wl_display* display);
  void wl_display_flush_clients(struct wl_display* display);
}
#endif

// Static member initialization
WaylandPlatform* WaylandPlatform::instance_ = nullptr;

WaylandPlatform::WaylandPlatform()
    : display_(nullptr)
    , registry_(nullptr)
    , compositor_(nullptr)
    , shm_(nullptr)
    , seat_(nullptr)
    , output_(nullptr)
    , shell_(nullptr)
    , backend_(nullptr)
    , renderer_(nullptr)
    , wlr_compositor_(nullptr)
    , output_layout_(nullptr)
    , cursor_(nullptr)
    , xcursor_manager_(nullptr)
    , wlr_seat_(nullptr)
    , xdg_shell_(nullptr)
    // , layer_shell_(nullptr)
    , event_loop_running_(false) {
    
    instance_ = this;
}

WaylandPlatform::~WaylandPlatform() {
    shutdown();
    if (instance_ == this) {
        instance_ = nullptr;
    }
}

bool WaylandPlatform::initialize() {
#ifndef USE_WAYLAND_STUB
    std::cout << "Initializing Wayland platform (wlroots real backend)..." << std::endl;
    // Create Wayland display
    display_ = wl_display_create();
    if (!display_) {
        std::cerr << "Failed to create wl_display" << std::endl;
        return false;
    }

    // Initialize wlroots logging (optional) - 2 corresponds to INFO
    wlr_log_init(2, nullptr);

    // Create backend (auto-detect e.g., DRM, Wayland, X11)
    backend_ = wlr_backend_autocreate(display_, nullptr);
    if (!backend_) {
        std::cerr << "Failed to create wlr_backend" << std::endl;
        return false;
    }

    // Create renderer and hook to display
    renderer_ = wlr_renderer_autocreate(backend_);
    if (!renderer_) {
        std::cerr << "Failed to create wlr_renderer" << std::endl;
        return false;
    }
    wlr_renderer_init_wl_display(renderer_, display_);

    // Create compositor (wlroots 0.17 requires protocol version). Use version 6.
    wlr_compositor_ = wlr_compositor_create(display_, 6, renderer_);
    if (!wlr_compositor_) {
        std::cerr << "Failed to create wlr_compositor" << std::endl;
        return false;
    }

    // Create seat (input)
    wlr_seat_ = wlr_seat_create(display_, "seat0");
    if (!wlr_seat_) {
        std::cerr << "Failed to create wlr_seat" << std::endl;
        return false;
    }

    // Create xdg-shell (requires protocol version)
    xdg_shell_ = wlr_xdg_shell_create(display_, 6);
    if (!xdg_shell_) {
        std::cerr << "Failed to create wlr_xdg_shell" << std::endl;
        return false;
    }

    // Minimal bring-up: listeners will be wired in a follow-up

    // Start backend
    if (!wlr_backend_start(backend_)) {
        std::cerr << "Failed to start wlr_backend" << std::endl;
        return false;
    }

    decorations_enabled_ = true;
    std::cout << "Wayland (wlroots) backend started." << std::endl;
    return true;
#else
    // Minimal Wayland backend initialization (no wlroots API calls).
    std::cout << "Initializing Wayland platform (minimal stub)..." << std::endl;
    decorations_enabled_ = true;
    std::cout << "Wayland platform initialized (stub)." << std::endl;
    return true;
#endif
}

void WaylandPlatform::shutdown() {
#ifndef USE_WAYLAND_STUB
    std::cout << "Shutting down Wayland platform (wlroots)..." << std::endl;
    if (xdg_shell_) { xdg_shell_ = nullptr; }
    // Seat is tied to display lifecycle; skip explicit destroy to avoid ABI issues
    wlr_seat_ = nullptr;
    if (wlr_compositor_) { /* wlr_compositor is owned by display, no explicit destroy */ wlr_compositor_ = nullptr; }
    // Renderer is owned by backend in this path; no explicit destroy
    renderer_ = nullptr;
    if (backend_) { wlr_backend_destroy(backend_); backend_ = nullptr; }
    if (display_) { wl_display_destroy(display_); display_ = nullptr; }
#else
    std::cout << "Shutting down Wayland platform (stub)..." << std::endl;
#endif
}

bool WaylandPlatform::setup_wlroots_backend() { std::cout << "wlroots backend (stub)" << std::endl; return true; }

bool WaylandPlatform::setup_compositor() { std::cout << "compositor (stub)" << std::endl; return true; }

// (removed unused stub-only setup_output/setup_input in real backend path)

bool WaylandPlatform::setup_shell_protocols() { std::cout << "shell protocols (stub)" << std::endl; return true; }

// bool WaylandPlatform::setup_xwayland() { return true; }

bool WaylandPlatform::poll_events(std::vector<Event>& events) {
#ifndef USE_WAYLAND_STUB
    if (!display_ || !backend_) return false;
    events.clear();
    // Dispatch Wayland events; non-blocking to integrate with app loop
    wl_display_dispatch_pending(display_);
    wl_display_flush_clients(display_);
    return true;
#else
    events.clear();
    return true;
#endif
}

void WaylandPlatform::process_event(const Event& event) {
    // TODO: Implement event processing
}

std::unique_ptr<SRDWindow> WaylandPlatform::create_window(const std::string& title, int x, int y, int width, int height) {
    // TODO: Implement window creation
    std::cout << "Creating Wayland window: " << title << std::endl;
    return nullptr;
}

void WaylandPlatform::destroy_window(SRDWindow* window) {
    // TODO: Implement window destruction
    std::cout << "Destroying Wayland window" << std::endl;
}

void WaylandPlatform::set_window_position(SRDWindow* window, int x, int y) {
    // TODO: Implement window positioning
}

void WaylandPlatform::set_window_size(SRDWindow* window, int width, int height) {
    // TODO: Implement window resizing
}

void WaylandPlatform::set_window_title(SRDWindow* window, const std::string& title) {
    // TODO: Implement title setting
}

void WaylandPlatform::focus_window(SRDWindow* window) {
    // TODO: Implement window focusing
}

void WaylandPlatform::minimize_window(SRDWindow* window) {
    // TODO: Implement window minimization
}

void WaylandPlatform::maximize_window(SRDWindow* window) {
    // TODO: Implement window maximization
}

void WaylandPlatform::close_window(SRDWindow* window) {
    // TODO: Implement window closing
}

std::vector<Monitor> WaylandPlatform::get_monitors() {
    // TODO: Implement monitor detection
    return {};
}

Monitor WaylandPlatform::get_primary_monitor() {
    // TODO: Implement primary monitor detection
    return Monitor{};
}

void WaylandPlatform::grab_keyboard() {
    // TODO: Implement keyboard grabbing
}

void WaylandPlatform::ungrab_keyboard() {
    // TODO: Implement keyboard ungrab
}

void WaylandPlatform::grab_pointer() {
    // TODO: Implement pointer grabbing
}

void WaylandPlatform::ungrab_pointer() {
    // TODO: Implement pointer ungrab
}

// Static callback functions
void WaylandPlatform::registry_global_handler(void* data, struct wl_registry* registry,
                                             uint32_t name, const char* interface, uint32_t version) {
    (void)registry; (void)name; (void)interface; (void)version;
    WaylandPlatform* platform = static_cast<WaylandPlatform*>(data);
    platform->handle_registry_global(nullptr, 0, interface ? interface : "", 0);
}

void WaylandPlatform::registry_global_remove_handler(void* data, struct wl_registry* registry, uint32_t name) {
    (void)registry; (void)name;
    WaylandPlatform* platform = static_cast<WaylandPlatform*>(data);
    platform->handle_registry_global_remove(nullptr, 0);
}

void WaylandPlatform::xdg_surface_new_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_xdg_surface_new(static_cast<struct wlr_xdg_surface*>(data));
    }
}

void WaylandPlatform::xdg_surface_destroy_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_xdg_surface_destroy(static_cast<struct wlr_xdg_surface*>(data));
    }
}

void WaylandPlatform::xdg_toplevel_new_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_xdg_toplevel_new(static_cast<struct wlr_xdg_toplevel*>(data));
    }
}

void WaylandPlatform::xdg_toplevel_destroy_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_xdg_toplevel_destroy(static_cast<struct wlr_xdg_toplevel*>(data));
    }
}

// (removed xwayland handlers; not declared while XWayland is disabled)

void WaylandPlatform::output_new_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_output_new(static_cast<struct wlr_output*>(data));
    }
}

void WaylandPlatform::output_destroy_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_output_destroy(static_cast<struct wlr_output*>(data));
    }
}

void WaylandPlatform::output_frame_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_output_frame(static_cast<struct wlr_output*>(data));
    }
}

void WaylandPlatform::pointer_motion_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_pointer_motion(static_cast<struct wlr_pointer_motion_event*>(data));
    }
}

void WaylandPlatform::pointer_button_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_pointer_button(static_cast<struct wlr_pointer_button_event*>(data));
    }
}

void WaylandPlatform::pointer_axis_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_pointer_axis(static_cast<struct wlr_pointer_axis_event*>(data));
    }
}

void WaylandPlatform::keyboard_key_handler(struct wl_listener* listener, void* data) {
    (void)listener;
    if (WaylandPlatform::instance_) {
        WaylandPlatform::instance_->handle_keyboard_key(static_cast<struct wlr_keyboard_key_event*>(data));
    }
}

// Event handling methods
void WaylandPlatform::handle_registry_global(struct wl_registry* registry, uint32_t name, 
                                            const char* interface, uint32_t version) {
    (void)registry; (void)name; (void)version;
    std::cout << "Registry global (stub): " << (interface ? interface : "?") << std::endl;
}

void WaylandPlatform::handle_registry_global_remove(struct wl_registry* registry, uint32_t name) {
    (void)registry; (void)name;
}

void WaylandPlatform::handle_xdg_surface_new(struct wlr_xdg_surface* surface) {
    std::cout << "New XDG surface" << std::endl;
    manage_xdg_window(surface);
}

void WaylandPlatform::handle_xdg_surface_destroy(struct wlr_xdg_surface* surface) {
    std::cout << "XDG surface destroyed" << std::endl;
}

void WaylandPlatform::handle_xdg_toplevel_new(struct wlr_xdg_toplevel* toplevel) {
    std::cout << "New XDG toplevel" << std::endl;
}

void WaylandPlatform::handle_xdg_toplevel_destroy(struct wlr_xdg_toplevel* toplevel) {
    std::cout << "XDG toplevel destroyed" << std::endl;
}

// void WaylandPlatform::handle_xwayland_surface_new(struct wlr_xwayland_surface* surface) {}

// void WaylandPlatform::handle_xwayland_surface_destroy(struct wlr_xwayland_surface* surface) {}

void WaylandPlatform::handle_output_new(struct wlr_output* output) {
    std::cout << "New output" << std::endl;
    handle_output_mode(output);
}

void WaylandPlatform::handle_output_destroy(struct wlr_output* output) {
    std::cout << "Output destroyed" << std::endl;
}

void WaylandPlatform::handle_output_frame(struct wlr_output* output) {
    // Handle output frame event
}

void WaylandPlatform::handle_pointer_motion(struct wlr_pointer_motion_event* event) {
    // Handle pointer motion
}

void WaylandPlatform::handle_pointer_button(struct wlr_pointer_button_event* event) {
    // Handle pointer button
}

void WaylandPlatform::handle_pointer_axis(struct wlr_pointer_axis_event* event) {
    // Handle pointer axis
}

void WaylandPlatform::handle_keyboard_key(struct wlr_keyboard_key_event* event) {
    // Handle keyboard key
}

void WaylandPlatform::manage_xdg_window(struct wlr_xdg_surface* surface) {
    // TODO: Implement XDG window management
}

// void WaylandPlatform::manage_xwayland_window(struct wlr_xwayland_surface* surface) {}

void WaylandPlatform::unmanage_window(SRDWindow* window) {
    // TODO: Implement window unmanagement
}

void WaylandPlatform::handle_output_mode(struct wlr_output* output) {
    // TODO: Implement output mode handling
}

void WaylandPlatform::handle_output_scale(struct wlr_output* output) {
    // TODO: Implement output scale handling
}

void WaylandPlatform::handle_key_event(uint32_t key, bool pressed) {
    // TODO: Implement key event handling
}

void WaylandPlatform::handle_button_event(uint32_t button, bool pressed) {
    // TODO: Implement button event handling
}

void WaylandPlatform::create_surface_window(struct wlr_surface* surface) {
    // TODO: Implement surface window creation
}

void WaylandPlatform::destroy_surface_window(struct wlr_surface* surface) {
    // TODO: Implement surface window destruction
}

void WaylandPlatform::update_surface_window(struct wlr_surface* surface) {
    // TODO: Implement surface window update
}

void WaylandPlatform::convert_wlroots_event_to_srdwm_event(void* event_data, EventType type) {
    // TODO: Implement event conversion
}

void WaylandPlatform::handle_wlroots_error(const std::string& error) {
    std::cerr << "wlroots error: " << error << std::endl;
}

// SRDWindow decoration implementations
void WaylandPlatform::set_window_decorations(SRDWindow* window, bool enabled) {
    // Temporary stub: wlroots xdg-decoration v1 helpers differ across versions.
    // Keep internal state and log, skip calling decoration APIs.
    std::cout << "WaylandPlatform: Set window decorations " << (enabled ? "enabled" : "disabled") << std::endl;
    (void)window; // unused for now
    decorations_enabled_ = enabled;
}

void WaylandPlatform::set_window_border_color(SRDWindow* window, int r, int g, int b) {
    std::cout << "WaylandPlatform: Set border color RGB(" << r << "," << g << "," << b << ")" << std::endl;
    
    if (!window || !renderer_) return;
    
    // Store border color for this window
    // In a full implementation, this would be stored in a window-specific data structure
    // and applied during rendering
    
    // For now, we'll just log the request
    // TODO: Implement custom border rendering via wlroots rendering pipeline
    std::cout << "Border color set for window " << window->getId() 
              << ": RGB(" << r << "," << g << "," << b << ")" << std::endl;
}

void WaylandPlatform::set_window_border_width(SRDWindow* window, int width) {
    std::cout << "WaylandPlatform: Set border width " << width << std::endl;
    
    if (!window) return;
    
    // TODO: Implement when wlroots rendering is set up
    // This would involve drawing custom borders on surfaces
}

bool WaylandPlatform::get_window_decorations(SRDWindow* window) const {
    if (!window) return false;
    
    return decorations_enabled_;
}

void WaylandPlatform::setup_decoration_manager() {
    // Temporary stub: skip creating xdg-decoration manager to avoid 
    // wlroots API/ABI differences across distro versions.
    std::cout << "Decoration manager (stub) initialized" << std::endl;
}

void WaylandPlatform::handle_decoration_request(struct wlr_xdg_surface* surface, uint32_t mode) {
    std::cout << "Handling decoration request, mode: " << mode << std::endl;
    
    // TODO: Implement when zxdg-decoration protocol is available
    // This would involve:
    // 1. Checking if server-side decorations are enabled
    // 2. Setting the decoration mode for the surface
    // 3. Drawing decorations if needed
}


