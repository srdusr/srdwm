#ifndef SRDWM_LUA_MANAGER_H
#define SRDWM_LUA_MANAGER_H

#include <memory>
#include <string>
#include <vector>
#include <map>
#include <functional>
#include <lua.hpp>

// Forward declarations
class SRDWindow;
class SRDWindowManager;
class LayoutEngine;

// Include platform header for full definition
#include "../platform/platform.h"

// Include layout engine header
#include "../layouts/layout_engine.h"

// Lua callback function type
using LuaCallback = std::function<void()>;

// Lua configuration value
struct LuaConfigValue {
    enum class Type {
        String,
        Number,
        Boolean,
        Table,
        Function
    };
    
    Type type;
    std::string string_value;
    double number_value;
    bool bool_value;
    std::map<std::string, LuaConfigValue> table_value;
    std::string function_name;
};

// Lua manager for SRDWM
class LuaManager {
public:
    LuaManager();
    ~LuaManager();

    // Initialization and cleanup
    bool initialize();
    void shutdown();
    
    // Configuration loading
    bool load_config_file(const std::string& path);
    bool load_config_directory(const std::string& dir_path);
    bool reload_config();
    
    // Configuration access
    LuaConfigValue get_config(const std::string& key) const;
    std::string get_string(const std::string& key, const std::string& default_value = "") const;
    int get_int(const std::string& key, int default_value = 0) const;
    bool get_bool(const std::string& key, bool default_value = false) const;
    double get_float(const std::string& key, double default_value = 0.0) const;
    
    // Configuration modification
    void set_config(const std::string& key, const LuaConfigValue& value);
    void set_string(const std::string& key, const std::string& value);
    void set_int(const std::string& key, int value);
    void set_bool(const std::string& key, bool value);
    void set_float(const std::string& key, double value);
    
    // Key binding system
    bool bind_key(const std::string& key_combination, const std::string& lua_function);
    bool unbind_key(const std::string& key_combination);
    std::vector<std::string> get_bound_keys() const;
    
    // Layout system
    bool configure_layout(const std::string& layout_name, const std::map<std::string, LuaConfigValue>& config);
    bool configure_layout(const std::string& layout_name, const std::map<std::string, std::string>& config);
    bool register_custom_layout(const std::string& name, const std::string& lua_function);
    std::vector<std::string> get_available_layouts() const;
    bool set_layout(int monitor_id, const std::string& layout_name);
    std::string get_layout_name(int monitor_id) const;
    void set_layout_engine(LayoutEngine* engine);
    
    // Theme system
    bool set_theme_colors(const std::map<std::string, std::string>& colors);
    bool set_theme_decorations(const std::map<std::string, LuaConfigValue>& decorations);
    
    // SRDWindow decoration controls
    bool set_window_decorations(const std::string& window_id, bool enabled);
    bool set_window_border_color(const std::string& window_id, int r, int g, int b);
    bool set_window_border_width(const std::string& window_id, int width);
    bool get_window_decorations(const std::string& window_id) const;
    
    // SRDWindow state controls
    bool set_window_floating(const std::string& window_id, bool floating);
    bool toggle_window_floating(const std::string& window_id);
    bool is_window_floating(const std::string& window_id) const;
    std::map<std::string, std::string> get_theme_colors() const;
    
    // SRDWindow rules
    bool add_window_rule(const std::map<std::string, LuaConfigValue>& rule);
    bool remove_window_rule(const std::string& rule_name);
    std::vector<std::map<std::string, LuaConfigValue>> get_window_rules() const;
    
    // Utility functions
    bool execute_lua_code(const std::string& code);
    bool validate_lua_syntax(const std::string& code);
    std::vector<std::string> get_lua_errors() const;
    
    // Configuration validation
    bool validate_config() const;
    std::vector<std::string> get_validation_errors() const;
    
    // Reset functionality
    void reset_config(const std::string& key);
    void reset_all_configs();
    void reset_category(const std::string& category);

private:
    lua_State* L_;
    std::map<std::string, LuaConfigValue> config_values_;
    std::map<std::string, std::string> key_bindings_;
    std::vector<std::string> lua_errors_;
    std::vector<std::string> validation_errors_;
    
    // SRDWindow manager reference
    SRDWindowManager* window_manager_;
    LayoutEngine* layout_engine_;
    Platform* platform_;
    
    // Private methods
    void setup_lua_environment();
    void register_srd_module();
    void register_window_functions();
    void register_layout_functions();
    void register_theme_functions();
    void register_utility_functions();
    
    // Configuration helpers
    void parse_config_value(lua_State* L, int index, const std::string& key);
    void save_config_to_lua();
    void load_default_config();
    
    // Error handling
    void add_lua_error(const std::string& error);
    void add_validation_error(const std::string& error) const;
    void clear_errors();
    
    // File watching
    void setup_file_watcher();
    void on_config_file_changed(const std::string& path);
    
    // Helper functions
    std::string get_config_file_path() const;
    std::string get_default_config_path() const;
    bool create_default_config() const;
};

// Global Lua manager instance
extern std::unique_ptr<LuaManager> g_lua_manager;

#endif // SRDWM_LUA_MANAGER_H


