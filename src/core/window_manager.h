#ifndef SRDWM_WINDOW_MANAGER_H
#define SRDWM_WINDOW_MANAGER_H

#include <vector>
#include <algorithm> // Required for std::remove_if
#include <memory> // Required for std::unique_ptr
#include <map>
#include <string>
#include <functional>
#include <set> // Required for std::set

#include "../input/input_handler.h"
#include "../layouts/layout_engine.h"
#include "../platform/platform.h" // For Event type
#include "../layouts/layout.h" // For Monitor type

class SRDWindow; // Forward declaration
class InputHandler; // Forward declaration
class LuaManager; // Forward declaration

// Workspace structure
struct Workspace {
    int id;
    std::string name;
    std::vector<SRDWindow*> windows;
    std::string layout;
    bool visible;
    
    Workspace(int id, const std::string& name = "") 
        : id(id), name(name), layout("tiling"), visible(false) {}
};

class SRDWindowManager {
public:
    SRDWindowManager();
    ~SRDWindowManager();

    void run(); // Main loop

    // SRDWindow management
    void add_window(std::unique_ptr<SRDWindow> window);
    void remove_window(SRDWindow* window);
    void focus_window(SRDWindow* window);
    SRDWindow* get_focused_window() const;
    std::vector<SRDWindow*> get_windows() const;
    void manage_windows(); // Added missing method
    void focus_next_window();
    void focus_previous_window();
    
    // SRDWindow operations
    void close_window(SRDWindow* window);
    void minimize_window(SRDWindow* window);
    void maximize_window(SRDWindow* window);
    void move_window(SRDWindow* window, int x, int y);
    void resize_window(SRDWindow* window, int width, int height);
    void toggle_window_floating(SRDWindow* window);
    bool is_window_floating(SRDWindow* window) const;
    
    // Window dragging and resizing
    void start_window_drag(SRDWindow* window, int start_x, int start_y);
    void start_window_resize(SRDWindow* window, int start_x, int start_y, int edge);
    void update_window_drag(int x, int y);
    void update_window_resize(int x, int y);
    void end_window_drag();
    void end_window_resize();
    bool is_dragging() const { return dragging_window_ != nullptr; }
    bool is_resizing() const { return resizing_window_ != nullptr; }

    // Workspace management
    void add_workspace(const std::string& name = "");
    void remove_workspace(int workspace_id);
    void switch_to_workspace(int workspace_id);
    void move_window_to_workspace(SRDWindow* window, int workspace_id);
    int get_current_workspace() const;
    std::vector<Workspace> get_workspaces() const;
    Workspace* get_workspace(int workspace_id);
    
    // Layout management
    void set_layout(int monitor_id, const std::string& layout_name);
    std::string get_layout(int monitor_id) const;
    void arrange_windows();
    void tile_windows();
    void arrange_windows_dynamic();

    // Key binding system
    void bind_key(const std::string& key_combination, std::function<void()> action);
    void unbind_key(const std::string& key_combination);
    void handle_key_press(int key_code, int modifiers);
    void handle_key_release(int key_code, int modifiers);
    
    // Input event handling (called by InputHandler)
    void handle_key_press(int key_code);
    void handle_key_release(int key_code);
    void handle_mouse_button_press(int button, int x, int y);
    void handle_mouse_button_release(int button, int x, int y);
    void handle_mouse_motion(int x, int y);
    void handle_event(const Event& event); // Added missing method

    // Integration
    void set_layout_engine(LayoutEngine* engine);
    void set_lua_manager(LuaManager* manager);
    void set_platform(Platform* platform);

private:
    // Window tracking
    std::vector<SRDWindow*> windows_; // Added missing member variable
    SRDWindow* focused_window_ = nullptr;
    std::set<SRDWindow*> floating_windows_; // Track floating windows
    InputHandler* input_handler_ = nullptr;
    LayoutEngine* layout_engine_ = nullptr;
    LuaManager* lua_manager_ = nullptr;
    Platform* platform_ = nullptr;

    // Workspace management
    std::vector<Workspace> workspaces_;
    int current_workspace_ = 0;
    int next_workspace_id_ = 1;
    
    // Window dragging and resizing state
    SRDWindow* dragging_window_ = nullptr;
    SRDWindow* resizing_window_ = nullptr;
    int drag_start_x_ = 0;
    int drag_start_y_ = 0;
    int drag_start_window_x_ = 0;
    int drag_start_window_y_ = 0;
    int resize_start_x_ = 0;
    int resize_start_y_ = 0;
    int resize_start_width_ = 0;
    int resize_start_height_ = 0;
    int resize_edge_ = 0; // 0=none, 1=left, 2=right, 3=top, 4=bottom, 5=corner

    // Key binding system
    std::map<std::string, std::function<void()>> key_bindings_;
    std::map<int, int> pressed_keys_; // key_code -> modifiers
    
    // Platform-specific data (placeholder)
    void* platform_data_ = nullptr;
    
    // Monitor information
    std::vector<Monitor> monitors_;
    
    // Helper methods
    std::string key_code_to_string(int key_code, int modifiers) const;
    void execute_key_binding(const std::string& key_combination);
    void update_layout_for_window(SRDWindow* window);
    void arrange_workspace_windows(int workspace_id);
    void update_workspace_visibility();
    
    // Window interaction helpers
    SRDWindow* find_window_at_position(int x, int y) const;
    bool is_in_titlebar_area(SRDWindow* window, int x, int y) const;
    bool is_in_resize_area(SRDWindow* window, int x, int y) const;
    int get_resize_edge(SRDWindow* window, int x, int y) const;
};

#endif // SRDWM_WINDOW_MANAGER_H
