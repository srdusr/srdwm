#ifndef SRDWM_MACOS_PLATFORM_H
#define SRDWM_MACOS_PLATFORM_H

#include "platform.h"
#include <memory>
#include <string>
#include <map>

// Forward declarations for macOS
#ifdef __APPLE__
typedef struct CGSRDWindow* CGSRDWindowRef;
typedef struct CGEvent* CGEventRef;
typedef struct CGDisplay* CGDirectDisplayID;
typedef struct CGMenu* CGMenuRef;
typedef struct CGMenuBar* CGMenuBarRef;
#endif

class MacOSPlatform : public Platform {
public:
    MacOSPlatform();
    ~MacOSPlatform() override;

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
    
    // SRDWindow decorations (macOS implementation)
    void set_window_decorations(SRDWindow* window, bool enabled) override;
    void set_window_border_color(SRDWindow* window, int r, int g, int b) override;
    void set_window_border_width(SRDWindow* window, int width) override;
    bool get_window_decorations(SRDWindow* window) const override;

    // macOS-specific features
    void setup_global_menu();
    void update_global_menu(const std::string& app_name);
    void set_menu_bar_visible(bool visible);
    void set_dock_visible(bool visible);
    void set_spaces_enabled(bool enabled);
    void switch_to_space(int space_id);
    int get_current_space() const;
    std::vector<int> get_available_spaces() const;
    
    // Mission Control and Spaces
    void show_mission_control();
    void show_app_expose();
    void show_desktop();
    
    // Window management enhancements
    void set_window_level(SRDWindow* window, int level);
    void set_window_shadow(SRDWindow* window, bool enabled);
    void set_window_blur(SRDWindow* window, bool enabled);
    void set_window_alpha(SRDWindow* window, float alpha);

    std::string get_platform_name() const override;
    bool is_wayland() const override;
    bool is_x11() const override;
    bool is_windows() const override;
    bool is_macos() const override;

private:
    bool initialized_;
    std::map<CGSRDWindowRef, SRDWindow*> window_map_;
    std::vector<Monitor> monitors_;
    
    // Decoration state (macOS limitations)
    bool decorations_enabled_;
    std::map<CGSRDWindowRef, CGSRDWindowRef> overlay_window_map_; // window -> overlay
    
    // macOS-specific state
    bool global_menu_enabled_;
    bool dock_visible_;
    bool spaces_enabled_;
    CGMenuBarRef menu_bar_;
    std::map<std::string, CGMenuRef> app_menus_;
    
    // Event handling
    void setup_event_tap();
    void remove_event_tap();
    
    // Monitor management
    void update_monitors();
    
    // Utility methods
    SRDWindow* find_window_by_cgwindow(CGSRDWindowRef cgwindow);
    
    // Decoration methods (macOS limitations)
    void create_overlay_window(SRDWindow* window);
    void destroy_overlay_window(SRDWindow* window);
    
    // Global menu methods
    void create_app_menu(const std::string& app_name);
    void update_menu_bar();
    void handle_menu_event(CGEventRef event);
};

#endif // SRDWM_MACOS_PLATFORM_H

