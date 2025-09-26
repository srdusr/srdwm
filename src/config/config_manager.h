#ifndef SRDWM_CONFIG_MANAGER_H
#define SRDWM_CONFIG_MANAGER_H

#include <string>
#include <memory>
#include <map>
#include <functional>
#include <vector> // Added missing include

// Forward declarations
class WindowManager;

// Configuration manager for srdwm
class ConfigManager {
public:
    struct KeyBinding {
        std::string key;
        std::string command;
        std::string description;
    };

    struct LayoutConfig {
        std::string name;
        std::string type; // "tiling", "dynamic", "floating"
        std::map<std::string, std::string> properties;
    };

    struct ThemeConfig {
        std::string name;
        std::map<std::string, std::string> colors;
        std::map<std::string, std::string> fonts;
        std::map<std::string, int> dimensions;
    };

    ConfigManager();
    ~ConfigManager();

    // Configuration loading
    bool load_config(const std::string& config_path);
    bool reload_config();
    
    // Configuration access
    std::string get_string(const std::string& key, const std::string& default_value = "") const;
    int get_int(const std::string& key, int default_value = 0) const;
    bool get_bool(const std::string& key, bool default_value = false) const;
    double get_float(const std::string& key, double default_value = 0.0) const;
    
    // Key bindings
    std::vector<KeyBinding> get_key_bindings() const;
    bool add_key_binding(const std::string& key, const std::string& command, const std::string& description = "");
    bool remove_key_binding(const std::string& key);
    
    // Layout configuration
    std::vector<LayoutConfig> get_layout_configs() const;
    LayoutConfig get_layout_config(const std::string& name) const;
    
    // Theme configuration
    ThemeConfig get_theme_config() const;
    bool set_theme(const std::string& theme_name);
    
    // Window rules
    struct WindowRule {
        std::string match_type; // "class", "title", "role"
        std::string match_value;
        std::map<std::string, std::string> properties;
    };
    
    std::vector<WindowRule> get_window_rules() const;
    bool add_window_rule(const WindowRule& rule);
    
    // Configuration validation
    bool validate_config() const;
    std::vector<std::string> get_validation_errors() const;

private:
    std::string config_path_;
    std::map<std::string, std::string> string_values_;
    std::map<std::string, int> int_values_;
    std::map<std::string, bool> bool_values_;
    std::map<std::string, double> float_values_;
    
    std::vector<KeyBinding> key_bindings_;
    std::vector<LayoutConfig> layout_configs_;
    std::vector<WindowRule> window_rules_;
    ThemeConfig current_theme_;
    
    std::vector<std::string> validation_errors_;
    
    // Private methods
    void parse_config_file(const std::string& content);
    void setup_default_config();
    bool validate_key_binding(const KeyBinding& binding) const;
    bool validate_layout_config(const LayoutConfig& config) const;
    bool validate_window_rule(const WindowRule& rule) const;
    
    // Lua integration (placeholder for future implementation)
    bool execute_lua_config(const std::string& lua_code);
};

#endif // SRDWM_CONFIG_MANAGER_H

