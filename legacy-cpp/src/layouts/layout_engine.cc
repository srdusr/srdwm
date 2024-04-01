#include "layout_engine.h"
#include <iostream>
#include <algorithm>

LayoutEngine::LayoutEngine() {
    std::cout << "LayoutEngine: Initializing..." << std::endl;
}

LayoutEngine::~LayoutEngine() {
    std::cout << "LayoutEngine: Shutting down..." << std::endl;
}

// Layout management
bool LayoutEngine::set_layout(int monitor_id, LayoutType layout_type) {
    active_layouts_[monitor_id] = layout_type;
    std::cout << "LayoutEngine: Set layout " << layout_type_to_string(layout_type) 
              << " for monitor " << monitor_id << std::endl;
    return true;
}

bool LayoutEngine::set_layout(int monitor_id, const std::string& layout_name) {
    LayoutType layout_type = string_to_layout_type(layout_name);
    if (layout_type != LayoutType::TILING && layout_type != LayoutType::DYNAMIC && layout_type != LayoutType::FLOATING) {
        std::cerr << "LayoutEngine: Unknown layout type: " << layout_name << std::endl;
        return false;
    }
    return set_layout(monitor_id, layout_type);
}

LayoutType LayoutEngine::get_layout(int monitor_id) const {
    auto it = active_layouts_.find(monitor_id);
    if (it != active_layouts_.end()) {
        return it->second;
    }
    return LayoutType::DYNAMIC; // Default layout
}

std::string LayoutEngine::get_layout_name(int monitor_id) const {
    return layout_type_to_string(get_layout(monitor_id));
}

// Layout configuration
bool LayoutEngine::configure_layout(const std::string& layout_name, const std::map<std::string, std::string>& config) {
    layout_configs_[layout_name] = config;
    std::cout << "LayoutEngine: Configured layout '" << layout_name << "' with " 
              << config.size() << " parameters" << std::endl;
    return true;
}

bool LayoutEngine::register_custom_layout(const std::string& name, std::function<void(const std::vector<SRDWindow*>&, const Monitor&)> layout_func) {
    custom_layouts_[name] = layout_func;
    std::cout << "LayoutEngine: Registered custom layout '" << name << "'" << std::endl;
    return true;
}

// SRDWindow management
void LayoutEngine::add_window(SRDWindow* window) {
    if (window && std::find(windows_.begin(), windows_.end(), window) == windows_.end()) {
        windows_.push_back(window);
        std::cout << "LayoutEngine: Added window " << window->getId() << std::endl;
    }
}

void LayoutEngine::remove_window(SRDWindow* window) {
    auto it = std::find(windows_.begin(), windows_.end(), window);
    if (it != windows_.end()) {
        windows_.erase(it);
        std::cout << "LayoutEngine: Removed window " << window->getId() << std::endl;
    }
}

void LayoutEngine::update_window(SRDWindow* window) {
    // Trigger rearrangement for the monitor this window is on
    // For now, just log the update
    std::cout << "LayoutEngine: Updated window " << window->getId() << std::endl;
}

// Monitor management
void LayoutEngine::add_monitor(const Monitor& monitor) {
    // Check if monitor already exists
    auto it = std::find_if(monitors_.begin(), monitors_.end(), 
                          [&](const Monitor& m) { return m.id == monitor.id; });
    if (it == monitors_.end()) {
        monitors_.push_back(monitor);
        // Set default layout for new monitor
        active_layouts_[monitor.id] = LayoutType::DYNAMIC;
        std::cout << "LayoutEngine: Added monitor " << monitor.id << std::endl;
    }
}

void LayoutEngine::remove_monitor(int monitor_id) {
    auto it = std::find_if(monitors_.begin(), monitors_.end(), 
                          [monitor_id](const Monitor& m) { return m.id == monitor_id; });
    if (it != monitors_.end()) {
        monitors_.erase(it);
        active_layouts_.erase(monitor_id);
        std::cout << "LayoutEngine: Removed monitor " << monitor_id << std::endl;
    }
}

void LayoutEngine::update_monitor(const Monitor& monitor) {
    auto it = std::find_if(monitors_.begin(), monitors_.end(), 
                          [&](const Monitor& m) { return m.id == monitor.id; });
    if (it != monitors_.end()) {
        *it = monitor;
        std::cout << "LayoutEngine: Updated monitor " << monitor.id << std::endl;
    }
}

// Arrangement
void LayoutEngine::arrange_on_monitor(const Monitor& monitor) {
    if (active_layouts_.count(monitor.id)) {
        LayoutType current_layout_type = active_layouts_[monitor.id];
        std::vector<SRDWindow*> windows_on_monitor = get_windows_on_monitor(monitor.id);
        
        std::cout << "LayoutEngine: Arranging " << windows_on_monitor.size() 
                  << " windows on monitor " << monitor.id 
                  << " with layout " << layout_type_to_string(current_layout_type) << std::endl;

        if (current_layout_type == LayoutType::TILING) {
            tiling_layout_.arrange_windows(windows_on_monitor, monitor);
        } else if (current_layout_type == LayoutType::DYNAMIC) {
            dynamic_layout_.arrange_windows(windows_on_monitor, monitor);
        } else if (current_layout_type == LayoutType::FLOATING) {
            // Floating layout - windows keep their current positions
            std::cout << "LayoutEngine: Floating layout - no arrangement needed" << std::endl;
        }
    }
}

void LayoutEngine::arrange_all_monitors() {
    for (const auto& monitor : monitors_) {
        arrange_on_monitor(monitor);
    }
}

// Utility
std::vector<std::string> LayoutEngine::get_available_layouts() const {
    return {"tiling", "dynamic", "floating"};
}

std::vector<SRDWindow*> LayoutEngine::get_windows_on_monitor(int monitor_id) const {
    std::vector<SRDWindow*> windows_on_monitor;
    
    // Find the monitor
    auto monitor_it = std::find_if(monitors_.begin(), monitors_.end(), 
                                  [monitor_id](const Monitor& m) { return m.id == monitor_id; });
    if (monitor_it == monitors_.end()) {
        return windows_on_monitor;
    }
    
    // Get windows that are on this monitor
    for (SRDWindow* window : windows_) {
        if (is_window_on_monitor(window, *monitor_it)) {
            windows_on_monitor.push_back(window);
        }
    }
    
    return windows_on_monitor;
}

// Helper methods
LayoutType LayoutEngine::string_to_layout_type(const std::string& name) const {
    if (name == "tiling") return LayoutType::TILING;
    if (name == "dynamic") return LayoutType::DYNAMIC;
    if (name == "floating") return LayoutType::FLOATING;
    return LayoutType::DYNAMIC; // Default
}

std::string LayoutEngine::layout_type_to_string(LayoutType type) const {
    switch (type) {
        case LayoutType::TILING: return "tiling";
        case LayoutType::DYNAMIC: return "dynamic";
        case LayoutType::FLOATING: return "floating";
        default: return "dynamic";
    }
}

bool LayoutEngine::is_window_on_monitor(const SRDWindow* window, const Monitor& monitor) const {
    if (!window) return false;
    
    // Simple check: window center is within monitor bounds
    int window_center_x = window->getX() + window->getWidth() / 2;
    int window_center_y = window->getY() + window->getHeight() / 2;
    
    return window_center_x >= monitor.x && window_center_x < monitor.x + monitor.width &&
           window_center_y >= monitor.y && window_center_y < monitor.y + monitor.height;
}
