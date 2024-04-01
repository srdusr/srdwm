#ifndef SRDWM_X11_PLATFORM_H
#define SRDWM_X11_PLATFORM_H

#include "platform.h"
#include <string>
#include <vector>
#include <map>
#include <algorithm>

#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/Xatom.h>
#include <X11/extensions/Xrandr.h>
#include <X11/extensions/Xinerama.h>

// X11 types are now properly included
// Use X11Window typedef to avoid collision with our SRDWindow class
typedef unsigned long X11Window;

// Helper functions to convert between X11Window and X11's Window type
inline ::Window to_x11_window(X11Window w) { return static_cast<::Window>(w); }
inline X11Window from_x11_window(::Window w) { return static_cast<X11Window>(w); }

class X11Platform : public Platform {
public:
    X11Platform();
    ~X11Platform() override;

    // Platform interface implementation
    bool initialize() override;
    void shutdown() override;
    bool poll_events(std::vector<Event>& events) override;
    void process_event(const Event& event) override;

    // Window management
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

    // Window decorations (X11 implementation)
    void set_window_decorations(SRDWindow* window, bool enabled) override;
    void set_window_border_color(SRDWindow* window, int r, int g, int b) override;
    void set_window_border_width(SRDWindow* window, int width) override;
    bool get_window_decorations(SRDWindow* window) const override;

    // Linux/X11-specific features
    void enable_compositor(bool enabled);
    void set_window_opacity(SRDWindow* window, unsigned char opacity);
    void set_window_blur(SRDWindow* window, bool enabled);
    void set_window_shadow(SRDWindow* window, bool enabled);
    
    // EWMH (Extended Window Manager Hints) support
    void set_ewmh_supported(bool supported);
    void set_window_type(SRDWindow* window, const std::string& type);
    void set_window_state(SRDWindow* window, const std::vector<std::string>& states);
    void set_window_strut(SRDWindow* window, int left, int right, int top, int bottom);
    
    // Virtual Desktop support (X11 workspaces)
    void create_virtual_desktop(const std::string& name);
    void remove_virtual_desktop(int desktop_id);
    void switch_to_virtual_desktop(int desktop_id);
    int get_current_virtual_desktop() const;
    std::vector<int> get_virtual_desktops() const;
    void move_window_to_desktop(SRDWindow* window, int desktop_id);
    
    // Panel/Dock integration
    void set_panel_visible(bool visible);
    void set_panel_position(int position); // 0=bottom, 1=top, 2=left, 3=right
    void set_panel_auto_hide(bool enabled);
    void update_panel_workspace_list();
    
    // System tray integration
    void add_system_tray_icon(const std::string& tooltip, Pixmap icon);
    void remove_system_tray_icon();
    void show_system_tray_menu(Window menu);
    
    // RandR (Resize and Rotate) support
    void enable_randr(bool enabled);
    void set_monitor_rotation(int monitor_id, int rotation);
    void set_monitor_refresh_rate(int monitor_id, int refresh_rate);
    void set_monitor_scale(int monitor_id, float scale);

    // Utility
    std::string get_platform_name() const override { return "X11"; }
    bool is_wayland() const override { return false; }
    bool is_x11() const override { return true; }
    bool is_windows() const override { return false; }
    bool is_macos() const override { return false; }

private:
    // X11-specific members
    Display* display_ = nullptr;
    X11Window root_ = 0;
    
    // Window tracking
    std::map<X11Window, ::SRDWindow*> window_map_;
    std::map<X11Window, X11Window> frame_window_map_; // client -> frame
    
    // Monitor information
    std::vector<Monitor> monitors_;
    
    // Decoration state
    bool decorations_enabled_;
    int border_width_;
    unsigned long border_color_;
    unsigned long focused_border_color_;
    
    // Linux/X11-specific state
    bool compositor_enabled_;
    bool ewmh_supported_;
    bool randr_enabled_;
    int current_virtual_desktop_;
    std::vector<int> virtual_desktops_;
    bool panel_visible_;
    bool panel_auto_hide_;
    int panel_position_;
    Window system_tray_icon_;
    
    // EWMH atoms
    Atom _NET_WM_STATE_;
    Atom _NET_WM_STATE_MAXIMIZED_VERT_;
    Atom _NET_WM_STATE_MAXIMIZED_HORZ_;
    Atom _NET_WM_STATE_FULLSCREEN_;
    Atom _NET_WM_STATE_ABOVE_;
    Atom _NET_WM_STATE_BELOW_;
    Atom _NET_WM_WINDOW_TYPE_;
    Atom _NET_WM_WINDOW_TYPE_DESKTOP_;
    Atom _NET_WM_WINDOW_TYPE_DOCK_;
    Atom _NET_WM_WINDOW_TYPE_TOOLBAR_;
    Atom _NET_WM_WINDOW_TYPE_MENU_;
    Atom _NET_WM_WINDOW_TYPE_UTILITY_;
    Atom _NET_WM_WINDOW_TYPE_SPLASH_;
    Atom _NET_WM_WINDOW_TYPE_DIALOG_;
    Atom _NET_WM_WINDOW_TYPE_DROPDOWN_MENU_;
    Atom _NET_WM_WINDOW_TYPE_POPUP_MENU_;
    Atom _NET_WM_WINDOW_TYPE_TOOLTIP_;
    Atom _NET_WM_WINDOW_TYPE_NOTIFICATION_;
    Atom _NET_WM_WINDOW_TYPE_COMBO_;
    Atom _NET_WM_WINDOW_TYPE_DND_;
    Atom _NET_WM_WINDOW_TYPE_NORMAL_;
    Atom _NET_WM_DESKTOP_;
    Atom _NET_NUMBER_OF_DESKTOPS_;
    Atom _NET_CURRENT_DESKTOP_;
    Atom _NET_DESKTOP_NAMES_;
    Atom _NET_WM_STRUT_;
    Atom _NET_WM_STRUT_PARTIAL_;
    Atom _NET_WM_OPACITY_;
    
    // Helper methods
    SRDWindow* get_focused_window() const;

    // Private methods
    bool setup_x11_environment();
    bool setup_event_masks();
    void handle_x11_event(XEvent& event);
    void setup_atoms();
    void setup_extensions();
    bool check_for_other_wm();
    
    // Event handlers
    void handle_map_request(XMapRequestEvent& event);
    void handle_configure_request(XConfigureRequestEvent& event);
    void handle_destroy_notify(XDestroyWindowEvent& event);
    void handle_unmap_notify(XUnmapEvent& event);
    void handle_key_press(XKeyEvent& event);
    void handle_button_press(XButtonEvent& event);
    void handle_motion_notify(XMotionEvent& event);
    
    // Decoration methods
    void create_frame_window(SRDWindow* window);
    void destroy_frame_window(SRDWindow* window);
    void draw_titlebar(SRDWindow* window);
    void update_frame_geometry(SRDWindow* window);
    
    // EWMH methods
    void setup_ewmh();
    void update_ewmh_desktop_info();
    void handle_ewmh_message(XClientMessageEvent& event);
    
    // Virtual Desktop methods
    void initialize_virtual_desktops();
    void cleanup_virtual_desktops();
    
    // RandR methods
    void initialize_randr();
    void cleanup_randr();
    
    // Panel methods
    void initialize_panel();
    void update_panel();
};

#endif // SRDWM_X11_PLATFORM_H

