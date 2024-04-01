#ifndef SRDWM_WAYLAND_PLATFORM_H
#define SRDWM_WAYLAND_PLATFORM_H

#include "platform.h"
#ifndef USE_WAYLAND_STUB
#include <wayland-server-core.h>
#endif
// Forward-declare Wayland and wlroots types to avoid version-specific headers.
struct wl_display; struct wl_registry; struct wl_compositor; struct wl_shm; struct wl_seat; struct wl_output; struct wl_shell; struct wl_listener; struct wl_surface;
struct wlr_backend; struct wlr_renderer; struct wlr_compositor; struct wlr_output_layout; struct wlr_cursor; struct wlr_xcursor_manager; struct wlr_seat;
struct wlr_xdg_shell; struct wlr_xdg_surface; struct wlr_xdg_toplevel;
struct wlr_output; struct wlr_pointer_motion_event; struct wlr_pointer_button_event; struct wlr_pointer_axis_event; struct wlr_keyboard_key_event;
struct wlr_xdg_decoration_manager_v1;
#include <map>
#include <memory>
#include <vector>

// Forward declarations
class SRDWindow;
class Monitor;

// Wayland-specific platform implementation
class WaylandPlatform : public Platform {
public:
    WaylandPlatform();
    ~WaylandPlatform();

    // Platform interface implementation
    bool initialize() override;
    void shutdown() override;
    
    // Event handling
    bool poll_events(std::vector<Event>& events) override;
    void process_event(const Event& event) override;

    // SRDWindow management
    std::unique_ptr<SRDWindow> create_window(const std::string& title, int x, int y, int width, int height) override;
    void destroy_window(SRDWindow* window) override;
    void set_window_position(SRDWindow* window, int x, int y) override;
    void set_window_size(SRDWindow* window, int width, int height) override;
    void set_window_title(SRDWindow* window, const std::string& title) override;
    void focus_window(SRDWindow* window) override;
    void minimize_window(SRDWindow* window) override;
    void maximize_window(SRDWindow* window) override;
    void close_window(SRDWindow* window) override;

    // Monitor management
    std::vector<Monitor> get_monitors() override;
    Monitor get_primary_monitor() override;

    // Input handling
    void grab_keyboard() override;
    void ungrab_keyboard() override;
    void grab_pointer() override;
    void ungrab_pointer() override;

    // Utility
    // SRDWindow decorations (Wayland implementation)
    void set_window_decorations(SRDWindow* window, bool enabled) override;
    void set_window_border_color(SRDWindow* window, int r, int g, int b) override;
    void set_window_border_width(SRDWindow* window, int width) override;
    bool get_window_decorations(SRDWindow* window) const override;

    std::string get_platform_name() const override { return "Wayland"; }
    bool is_wayland() const override { return true; }
    bool is_x11() const override { return false; }
    bool is_windows() const override { return false; }
    bool is_macos() const override { return false; }

private:
    static WaylandPlatform* instance_;
    // Wayland-specific members
    struct wl_display* display_;
    struct wl_registry* registry_;
    struct wl_compositor* compositor_;
    struct wl_shm* shm_;
    struct wl_seat* seat_;
    struct wl_output* output_;
    struct wl_shell* shell_;
    
    // wlroots backend
    struct wlr_backend* backend_;
    struct wlr_renderer* renderer_;
    struct wlr_compositor* wlr_compositor_;
    struct wlr_output_layout* output_layout_;
    struct wlr_cursor* cursor_;
    struct wlr_xcursor_manager* xcursor_manager_;
    struct wlr_seat* wlr_seat_;
    
    // Shell protocols
    struct wlr_xdg_shell* xdg_shell_;
    // struct wlr_layer_shell_v1* layer_shell_;
    
    // XWayland support (temporarily disabled)
    // struct wlr_xwayland* xwayland_;
    // struct wlr_xwayland_server* xwayland_server_;
    
    // SRDWindow tracking
    std::map<struct wlr_surface*, SRDWindow*> surface_window_map_;
    std::map<struct wlr_xdg_surface*, SRDWindow*> xdg_window_map_;
    // std::map<struct wlr_xwayland_surface*, SRDWindow*> xwayland_window_map_;
    
    // Monitor information
    std::vector<Monitor> monitors_;
    Monitor primary_monitor_;
    
    // Event handling
    bool event_loop_running_;
    std::vector<Event> pending_events_;
    
    // Private methods
    bool setup_wlroots_backend();
    bool setup_compositor();
    bool setup_shell_protocols();
    // bool setup_xwayland();
    
    // Event handling
    void handle_registry_global(struct wl_registry* registry, uint32_t name, 
                               const char* interface, uint32_t version);
    void handle_registry_global_remove(struct wl_registry* registry, uint32_t name);
    
    // Shell protocol handlers
    void handle_xdg_surface_new(struct wlr_xdg_surface* surface);
    void handle_xdg_surface_destroy(struct wlr_xdg_surface* surface);
    void handle_xdg_toplevel_new(struct wlr_xdg_toplevel* toplevel);
    void handle_xdg_toplevel_destroy(struct wlr_xdg_toplevel* toplevel);
    
    // XWayland handlers (temporarily disabled)
    // void handle_xwayland_surface_new(struct wlr_xwayland_surface* surface);
    // void handle_xwayland_surface_destroy(struct wlr_xwayland_surface* surface);
    
    // Output handlers
    void handle_output_new(struct wlr_output* output);
    void handle_output_destroy(struct wlr_output* output);
    void handle_output_frame(struct wlr_output* output);
    
    // Input handlers
    void handle_pointer_motion(struct wlr_pointer_motion_event* event);
    void handle_pointer_button(struct wlr_pointer_button_event* event);
    void handle_pointer_axis(struct wlr_pointer_axis_event* event);
    void handle_keyboard_key(struct wlr_keyboard_key_event* event);
    
    // SRDWindow management helpers
    SRDWindow* find_window_by_surface(struct wlr_surface* surface);
    SRDWindow* find_window_by_xdg_surface(struct wlr_xdg_surface* surface);
    // SRDWindow* find_window_by_xwayland_surface(struct wlr_xwayland_surface* surface);
    void manage_xdg_window(struct wlr_xdg_surface* surface);
    // void manage_xwayland_window(struct wlr_xwayland_surface* surface);
    void unmanage_window(SRDWindow* window);
    
    // Monitor helpers
    void update_monitor_info();
    void handle_output_mode(struct wlr_output* output);
    void handle_output_scale(struct wlr_output* output);
    
    // Input helpers
    void setup_keyboard_grab();
    void setup_pointer_grab();
    void handle_key_event(uint32_t key, bool pressed);
    void handle_button_event(uint32_t button, bool pressed);
    
    // Utility helpers
    void create_surface_window(struct wlr_surface* surface);
    void destroy_surface_window(struct wlr_surface* surface);
    void update_surface_window(struct wlr_surface* surface);
    
    // Event conversion
    void convert_wlroots_event_to_srdwm_event(void* event_data, EventType type);
    
    // Error handling
    void handle_wlroots_error(const std::string& error);
    
    // Decoration methods
    void setup_decoration_manager();
    void handle_decoration_request(struct wlr_xdg_surface* surface, uint32_t mode);
    
    // Static callback functions
    static void registry_global_handler(void* data, struct wl_registry* registry,
                                       uint32_t name, const char* interface, uint32_t version);
    static void registry_global_remove_handler(void* data, struct wl_registry* registry, uint32_t name);
    
    static void xdg_surface_new_handler(struct wl_listener* listener, void* data);
    static void xdg_surface_destroy_handler(struct wl_listener* listener, void* data);
    static void xdg_toplevel_new_handler(struct wl_listener* listener, void* data);
    static void xdg_toplevel_destroy_handler(struct wl_listener* listener, void* data);
    
    // static void xwayland_surface_new_handler(struct wl_listener* listener, void* data);
    // static void xwayland_surface_destroy_handler(struct wl_listener* listener, void* data);
    
    static void output_new_handler(struct wl_listener* listener, void* data);
    static void output_destroy_handler(struct wl_listener* listener, void* data);
    static void output_frame_handler(struct wl_listener* listener, void* data);
    
    static void pointer_motion_handler(struct wl_listener* listener, void* data);
    static void pointer_button_handler(struct wl_listener* listener, void* data);
    static void pointer_axis_handler(struct wl_listener* listener, void* data);
    static void keyboard_key_handler(struct wl_listener* listener, void* data);
    
    // Event listeners removed in stubbed build to avoid wl_listener dependency
    // wlroots event listeners (instantiated only in real Wayland path)
    #ifndef USE_WAYLAND_STUB
    struct wl_listener xdg_surface_new_listener_;
    struct wl_listener xdg_surface_destroy_listener_;
    struct wl_listener xdg_toplevel_new_listener_;
    struct wl_listener xdg_toplevel_destroy_listener_;
    struct wl_listener output_new_listener_;
    struct wl_listener output_destroy_listener_;
    struct wl_listener output_frame_listener_;
    struct wl_listener pointer_motion_listener_;
    struct wl_listener pointer_button_listener_;
    struct wl_listener pointer_axis_listener_;
    struct wl_listener keyboard_key_listener_;
    #endif

    // Decoration state
    bool decorations_enabled_;
    struct wlr_xdg_decoration_manager_v1* decoration_manager_;
    
    // struct wl_listener xwayland_new_surface;
    // struct wl_listener xwayland_destroy;
    
    // Additional listeners removed in stubbed build
};

#endif // SRDWM_WAYLAND_PLATFORM_H


