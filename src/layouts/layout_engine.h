#ifndef SRDWM_LAYOUT_ENGINE_H
#define SRDWM_LAYOUT_ENGINE_H

#include "layout.h"
#include "tiling_layout.h"
#include "dynamic_layout.h"
#include <vector>
#include <map>
#include <string>
#include <functional>

class SRDWindow; // Forward declaration to avoid circular dependency

enum class LayoutType {
    TILING,
    DYNAMIC,
    FLOATING
    // Add other layout types here later
};

class LayoutEngine {
public:
    LayoutEngine();
    ~LayoutEngine();

    // Layout management
    bool set_layout(int monitor_id, LayoutType layout_type);
    bool set_layout(int monitor_id, const std::string& layout_name);
    LayoutType get_layout(int monitor_id) const;
    std::string get_layout_name(int monitor_id) const;
    
    // Layout configuration
    bool configure_layout(const std::string& layout_name, const std::map<std::string, std::string>& config);
    bool register_custom_layout(const std::string& name, std::function<void(const std::vector<SRDWindow*>&, const Monitor&)> layout_func);
    
    // SRDWindow management
    void add_window(SRDWindow* window);
    void remove_window(SRDWindow* window);
    void update_window(SRDWindow* window);
    
    // Monitor management
    void add_monitor(const Monitor& monitor);
    void remove_monitor(int monitor_id);
    void update_monitor(const Monitor& monitor);
    
    // Arrangement
    void arrange_on_monitor(const Monitor& monitor);
    void arrange_all_monitors();
    
    // Utility
    std::vector<std::string> get_available_layouts() const;
    std::vector<SRDWindow*> get_windows_on_monitor(int monitor_id) const;

private:
    // Member variables for layout state
    std::vector<Monitor> monitors_;
    std::vector<SRDWindow*> windows_;
    TilingLayout tiling_layout_;
    DynamicLayout dynamic_layout_;
    std::map<int, LayoutType> active_layouts_; // Map monitor ID to active layout type
    std::map<std::string, std::function<void(const std::vector<SRDWindow*>&, const Monitor&)>> custom_layouts_;
    std::map<std::string, std::map<std::string, std::string>> layout_configs_;
    
    // Helper methods
    LayoutType string_to_layout_type(const std::string& name) const;
    std::string layout_type_to_string(LayoutType type) const;
    bool is_window_on_monitor(const SRDWindow* window, const Monitor& monitor) const;
};

#endif // SRDWM_LAYOUT_ENGINE_H
