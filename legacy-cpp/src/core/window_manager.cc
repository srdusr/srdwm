#include "window_manager.h"
#include "window.h"
#include "../input/input_handler.h"
#include "../layouts/layout_engine.h"
#include "../platform/platform.h"
#include "../config/lua_manager.h"
#include <iostream>
#include <chrono>
#include <thread>

SRDWindowManager::SRDWindowManager() {
    std::cout << "SRDWindowManager: Initializing..." << std::endl;
}

SRDWindowManager::~SRDWindowManager() {
    std::cout << "SRDWindowManager: Shutting down..." << std::endl;
}

void SRDWindowManager::run() {
    // Main event loop
    std::cout << "SRDWindowManager: Starting main loop" << std::endl;
    
    if (!platform_) {
        std::cerr << "SRDWindowManager: No platform available, cannot run" << std::endl;
        return;
    }
    
    std::cout << "SRDWindowManager: Entering main event loop..." << std::endl;
    
    bool running = true;
    while (running) {
        // Poll for platform events
        std::vector<Event> events;
        if (platform_->poll_events(events)) {
            // Process all events
            for (const auto& event : events) {
                handle_event(event);
                
                // Check for exit condition
                if (event.type == EventType::KeyPress) {
                    // TODO: Check for exit key combination
                    // For now, just continue
                }
            }
        }
        
        // Manage windows
        manage_windows();
        
        // Arrange windows if needed
        if (layout_engine_) {
            layout_engine_->arrange_all_monitors();
        }
        
        // Small delay to prevent busy waiting
        // TODO: Use proper event-driven approach instead of polling
        std::this_thread::sleep_for(std::chrono::milliseconds(16)); // ~60 FPS
    }
    
    std::cout << "SRDWindowManager: Main loop ended" << std::endl;
}

// SRDWindow management
void SRDWindowManager::add_window(std::unique_ptr<SRDWindow> window) {
    if (window) {
        windows_.push_back(window.get());
        window.release(); // Transfer ownership
        
        // Add to layout engine if available
        if (layout_engine_) {
            layout_engine_->add_window(window.get());
        }
        
        std::cout << "SRDWindowManager: Added window " << window->getId() << std::endl;
    }
}

void SRDWindowManager::remove_window(SRDWindow* window) {
    auto it = std::find(windows_.begin(), windows_.end(), window);
    if (it != windows_.end()) {
        windows_.erase(it);
        
        // Remove from layout engine if available
        if (layout_engine_) {
            layout_engine_->remove_window(window);
        }
        
        std::cout << "SRDWindowManager: Removed window " << window->getId() << std::endl;
    }
}

void SRDWindowManager::focus_window(SRDWindow* window) {
    focused_window_ = window;
    std::cout << "SRDWindowManager: Focused window " << (window ? window->getId() : -1) << std::endl;
}

SRDWindow* SRDWindowManager::get_focused_window() const {
    return focused_window_;
}

std::vector<SRDWindow*> SRDWindowManager::get_windows() const {
    return windows_;
}

void SRDWindowManager::focus_next_window() {
    if (windows_.empty()) return;
    
    if (!focused_window_) {
        // No window focused, focus the first one
        focus_window(windows_[0]);
        return;
    }
    
    // Find current focused window index
    auto it = std::find(windows_.begin(), windows_.end(), focused_window_);
    if (it == windows_.end()) {
        focus_window(windows_[0]);
        return;
    }
    
    // Move to next window (wrap around)
    ++it;
    if (it == windows_.end()) {
        it = windows_.begin();
    }
    
    focus_window(*it);
    std::cout << "SRDWindowManager: Focused next window " << (*it)->getId() << std::endl;
}

void SRDWindowManager::focus_previous_window() {
    if (windows_.empty()) return;
    
    if (!focused_window_) {
        // No window focused, focus the last one
        focus_window(windows_.back());
        return;
    }
    
    // Find current focused window index
    auto it = std::find(windows_.begin(), windows_.end(), focused_window_);
    if (it == windows_.end()) {
        focus_window(windows_.back());
        return;
    }
    
    // Move to previous window (wrap around)
    if (it == windows_.begin()) {
        it = windows_.end() - 1;
    } else {
        --it;
    }
    
    focus_window(*it);
    std::cout << "SRDWindowManager: Focused previous window " << (*it)->getId() << std::endl;
}

void SRDWindowManager::manage_windows() {
    // Manage window states
    std::cout << "SRDWindowManager: Managing " << windows_.size() << " windows" << std::endl;
}

// SRDWindow operations
void SRDWindowManager::close_window(SRDWindow* window) {
    if (window) {
        std::cout << "SRDWindowManager: Closing window " << window->getId() << std::endl;
        // TODO: Implement actual window closing
    }
}

void SRDWindowManager::minimize_window(SRDWindow* window) {
    if (window) {
        std::cout << "SRDWindowManager: Minimizing window " << window->getId() << std::endl;
        // TODO: Implement actual window minimizing
    }
}

void SRDWindowManager::maximize_window(SRDWindow* window) {
    if (window) {
        std::cout << "SRDWindowManager: Maximizing window " << window->getId() << std::endl;
        // TODO: Implement actual window maximizing
    }
}

void SRDWindowManager::move_window(SRDWindow* window, int x, int y) {
    if (window) {
        window->setPosition(x, y);
        update_layout_for_window(window);
        std::cout << "SRDWindowManager: Moved window " << window->getId() << " to (" << x << ", " << y << ")" << std::endl;
    }
}

void SRDWindowManager::resize_window(SRDWindow* window, int width, int height) {
    if (window) {
        window->setSize(width, height);
        update_layout_for_window(window);
        std::cout << "SRDWindowManager: Resized window " << window->getId() << " to " << width << "x" << height << std::endl;
    }
}

void SRDWindowManager::toggle_window_floating(SRDWindow* window) {
    if (!window) return;
    
    auto it = floating_windows_.find(window);
    if (it != floating_windows_.end()) {
        // Window is floating, make it tiled
        floating_windows_.erase(it);
        std::cout << "SRDWindowManager: Window " << window->getId() << " is now tiled" << std::endl;
    } else {
        // Window is tiled, make it floating
        floating_windows_.insert(window);
        std::cout << "SRDWindowManager: Window " << window->getId() << " is now floating" << std::endl;
    }
    
    // Re-arrange windows to reflect the change
    arrange_windows();
}

bool SRDWindowManager::is_window_floating(SRDWindow* window) const {
    if (!window) return false;
    return floating_windows_.find(window) != floating_windows_.end();
}

// Window dragging and resizing implementation
void SRDWindowManager::start_window_drag(SRDWindow* window, int start_x, int start_y) {
    if (!window || dragging_window_) return;
    
    dragging_window_ = window;
    drag_start_x_ = start_x;
    drag_start_y_ = start_y;
    drag_start_window_x_ = window->getX();
    drag_start_window_y_ = window->getY();
    
    std::cout << "SRDWindowManager: Started dragging window " << window->getId() << std::endl;
}

void SRDWindowManager::start_window_resize(SRDWindow* window, int start_x, int start_y, int edge) {
    if (!window || resizing_window_) return;
    
    resizing_window_ = window;
    resize_start_x_ = start_x;
    resize_start_y_ = start_y;
    resize_start_width_ = window->getWidth();
    resize_start_height_ = window->getHeight();
    resize_edge_ = edge;
    
    std::cout << "SRDWindowManager: Started resizing window " << window->getId() << " edge: " << edge << std::endl;
}

void SRDWindowManager::update_window_drag(int x, int y) {
    if (!dragging_window_) return;
    
    int delta_x = x - drag_start_x_;
    int delta_y = y - drag_start_y_;
    
    int new_x = drag_start_window_x_ + delta_x;
    int new_y = drag_start_window_y_ + delta_y;
    
    // Ensure window stays within monitor bounds
    // TODO: Get actual monitor bounds
    new_x = std::max(0, std::min(new_x, 1920 - dragging_window_->getWidth()));
    new_y = std::max(0, std::min(new_y, 1080 - dragging_window_->getHeight()));
    
    dragging_window_->setPosition(new_x, new_y);
    update_layout_for_window(dragging_window_);
}

void SRDWindowManager::update_window_resize(int x, int y) {
    if (!resizing_window_) return;
    
    int delta_x = x - resize_start_x_;
    int delta_y = y - resize_start_y_;
    
    int new_width = resize_start_width_;
    int new_height = resize_start_height_;
    int new_x = resizing_window_->getX();
    int new_y = resizing_window_->getY();
    
    // Handle different resize edges
    switch (resize_edge_) {
        case 1: // Left edge
            new_width = std::max(100, resize_start_width_ - delta_x);
            new_x = resize_start_x_ + resize_start_width_ - new_width;
            break;
        case 2: // Right edge
            new_width = std::max(100, resize_start_width_ + delta_x);
            break;
        case 3: // Top edge
            new_height = std::max(100, resize_start_height_ - delta_y);
            new_y = resize_start_y_ + resize_start_height_ - new_height;
            break;
        case 4: // Bottom edge
            new_height = std::max(100, resize_start_height_ + delta_y);
            break;
        case 5: // Corner (both width and height)
            new_width = std::max(100, resize_start_width_ + delta_x);
            new_height = std::max(100, resize_start_height_ + delta_y);
            break;
    }
    
    // Ensure minimum size and bounds
    new_width = std::max(100, std::min(new_width, 1920 - new_x));
    new_height = std::max(100, std::min(new_height, 1080 - new_y));
    
    resizing_window_->setPosition(new_x, new_y);
    resizing_window_->setSize(new_width, new_height);
    update_layout_for_window(resizing_window_);
}

void SRDWindowManager::end_window_drag() {
    if (dragging_window_) {
        std::cout << "SRDWindowManager: Ended dragging window " << dragging_window_->getId() << std::endl;
        dragging_window_ = nullptr;
    }
}

void SRDWindowManager::end_window_resize() {
    if (resizing_window_) {
        std::cout << "SRDWindowManager: Ended resizing window " << resizing_window_->getId() << std::endl;
        resizing_window_ = nullptr;
    }
}

// Layout management
void SRDWindowManager::set_layout(int monitor_id, const std::string& layout_name) {
    if (layout_engine_) {
        layout_engine_->set_layout(monitor_id, layout_name);
        std::cout << "SRDWindowManager: Set layout '" << layout_name << "' for monitor " << monitor_id << std::endl;
    }
}

std::string SRDWindowManager::get_layout(int monitor_id) const {
    if (layout_engine_) {
        return layout_engine_->get_layout_name(monitor_id);
    }
    return "dynamic";
}

void SRDWindowManager::arrange_windows() {
    if (layout_engine_) {
        layout_engine_->arrange_all_monitors();
        std::cout << "SRDWindowManager: Arranged all windows" << std::endl;
    }
}

void SRDWindowManager::tile_windows() {
    set_layout(0, "tiling");
    arrange_windows();
}

void SRDWindowManager::arrange_windows_dynamic() {
    set_layout(0, "dynamic");
    arrange_windows();
}

// Key binding system
void SRDWindowManager::bind_key(const std::string& key_combination, std::function<void()> action) {
    key_bindings_[key_combination] = action;
    std::cout << "SRDWindowManager: Bound key '" << key_combination << "'" << std::endl;
}

void SRDWindowManager::unbind_key(const std::string& key_combination) {
    auto it = key_bindings_.find(key_combination);
    if (it != key_bindings_.end()) {
        key_bindings_.erase(it);
        std::cout << "SRDWindowManager: Unbound key '" << key_combination << "'" << std::endl;
    }
}

void SRDWindowManager::handle_key_press(int key_code, int modifiers) {
    std::string key_string = key_code_to_string(key_code, modifiers);
    pressed_keys_[key_code] = modifiers;
    
    std::cout << "SRDWindowManager: Key press " << key_code << " (modifiers: " << modifiers << ") -> '" << key_string << "'" << std::endl;
    
    // Check for key bindings
    auto it = key_bindings_.find(key_string);
    if (it != key_bindings_.end()) {
        std::cout << "SRDWindowManager: Executing key binding for '" << key_string << "'" << std::endl;
        it->second();
    }
}

void SRDWindowManager::handle_key_release(int key_code, int modifiers) {
    pressed_keys_.erase(key_code);
    std::cout << "SRDWindowManager: Key release " << key_code << std::endl;
}

// Integration
void SRDWindowManager::set_layout_engine(LayoutEngine* engine) {
    layout_engine_ = engine;
    std::cout << "SRDWindowManager: Layout engine connected" << std::endl;
}

void SRDWindowManager::set_lua_manager(LuaManager* manager) {
    lua_manager_ = manager;
    std::cout << "SRDWindowManager: Lua manager connected" << std::endl;
}

void SRDWindowManager::set_platform(Platform* platform) {
    platform_ = platform;
    std::cout << "SRDWindowManager: Platform connected" << std::endl;
}

// Legacy input handling (for compatibility)
void SRDWindowManager::handle_key_press(int key_code) {
    handle_key_press(key_code, 0);
}

void SRDWindowManager::handle_key_release(int key_code) {
    handle_key_release(key_code, 0);
}

void SRDWindowManager::handle_mouse_button_press(int button, int x, int y) {
    std::cout << "SRDWindowManager: Mouse button press " << button << " at (" << x << ", " << y << ")" << std::endl;
    
    // Find window under cursor
    SRDWindow* window_under_cursor = find_window_at_position(x, y);
    
    if (window_under_cursor) {
        // Focus the window
        focus_window(window_under_cursor);
        
        // Handle different mouse buttons
        if (button == 1) { // Left button - start drag or resize
            if (is_in_titlebar_area(window_under_cursor, x, y)) {
                // Start dragging from titlebar
                start_window_drag(window_under_cursor, x, y);
            } else if (is_in_resize_area(window_under_cursor, x, y)) {
                // Start resizing
                int edge = get_resize_edge(window_under_cursor, x, y);
                start_window_resize(window_under_cursor, x, y, edge);
            }
        }
    }
}

void SRDWindowManager::handle_mouse_button_release(int button, int x, int y) {
    std::cout << "SRDWindowManager: Mouse button release " << button << " at (" << x << ", " << y << ")" << std::endl;
    
    if (button == 1) { // Left button
        if (is_dragging()) {
            end_window_drag();
        } else if (is_resizing()) {
            end_window_resize();
        }
    }
}

void SRDWindowManager::handle_mouse_motion(int x, int y) {
    // Only log if we're not dragging or resizing to avoid spam
    if (!is_dragging() && !is_resizing()) {
        std::cout << "SRDWindowManager: Mouse motion to (" << x << ", " << y << ")" << std::endl;
    }
    
    // Update drag or resize if active
    if (is_dragging()) {
        update_window_drag(x, y);
    } else if (is_resizing()) {
        update_window_resize(x, y);
    }
}

void SRDWindowManager::handle_event(const Event& event) {
    std::cout << "SRDWindowManager: Handling event type " << static_cast<int>(event.type) << std::endl;
}

// Workspace management
void SRDWindowManager::add_workspace(const std::string& name) {
    Workspace workspace(next_workspace_id_++, name);
    workspaces_.push_back(workspace);
    
    // If this is the first workspace, make it current
    if (workspaces_.size() == 1) {
        current_workspace_ = workspace.id;
        workspace.visible = true;
    }
    
    std::cout << "SRDWindowManager: Added workspace " << workspace.id << " (" << name << ")" << std::endl;
}

void SRDWindowManager::remove_workspace(int workspace_id) {
    auto it = std::find_if(workspaces_.begin(), workspaces_.end(),
                           [workspace_id](const Workspace& w) { return w.id == workspace_id; });
    
    if (it != workspaces_.end()) {
        // Move windows to current workspace if removing current
        if (workspace_id == current_workspace_) {
            for (auto* window : it->windows) {
                move_window_to_workspace(window, current_workspace_);
            }
        }
        
        workspaces_.erase(it);
        std::cout << "SRDWindowManager: Removed workspace " << workspace_id << std::endl;
    }
}

void SRDWindowManager::switch_to_workspace(int workspace_id) {
    auto* workspace = get_workspace(workspace_id);
    if (workspace) {
        // Hide current workspace
        auto* current = get_workspace(current_workspace_);
        if (current) {
            current->visible = false;
        }
        
        // Show new workspace
        current_workspace_ = workspace_id;
        workspace->visible = true;
        
        // Arrange windows on the new workspace
        arrange_workspace_windows(workspace_id);
        
        std::cout << "SRDWindowManager: Switched to workspace " << workspace_id << std::endl;
    }
}

void SRDWindowManager::move_window_to_workspace(SRDWindow* window, int workspace_id) {
    if (!window) return;
    
    auto* target_workspace = get_workspace(workspace_id);
    if (!target_workspace) return;
    
    // Remove from current workspace
    for (auto& workspace : workspaces_) {
        auto it = std::find(workspace.windows.begin(), workspace.windows.end(), window);
        if (it != workspace.windows.end()) {
            workspace.windows.erase(it);
            break;
        }
    }
    
    // Add to target workspace
    target_workspace->windows.push_back(window);
    
    std::cout << "SRDWindowManager: Moved window " << window->getId() 
              << " to workspace " << workspace_id << std::endl;
}

int SRDWindowManager::get_current_workspace() const {
    return current_workspace_;
}

std::vector<Workspace> SRDWindowManager::get_workspaces() const {
    return workspaces_;
}

Workspace* SRDWindowManager::get_workspace(int workspace_id) {
    auto it = std::find_if(workspaces_.begin(), workspaces_.end(),
                           [workspace_id](const Workspace& w) { return w.id == workspace_id; });
    return it != workspaces_.end() ? &(*it) : nullptr;
}

// Helper methods
std::string SRDWindowManager::key_code_to_string(int key_code, int modifiers) const {
    std::string result;
    
    // Add modifiers
    if (modifiers & 0x01) result += "Ctrl+";  // Control
    if (modifiers & 0x02) result += "Shift+"; // Shift
    if (modifiers & 0x04) result += "Alt+";   // Alt
    if (modifiers & 0x08) result += "Mod4+";  // Super/SRDWindows
    
    // Add key
    if (key_code >= 'A' && key_code <= 'Z') {
        result += static_cast<char>(key_code);
    } else if (key_code >= '0' && key_code <= '9') {
        result += static_cast<char>(key_code);
    } else {
        result += "Key" + std::to_string(key_code);
    }
    
    return result;
}

void SRDWindowManager::execute_key_binding(const std::string& key_combination) {
    auto it = key_bindings_.find(key_combination);
    if (it != key_bindings_.end()) {
        it->second();
    }
}

void SRDWindowManager::update_layout_for_window(SRDWindow* window) {
    if (layout_engine_) {
        layout_engine_->update_window(window);
    }
}

void SRDWindowManager::arrange_workspace_windows(int workspace_id) {
    auto* workspace = get_workspace(workspace_id);
    if (!workspace || !layout_engine_) return;
    
    // Get monitor for current workspace (simplified - assuming single monitor for now)
    if (!workspace->windows.empty() && !monitors_.empty()) {
        // Arrange windows using the layout engine
        // TODO: Get the actual monitor for this workspace
        layout_engine_->arrange_on_monitor(monitors_[0]);
    }
}

void SRDWindowManager::update_workspace_visibility() {
    for (auto& workspace : workspaces_) {
        workspace.visible = (workspace.id == current_workspace_);
    }
}

// Window interaction helper methods
SRDWindow* SRDWindowManager::find_window_at_position(int x, int y) const {
    // Find the topmost window at the given position
    // For now, just check if point is within any window bounds
    // TODO: Implement proper z-order checking
    for (auto* window : windows_) {
        if (x >= window->getX() && x < window->getX() + window->getWidth() &&
            y >= window->getY() && y < window->getY() + window->getHeight()) {
            return window;
        }
    }
    return nullptr;
}

bool SRDWindowManager::is_in_titlebar_area(SRDWindow* window, int x, int y) const {
    if (!window) return false;
    
    // Check if point is in the top area of the window (titlebar region)
    // Titlebar is typically the top 20-30 pixels of the window
    int titlebar_height = 30;
    
    return (x >= window->getX() && x < window->getX() + window->getWidth() &&
            y >= window->getY() && y < window->getY() + titlebar_height);
}

bool SRDWindowManager::is_in_resize_area(SRDWindow* window, int x, int y) const {
    if (!window) return false;
    
    // Check if point is near the edges of the window (resize handles)
    int resize_margin = 5;
    
    int left = window->getX();
    int right = left + window->getWidth();
    int top = window->getY();
    int bottom = top + window->getHeight();
    
    return (x <= left + resize_margin || x >= right - resize_margin ||
            y <= top + resize_margin || y >= bottom - resize_margin);
}

int SRDWindowManager::get_resize_edge(SRDWindow* window, int x, int y) const {
    if (!window) return 0;
    
    int resize_margin = 5;
    int left = window->getX();
    int right = left + window->getWidth();
    int top = window->getY();
    int bottom = top + window->getHeight();
    
    bool near_left = (x <= left + resize_margin);
    bool near_right = (x >= right - resize_margin);
    bool near_top = (y <= top + resize_margin);
    bool near_bottom = (y >= bottom - resize_margin);
    
    // Determine which edge or corner
    if (near_left && near_top) return 5;      // Top-left corner
    if (near_right && near_top) return 5;     // Top-right corner
    if (near_left && near_bottom) return 5;   // Bottom-left corner
    if (near_right && near_bottom) return 5;  // Bottom-right corner
    if (near_left) return 1;                  // Left edge
    if (near_right) return 2;                 // Right edge
    if (near_top) return 3;                   // Top edge
    if (near_bottom) return 4;                // Bottom edge
    
    return 0; // No resize edge
}
