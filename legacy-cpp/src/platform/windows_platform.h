#ifndef SRDWM_WINDOWS_PLATFORM_H
#define SRDWM_WINDOWS_PLATFORM_H

#include "platform.h"
#include <windows.h>
#include <memory>
#include <string>
#include <map>

class SRDWindowsPlatform : public Platform {
public:
    SRDWindowsPlatform();
    ~SRDWindowsPlatform() override;

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
    
    // SRDWindow decorations (SRDWindows implementation)
    void set_window_decorations(SRDWindow* window, bool enabled) override;
    void set_window_border_color(SRDWindow* window, int r, int g, int b) override;
    void set_window_border_width(SRDWindow* window, int width) override;
    bool get_window_decorations(SRDWindow* window) const override;

    // Windows-specific features
    void enable_dwm_composition(bool enabled);
    void set_dwm_window_attribute(HWND hwnd, DWORD attribute, DWORD value);
    void set_dwm_window_thumbnail(HWND hwnd, HWND thumbnail_hwnd);
    void set_dwm_window_peek(HWND hwnd, bool enabled);
    
    // Virtual Desktop support (Windows 10+)
    void create_virtual_desktop();
    void remove_virtual_desktop(int desktop_id);
    void switch_to_virtual_desktop(int desktop_id);
    int get_current_virtual_desktop() const;
    std::vector<int> get_virtual_desktops() const;
    void move_window_to_desktop(SRDWindow* window, int desktop_id);
    
    // Taskbar integration
    void set_taskbar_visible(bool visible);
    void set_taskbar_position(int position); // 0=bottom, 1=top, 2=left, 3=right
    void set_taskbar_auto_hide(bool enabled);
    void update_taskbar_preview(HWND hwnd, const std::string& title);
    
    // Aero effects
    void enable_aero_effects(bool enabled);
    void set_window_transparency(HWND hwnd, BYTE alpha);
    void set_window_blur(HWND hwnd, bool enabled);
    void set_window_shadow(HWND hwnd, bool enabled);
    
    // System tray integration
    void add_system_tray_icon(const std::string& tooltip, HICON icon);
    void remove_system_tray_icon();
    void show_system_tray_menu(HMENU menu);

    std::string get_platform_name() const override;
    bool is_wayland() const override;
    bool is_x11() const override;
    bool is_windows() const override;
    bool is_macos() const override;

private:
    bool initialized_;
    HINSTANCE h_instance_;
    std::map<HWND, SRDWindow*> window_map_;
    std::vector<Monitor> monitors_;
    
    // Decoration state
    bool decorations_enabled_;
    int border_width_;
    COLORREF border_color_;
    COLORREF focused_border_color_;
    
    // Windows-specific state
    bool dwm_enabled_;
    bool aero_enabled_;
    bool taskbar_visible_;
    bool taskbar_auto_hide_;
    int taskbar_position_;
    int current_virtual_desktop_;
    std::vector<int> virtual_desktops_;
    NOTIFYICONDATA system_tray_icon_;
    
    // SRDWindow class registration
    bool register_window_class();
    void unregister_window_class();
    
    // SRDWindow procedure
    static LRESULT CALLBACK window_proc(HWND hwnd, UINT msg, WPARAM wparam, LPARAM lparam);
    
    // Event conversion
    void convert_win32_message(UINT msg, WPARAM wparam, LPARAM lparam, std::vector<Event>& events);
    
    // Monitor enumeration
    static BOOL CALLBACK enum_monitor_proc(HMONITOR hmonitor, HDC hdc_monitor, LPRECT lprc_monitor, LPARAM dw_data);
    
    // Utility methods
    void update_monitors();
    SRDWindow* find_window_by_hwnd(HWND hwnd);
    
    // Decoration methods
    void apply_dwm_border_color(HWND hwnd, int r, int g, int b);
    void remove_dwm_border_color(HWND hwnd);
    
    // Virtual Desktop methods
    void initialize_virtual_desktops();
    void cleanup_virtual_desktops();
    
    // DWM methods
    bool initialize_dwm();
    void cleanup_dwm();
    
    // Taskbar methods
    void initialize_taskbar();
    void update_taskbar();
};

#endif // SRDWM_WINDOWS_PLATFORM_H

