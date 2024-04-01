#include "wayland_platform.h"
#include <iostream>
#include <cstdint>

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
    , event_loop_running_(false)
    , decorations_enabled_(true)
    , decoration_manager_(nullptr) {
  instance_ = this;
}

WaylandPlatform::~WaylandPlatform() {
  shutdown();
  if (instance_ == this) instance_ = nullptr;
}

bool WaylandPlatform::initialize() {
  std::cout << "Wayland (stub): initialize" << std::endl;
  decorations_enabled_ = true;
  return true;
}

void WaylandPlatform::shutdown() {
  std::cout << "Wayland (stub): shutdown" << std::endl;
}

bool WaylandPlatform::poll_events(std::vector<Event>& events) {
  events.clear();
  return true;
}

void WaylandPlatform::process_event(const Event& /*event*/) {}

std::unique_ptr<SRDWindow> WaylandPlatform::create_window(const std::string& title, int, int, int, int) {
  std::cout << "Wayland (stub): create_window '" << title << "'" << std::endl;
  return nullptr;
}

void WaylandPlatform::destroy_window(SRDWindow*) {}
void WaylandPlatform::set_window_position(SRDWindow*, int, int) {}
void WaylandPlatform::set_window_size(SRDWindow*, int, int) {}
void WaylandPlatform::set_window_title(SRDWindow*, const std::string&) {}
void WaylandPlatform::focus_window(SRDWindow*) {}
void WaylandPlatform::minimize_window(SRDWindow*) {}
void WaylandPlatform::maximize_window(SRDWindow*) {}
void WaylandPlatform::close_window(SRDWindow*) {}
std::vector<Monitor> WaylandPlatform::get_monitors() { return {}; }
Monitor WaylandPlatform::get_primary_monitor() { return Monitor{}; }
void WaylandPlatform::grab_keyboard() {}
void WaylandPlatform::ungrab_keyboard() {}
void WaylandPlatform::grab_pointer() {}
void WaylandPlatform::ungrab_pointer() {}
void WaylandPlatform::set_window_decorations(SRDWindow*, bool enabled) { decorations_enabled_ = enabled; }
void WaylandPlatform::set_window_border_color(SRDWindow*, int, int, int) {}
void WaylandPlatform::set_window_border_width(SRDWindow*, int) {}
bool WaylandPlatform::get_window_decorations(SRDWindow*) const { return decorations_enabled_; }

// Private helpers (stubs)
bool WaylandPlatform::setup_wlroots_backend() { return true; }
bool WaylandPlatform::setup_compositor() { return true; }
bool WaylandPlatform::setup_shell_protocols() { return true; }
void WaylandPlatform::handle_registry_global(struct wl_registry*, uint32_t, const char*, uint32_t) {}
void WaylandPlatform::handle_registry_global_remove(struct wl_registry*, uint32_t) {}
void WaylandPlatform::handle_xdg_surface_new(struct wlr_xdg_surface*) {}
void WaylandPlatform::handle_xdg_surface_destroy(struct wlr_xdg_surface*) {}
void WaylandPlatform::handle_xdg_toplevel_new(struct wlr_xdg_toplevel*) {}
void WaylandPlatform::handle_xdg_toplevel_destroy(struct wlr_xdg_toplevel*) {}
void WaylandPlatform::handle_output_new(struct wlr_output*) {}
void WaylandPlatform::handle_output_destroy(struct wlr_output*) {}
void WaylandPlatform::handle_output_frame(struct wlr_output*) {}
void WaylandPlatform::handle_pointer_motion(struct wlr_pointer_motion_event*) {}
void WaylandPlatform::handle_pointer_button(struct wlr_pointer_button_event*) {}
void WaylandPlatform::handle_pointer_axis(struct wlr_pointer_axis_event*) {}
void WaylandPlatform::keyboard_key_handler(struct wl_listener*, void*) {}
void WaylandPlatform::pointer_motion_handler(struct wl_listener*, void*) {}
void WaylandPlatform::pointer_button_handler(struct wl_listener*, void*) {}
void WaylandPlatform::pointer_axis_handler(struct wl_listener*, void*) {}
void WaylandPlatform::output_new_handler(struct wl_listener*, void*) {}
void WaylandPlatform::output_destroy_handler(struct wl_listener*, void*) {}
void WaylandPlatform::output_frame_handler(struct wl_listener*, void*) {}
void WaylandPlatform::xdg_surface_new_handler(struct wl_listener*, void*) {}
void WaylandPlatform::xdg_surface_destroy_handler(struct wl_listener*, void*) {}
void WaylandPlatform::xdg_toplevel_new_handler(struct wl_listener*, void*) {}
void WaylandPlatform::xdg_toplevel_destroy_handler(struct wl_listener*, void*) {}
void WaylandPlatform::manage_xdg_window(struct wlr_xdg_surface*) {}
void WaylandPlatform::unmanage_window(SRDWindow*) {}
void WaylandPlatform::handle_output_mode(struct wlr_output*) {}
void WaylandPlatform::handle_output_scale(struct wlr_output*) {}
void WaylandPlatform::handle_key_event(uint32_t, bool) {}
void WaylandPlatform::handle_button_event(uint32_t, bool) {}
void WaylandPlatform::create_surface_window(struct wlr_surface*) {}
void WaylandPlatform::destroy_surface_window(struct wlr_surface*) {}
void WaylandPlatform::update_surface_window(struct wlr_surface*) {}
void WaylandPlatform::convert_wlroots_event_to_srdwm_event(void*, EventType) {}
void WaylandPlatform::handle_wlroots_error(const std::string&) {}
void WaylandPlatform::setup_decoration_manager() {}
void WaylandPlatform::handle_decoration_request(struct wlr_xdg_surface*, uint32_t) {}
