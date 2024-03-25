#include "lua_manager.h"
#include <iostream>
#include <fstream>
#include <filesystem>
#include <algorithm>
#include <cstring>

// Global Lua manager instance
std::unique_ptr<LuaManager> g_lua_manager;

// Default configuration values
namespace Defaults {
    std::map<std::string, LuaConfigValue> create_default_config() {
        std::map<std::string, LuaConfigValue> config;
        
        config["general.default_layout"] = {LuaConfigValue::Type::String, "dynamic", 0.0, false, {}, ""};
        config["general.smart_placement"] = {LuaConfigValue::Type::Boolean, "", 0.0, true, {}, ""};
        config["general.window_gap"] = {LuaConfigValue::Type::Number, "", 8.0, false, {}, ""};
        config["general.border_width"] = {LuaConfigValue::Type::Number, "", 2.0, false, {}, ""};
        config["general.animations"] = {LuaConfigValue::Type::Boolean, "", 0.0, true, {}, ""};
        config["general.animation_duration"] = {LuaConfigValue::Type::Number, "", 200.0, false, {}, ""};
        config["general.focus_follows_mouse"] = {LuaConfigValue::Type::Boolean, "", 0.0, false, {}, ""};
        config["general.mouse_follows_focus"] = {LuaConfigValue::Type::Boolean, "", 0.0, true, {}, ""};
        config["general.auto_raise"] = {LuaConfigValue::Type::Boolean, "", 0.0, false, {}, ""};
        config["general.auto_focus"] = {LuaConfigValue::Type::Boolean, "", 0.0, true, {}, ""};
        
        config["monitor.primary_layout"] = {LuaConfigValue::Type::String, "dynamic", 0.0, false, {}, ""};
        config["monitor.secondary_layout"] = {LuaConfigValue::Type::String, "tiling", 0.0, false, {}, ""};
        config["monitor.auto_detect"] = {LuaConfigValue::Type::Boolean, "", 0.0, true, {}, ""};
        config["monitor.primary_workspace"] = {LuaConfigValue::Type::Number, "", 1.0, false, {}, ""};
        config["monitor.workspace_count"] = {LuaConfigValue::Type::Number, "", 10.0, false, {}, ""};
        
        config["performance.vsync"] = {LuaConfigValue::Type::Boolean, "", 0.0, true, {}, ""};
        config["performance.max_fps"] = {LuaConfigValue::Type::Number, "", 60.0, false, {}, ""};
        config["performance.window_cache_size"] = {LuaConfigValue::Type::Number, "", 100.0, false, {}, ""};
        config["performance.event_queue_size"] = {LuaConfigValue::Type::Number, "", 1000.0, false, {}, ""};
        
        config["debug.logging"] = {LuaConfigValue::Type::Boolean, "", 0.0, true, {}, ""};
        config["debug.log_level"] = {LuaConfigValue::Type::String, "info", 0.0, false, {}, ""};
        config["debug.profile"] = {LuaConfigValue::Type::Boolean, "", 0.0, false, {}, ""};
        config["debug.trace_events"] = {LuaConfigValue::Type::Boolean, "", 0.0, false, {}, ""};
        
        return config;
    }
}

LuaManager::LuaManager() 
    : L_(nullptr)
    , window_manager_(nullptr)
    , layout_engine_(nullptr)
    , platform_(nullptr) {
    
    // Initialize with default values
    config_values_ = Defaults::create_default_config();
}

LuaManager::~LuaManager() {
    shutdown();
}

bool LuaManager::initialize() {
    std::cout << "Initializing Lua manager..." << std::endl;
    
    // Create Lua state
    L_ = luaL_newstate();
    if (!L_) {
        std::cerr << "Failed to create Lua state" << std::endl;
        return false;
    }
    
    std::cout << "Lua state created successfully" << std::endl;
    
    // Open Lua libraries
    luaL_openlibs(L_);
    std::cout << "Lua libraries opened" << std::endl;
    
    // Setup Lua environment
    setup_lua_environment();
    std::cout << "Lua environment setup complete" << std::endl;
    
    // Register SRD module
    register_srd_module();
    std::cout << "SRD module registration complete" << std::endl;
    
    // Load default configuration
    load_default_config();
    std::cout << "Default configuration loaded" << std::endl;
    
    std::cout << "Lua manager initialized successfully" << std::endl;
    return true;
}

void LuaManager::shutdown() {
    if (L_) {
        lua_close(L_);
        L_ = nullptr;
    }
    
    config_values_.clear();
    key_bindings_.clear();
    lua_errors_.clear();
    validation_errors_.clear();
}

void LuaManager::setup_lua_environment() {
    // Set Lua path to include SRDWM modules
    std::string lua_path = "package.path = package.path .. ';";
    lua_path += get_config_file_path() + "/?.lua;";
    lua_path += get_config_file_path() + "/?/init.lua'";
    
    if (luaL_dostring(L_, lua_path.c_str()) != LUA_OK) {
        std::cerr << "Failed to set Lua path: " << lua_tostring(L_, -1) << std::endl;
        lua_pop(L_, 1);
    }
    
    // Set global error handler
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        std::string error_msg = lua_tostring(L, 1);
        std::cerr << "Lua error: " << error_msg << std::endl;
        return 0;
    });
    lua_setglobal(L_, "error_handler");
}

void LuaManager::register_srd_module() {
    std::cout << "Creating srd table..." << std::endl;
    
    // Create srd table
    lua_newtable(L_);
    
    std::cout << "Registering window functions..." << std::endl;
    // Register window functions
    register_window_functions();
    
    std::cout << "Registering layout functions..." << std::endl;
    // Register layout functions
    register_layout_functions();
    
    std::cout << "Registering theme functions..." << std::endl;
    // Register theme functions
    register_theme_functions();
    
    std::cout << "Registering utility functions..." << std::endl;
    // Register utility functions
    register_utility_functions();
    
    std::cout << "Setting global srd variable..." << std::endl;
    // Set global srd variable
    lua_setglobal(L_, "srd");
    
    // Store reference to this LuaManager instance for callbacks
    lua_pushlightuserdata(L_, this);
    lua_setglobal(L_, "_lua_manager_instance");
    
    std::cout << "SRD module registered successfully" << std::endl;
}

void LuaManager::register_window_functions() {
    // Create window subtable
    lua_newtable(L_);
    
    // Register window functions
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.window.focused()
        // Get the global Lua manager instance
        lua_getglobal(L, "_lua_manager_instance");
        if (lua_islightuserdata(L, -1)) {
            LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
            if (manager) {
                // Return focused window info
                lua_newtable(L);
                lua_pushstring(L, "id");
                lua_pushinteger(L, 0); // Placeholder
                lua_settable(L, -3);
                lua_pushstring(L, "title");
                lua_pushstring(L, "Focused SRDWindow");
                lua_settable(L, -3);
                return 1;
            }
        }
        lua_pop(L, 1);
        lua_pushnil(L);
        return 1;
    });
    lua_setfield(L_, -2, "focused");
    
    // SRDWindow decoration controls
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.window.set_decorations(window_id, enabled)
        const char* window_id = lua_tostring(L, 1);
        bool enabled = lua_toboolean(L, 2);
        
        lua_getglobal(L, "_lua_manager_instance");
        if (lua_islightuserdata(L, -1)) {
            LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
            if (manager && window_id) {
                manager->set_window_decorations(window_id, enabled);
            }
        }
        lua_pop(L, 1);
        return 0;
    });
    lua_setfield(L_, -2, "set_decorations");
    
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.window.set_border_color(window_id, r, g, b)
        const char* window_id = lua_tostring(L, 1);
        int r = lua_tointeger(L, 2);
        int g = lua_tointeger(L, 3);
        int b = lua_tointeger(L, 4);
        
        lua_getglobal(L, "_lua_manager_instance");
        if (lua_islightuserdata(L, -1)) {
            LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
            if (manager && window_id) {
                manager->set_window_border_color(window_id, r, g, b);
            }
        }
        lua_pop(L, 1);
        return 0;
    });
    lua_setfield(L_, -2, "set_border_color");
    
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.window.set_border_width(window_id, width)
        const char* window_id = lua_tostring(L, 1);
        int width = lua_tointeger(L, 2);
        
        lua_getglobal(L, "_lua_manager_instance");
        if (lua_islightuserdata(L, -1)) {
            LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
            if (manager && window_id) {
                manager->set_window_border_width(window_id, width);
            }
        }
        lua_pop(L, 1);
        return 0;
    });
    lua_setfield(L_, -2, "set_border_width");
    
    // SRDWindow state controls
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.window.set_floating(window_id, floating)
        const char* window_id = lua_tostring(L, 1);
        bool floating = lua_toboolean(L, 2);
        
        lua_getglobal(L, "_lua_manager_instance");
        if (lua_islightuserdata(L, -1)) {
            LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
            if (manager && window_id) {
                manager->set_window_floating(window_id, floating);
            }
        }
        lua_pop(L, 1);
        return 0;
    });
    lua_setfield(L_, -2, "set_floating");
    
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.window.toggle_floating(window_id)
        const char* window_id = lua_tostring(L, 1);
        
        lua_getglobal(L, "_lua_manager_instance");
        if (lua_islightuserdata(L, -1)) {
            LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
            if (manager && window_id) {
                manager->toggle_window_floating(window_id);
            }
        }
        lua_pop(L, 1);
        return 0;
    });
    lua_setfield(L_, -2, "toggle_floating");
    
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.window.is_floating(window_id)
        const char* window_id = lua_tostring(L, 1);
        
        lua_getglobal(L, "_lua_manager_instance");
        if (lua_islightuserdata(L, -1)) {
            LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
            if (manager && window_id) {
                bool floating = manager->is_window_floating(window_id);
                lua_pushboolean(L, floating);
                lua_pop(L, 1); // Pop userdata
                return 1;
            }
        }
        lua_pop(L, 1);
        lua_pushboolean(L, false);
        return 1;
    });
    lua_setfield(L_, -2, "is_floating");
    
    // Set window subtable in the srd table (which is at index -2)
    lua_setfield(L_, -2, "window");
}

void LuaManager::register_layout_functions() {
    // Create layout subtable
    lua_newtable(L_);
    
    // Register layout functions
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.layout.set(layout_name)
        const char* layout_name = lua_tostring(L, 1);
        if (layout_name) {
            // Get the global Lua manager instance
            lua_getglobal(L, "_lua_manager_instance");
            if (lua_islightuserdata(L, -1)) {
                LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
                if (manager) {
                    // For now, set layout on monitor 0 (primary monitor)
                    manager->set_layout(0, layout_name);
                }
            }
            lua_pop(L, 1); // Pop the userdata
            std::cout << "Switching to layout: " << layout_name << std::endl;
        }
        return 0;
    });
    lua_setfield(L_, -2, "set");
    
    // Register configure function
    lua_pushcclosure(L_, [](lua_State* L) -> int {
        // srd.layout.configure(layout_name, config_table)
        const char* layout_name = lua_tostring(L, 1);
        if (layout_name && lua_istable(L, 2)) {
            // Get the global Lua manager instance
            lua_getglobal(L, "_lua_manager_instance");
            if (lua_islightuserdata(L, -1)) {
                LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
                if (manager) {
                    // Convert Lua table to C++ map
                    std::map<std::string, std::string> config;
                    lua_pushnil(L); // First key
                    while (lua_next(L, 2) != 0) {
                        if (lua_isstring(L, -2) && lua_isstring(L, -1)) {
                            const char* key = lua_tostring(L, -2);
                            const char* value = lua_tostring(L, -1);
                            config[key] = value;
                        }
                        lua_pop(L, 1); // Remove value, keep key for next iteration
                    }
                    lua_pop(L, 1); // Remove the userdata
                    
                    // Configure the layout
                    manager->configure_layout(layout_name, config);
                }
            } else {
                lua_pop(L, 1);
            }
            std::cout << "Configuring layout: " << layout_name << std::endl;
        }
        return 0;
    }, 0);
    lua_setfield(L_, -2, "configure");
    
    // Set layout subtable in the srd table (which is at index -2)
    lua_setfield(L_, -2, "layout");
}

void LuaManager::register_theme_functions() {
    // Create theme subtable
    lua_newtable(L_);
    
    // Register theme functions
    lua_pushcclosure(L_, [](lua_State* L) -> int {
        // srd.theme.set_colors(colors_table)
        if (lua_istable(L, 1)) {
            // Get the global Lua manager instance
            lua_getglobal(L, "_lua_manager_instance");
            if (lua_islightuserdata(L, -1)) {
                LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
                if (manager) {
                    // Convert Lua table to C++ map
                    std::map<std::string, std::string> colors;
                    lua_pushnil(L); // First key
                    while (lua_next(L, 1) != 0) {
                        if (lua_isstring(L, -2) && lua_isstring(L, -1)) {
                            const char* key = lua_tostring(L, -2);
                            const char* value = lua_tostring(L, -1);
                            colors[key] = value;
                        }
                        lua_pop(L, 1); // Remove value, keep key for next iteration
                    }
                    lua_pop(L, 1); // Remove the userdata
                    
                    // Set theme colors
                    for (const auto& [key, value] : colors) {
                        manager->set_string("theme." + key, value);
                    }
                }
            } else {
                lua_pop(L, 1);
            }
            std::cout << "Setting theme colors" << std::endl;
        }
        return 0;
    }, 0);
    lua_setfield(L_, -2, "set_colors");
    
    // Set theme subtable in the srd table (which is at index -2)
    lua_setfield(L_, -2, "theme");
}

void LuaManager::register_utility_functions() {
    // Register utility functions directly in the srd table (which is at index -1)
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.set(key, value)
        const char* key = lua_tostring(L, 1);
        if (key) {
            // Get the global Lua manager instance
            lua_getglobal(L, "_lua_manager_instance");
            if (lua_islightuserdata(L, -1)) {
                LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
                if (manager) {
                    // Handle different value types
                    if (lua_isstring(L, 2)) {
                        const char* value = lua_tostring(L, 2);
                        manager->set_string(key, value);
                    } else if (lua_isnumber(L, 2)) {
                        double value = lua_tonumber(L, 2);
                        manager->set_float(key, value);
                    } else if (lua_isboolean(L, 2)) {
                        bool value = lua_toboolean(L, 2);
                        manager->set_bool(key, value);
                    }
                }
            }
            lua_pop(L, 1); // Remove the userdata
            std::cout << "Setting config: " << key << std::endl;
        }
        return 0;
    });
    lua_setfield(L_, -2, "set");
    
    // Register bind function
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.bind(key, function)
        const char* key = lua_tostring(L, 1);
        if (key && lua_isfunction(L, 2)) {
            // Get the global Lua manager instance
            lua_getglobal(L, "_lua_manager_instance");
            if (lua_islightuserdata(L, -1)) {
                LuaManager* manager = static_cast<LuaManager*>(lua_touserdata(L, -1));
                if (manager) {
                    // Store the function reference
                    // For now, just store the key binding
                    manager->bind_key(key, "lua_function");
                }
            }
            lua_pop(L, 1); // Remove the userdata
            std::cout << "Binding key: " << key << std::endl;
        }
        return 0;
    });
    lua_setfield(L_, -2, "bind");
    
    // Register load function
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.load(module_name)
        const char* module_name = lua_tostring(L, 1);
        if (module_name) {
            // TODO: Implement module loading
            std::cout << "Loading module: " << module_name << std::endl;
        }
        return 0;
    });
    lua_setfield(L_, -2, "load");
    
    // Register spawn function
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.spawn(command)
        const char* command = lua_tostring(L, 1);
        if (command) {
            // TODO: Implement command spawning
            std::cout << "Spawning command: " << command << std::endl;
        }
        return 0;
    });
    lua_setfield(L_, -2, "spawn");
    
    // Register notify function
    lua_pushcfunction(L_, [](lua_State* L) -> int {
        // srd.notify(message, level)
        const char* message = lua_tostring(L, 1);
        const char* level = lua_tostring(L, 2);
        if (message) {
            std::cout << "Notification [" << (level ? level : "info") << "]: " << message << std::endl;
        }
        return 0;
    });
    lua_setfield(L_, -2, "notify");
}

bool LuaManager::load_config_file(const std::string& path) {
    if (!L_) {
        std::cerr << "Lua manager not initialized" << std::endl;
        return false;
    }
    
    std::cout << "Loading config file: " << path << std::endl;
    
    // Test if srd module exists before loading
    lua_getglobal(L_, "srd");
    if (lua_isnil(L_, -1)) {
        std::cerr << "ERROR: srd module is nil before loading config!" << std::endl;
        lua_pop(L_, 1);
        return false;
    } else {
        std::cout << "srd module exists before loading config" << std::endl;
        lua_pop(L_, 1);
    }
    
    // Load and execute Lua file
    if (luaL_dofile(L_, path.c_str()) != LUA_OK) {
        std::string error = lua_tostring(L_, -1);
        add_lua_error("Failed to load config file: " + error);
        lua_pop(L_, 1);
        return false;
    }
    
    std::cout << "Config file loaded successfully: " << path << std::endl;
    return true;
}

bool LuaManager::load_config_directory(const std::string& dir_path) {
    std::cout << "Loading config directory: " << dir_path << std::endl;
    
    try {
        std::filesystem::path config_dir(dir_path);
        if (!std::filesystem::exists(config_dir)) {
            std::cerr << "Config directory does not exist: " << dir_path << std::endl;
            return false;
        }
        
        // Load init.lua first if it exists
        std::filesystem::path init_file = config_dir / "init.lua";
        if (std::filesystem::exists(init_file)) {
            if (!load_config_file(init_file.string())) {
                return false;
            }
        }
        
        // Load other .lua files
        for (const auto& entry : std::filesystem::directory_iterator(config_dir)) {
            if (entry.is_regular_file() && entry.path().extension() == ".lua") {
                if (entry.path().filename() != "init.lua") {
                    if (!load_config_file(entry.path().string())) {
                        std::cerr << "Failed to load config file: " << entry.path() << std::endl;
                        // Continue loading other files
                    }
                }
            }
        }
        
        std::cout << "Config directory loaded successfully: " << dir_path << std::endl;
        return true;
        
    } catch (const std::exception& e) {
        std::cerr << "Error loading config directory: " << e.what() << std::endl;
        return false;
    }
}

bool LuaManager::reload_config() {
    std::cout << "Reloading configuration..." << std::endl;
    
    // Clear current configuration
    config_values_.clear();
    key_bindings_.clear();
    clear_errors();
    
    // Reload default configuration
    load_default_config();
    
    // Reload user configuration
    std::string config_path = get_config_file_path();
    if (!load_config_directory(config_path)) {
        std::cerr << "Failed to reload configuration" << std::endl;
        return false;
    }
    
    std::cout << "Configuration reloaded successfully" << std::endl;
    return true;
}

// SRDWindow decoration controls implementation
bool LuaManager::set_window_decorations(const std::string& window_id, bool enabled) {
    std::cout << "LuaManager: Set window decorations for " << window_id << " to " << (enabled ? "enabled" : "disabled") << std::endl;
    
    if (!platform_) {
        std::cerr << "Platform not available for decoration control" << std::endl;
        return false;
    }
    
    // Find window by ID (placeholder implementation)
    // In a real implementation, you'd look up the window in the window manager
    SRDWindow* window = nullptr; // TODO: Get window from window manager
    
    if (window) {
        platform_->set_window_decorations(window, enabled);
        return true;
    }
    
    return false;
}

bool LuaManager::set_window_border_color(const std::string& window_id, int r, int g, int b) {
    std::cout << "LuaManager: Set border color for " << window_id << " to RGB(" << r << "," << g << "," << b << ")" << std::endl;
    
    if (!platform_) {
        std::cerr << "Platform not available for border color control" << std::endl;
        return false;
    }
    
    // Find window by ID (placeholder implementation)
    SRDWindow* window = nullptr; // TODO: Get window from window manager
    
    if (window) {
        platform_->set_window_border_color(window, r, g, b);
        return true;
    }
    
    return false;
}

bool LuaManager::set_window_border_width(const std::string& window_id, int width) {
    std::cout << "LuaManager: Set border width for " << window_id << " to " << width << std::endl;
    
    if (!platform_) {
        std::cerr << "Platform not available for border width control" << std::endl;
        return false;
    }
    
    // Find window by ID (placeholder implementation)
    SRDWindow* window = nullptr; // TODO: Get window from window manager
    
    if (window) {
        platform_->set_window_border_width(window, width);
        return true;
    }
    
    return false;
}

bool LuaManager::get_window_decorations(const std::string& window_id) const {
    if (!platform_) {
        return false;
    }
    
    // Find window by ID (placeholder implementation)
    SRDWindow* window = nullptr; // TODO: Get window from window manager
    
    if (window) {
        return platform_->get_window_decorations(window);
    }
    
    return false;
}

// SRDWindow state controls implementation
bool LuaManager::set_window_floating(const std::string& window_id, bool floating) {
    std::cout << "LuaManager: Set window " << window_id << " floating to " << (floating ? "true" : "false") << std::endl;
    
    if (!window_manager_) {
        std::cerr << "SRDWindow manager not available for floating control" << std::endl;
        return false;
    }
    
    // Find window by ID (placeholder implementation)
    SRDWindow* window = nullptr; // TODO: Get window from window manager
    
    if (window) {
        // TODO: Implement window floating state change
        // This would involve changing the window's layout state
        return true;
    }
    
    return false;
}

bool LuaManager::toggle_window_floating(const std::string& window_id) {
    std::cout << "LuaManager: Toggle window " << window_id << " floating state" << std::endl;
    
    bool current_state = is_window_floating(window_id);
    return set_window_floating(window_id, !current_state);
}

bool LuaManager::is_window_floating(const std::string& window_id) const {
    if (!window_manager_) {
        return false;
    }
    
    // Find window by ID (placeholder implementation)
    SRDWindow* window = nullptr; // TODO: Get window from window manager
    
    if (window) {
        // TODO: Check window's current layout state
        return false; // Placeholder
    }
    
    return false;
}

void LuaManager::load_default_config() {
    config_values_ = Defaults::create_default_config();
}

std::string LuaManager::get_string(const std::string& key, const std::string& default_value) const {
    auto it = config_values_.find(key);
    if (it != config_values_.end() && it->second.type == LuaConfigValue::Type::String) {
        return it->second.string_value;
    }
    return default_value;
}

int LuaManager::get_int(const std::string& key, int default_value) const {
    auto it = config_values_.find(key);
    if (it != config_values_.end() && it->second.type == LuaConfigValue::Type::Number) {
        return static_cast<int>(it->second.number_value);
    }
    return default_value;
}

bool LuaManager::get_bool(const std::string& key, bool default_value) const {
    auto it = config_values_.find(key);
    if (it != config_values_.end() && it->second.type == LuaConfigValue::Type::Boolean) {
        return it->second.bool_value;
    }
    return default_value;
}

double LuaManager::get_float(const std::string& key, double default_value) const {
    auto it = config_values_.find(key);
    if (it != config_values_.end() && it->second.type == LuaConfigValue::Type::Number) {
        return it->second.number_value;
    }
    return default_value;
}

void LuaManager::set_string(const std::string& key, const std::string& value) {
    LuaConfigValue config_value;
    config_value.type = LuaConfigValue::Type::String;
    config_value.string_value = value;
    config_values_[key] = config_value;
}

void LuaManager::set_int(const std::string& key, int value) {
    LuaConfigValue config_value;
    config_value.type = LuaConfigValue::Type::Number;
    config_value.number_value = static_cast<double>(value);
    config_values_[key] = config_value;
}

void LuaManager::set_bool(const std::string& key, bool value) {
    LuaConfigValue config_value;
    config_value.type = LuaConfigValue::Type::Boolean;
    config_value.bool_value = value;
    config_values_[key] = config_value;
}

void LuaManager::set_float(const std::string& key, double value) {
    LuaConfigValue config_value;
    config_value.type = LuaConfigValue::Type::Number;
    config_value.number_value = value;
    config_values_[key] = config_value;
}

bool LuaManager::bind_key(const std::string& key_combination, const std::string& lua_function) {
    key_bindings_[key_combination] = lua_function;
    std::cout << "Bound key: " << key_combination << " -> " << lua_function << std::endl;
    return true;
}

bool LuaManager::unbind_key(const std::string& key_combination) {
    auto it = key_bindings_.find(key_combination);
    if (it != key_bindings_.end()) {
        key_bindings_.erase(it);
        std::cout << "Unbound key: " << key_combination << std::endl;
        return true;
    }
    return false;
}

std::vector<std::string> LuaManager::get_bound_keys() const {
    std::vector<std::string> keys;
    for (const auto& binding : key_bindings_) {
        keys.push_back(binding.first);
    }
    return keys;
}

bool LuaManager::execute_lua_code(const std::string& code) {
    if (!L_) {
        return false;
    }
    
    if (luaL_dostring(L_, code.c_str()) != LUA_OK) {
        std::string error = lua_tostring(L_, -1);
        add_lua_error("Failed to execute Lua code: " + error);
        lua_pop(L_, 1);
        return false;
    }
    
    return true;
}

bool LuaManager::validate_lua_syntax(const std::string& code) {
    if (!L_) {
        return false;
    }
    
    // Try to load the code as a function (syntax check)
    if (luaL_loadstring(L_, code.c_str()) != LUA_OK) {
        std::string error = lua_tostring(L_, -1);
        add_lua_error("Lua syntax error: " + error);
        lua_pop(L_, 1);
        return false;
    }
    
    // Pop the loaded function
    lua_pop(L_, 1);
    return true;
}

std::vector<std::string> LuaManager::get_lua_errors() const {
    return lua_errors_;
}

bool LuaManager::validate_config() const {
    std::cout << "LuaManager: Validating configuration..." << std::endl;
    
    // Check for required configuration values
    std::vector<std::string> required_keys = {
        "general.default_layout",
        "general.window_gap",
        "general.border_width"
    };
    
    for (const auto& key : required_keys) {
        auto it = config_values_.find(key);
        if (it == config_values_.end()) {
            add_validation_error("Missing required configuration: " + key);
            return false;
        }
    }
    
    // Validate layout configuration
    auto layout_it = config_values_.find("general.default_layout");
    if (layout_it != config_values_.end()) {
        std::string layout = layout_it->second.string_value;
        if (layout != "tiling" && layout != "dynamic" && layout != "floating") {
            add_validation_error("Invalid default layout: " + layout);
            return false;
        }
    }
    
    // Validate numeric values
    auto gap_it = config_values_.find("general.window_gap");
    if (gap_it != config_values_.end()) {
        double gap = gap_it->second.number_value;
        if (gap < 0 || gap > 100) {
            add_validation_error("SRDWindow gap must be between 0 and 100");
            return false;
        }
    }
    
    auto border_it = config_values_.find("general.border_width");
    if (border_it != config_values_.end()) {
        double border = border_it->second.number_value;
        if (border < 0 || border > 50) {
            add_validation_error("Border width must be between 0 and 50");
            return false;
        }
    }
    
    std::cout << "LuaManager: Configuration validation passed" << std::endl;
    return true;
}

std::vector<std::string> LuaManager::get_validation_errors() const {
    return validation_errors_;
}

void LuaManager::reset_config(const std::string& key) {
    auto default_config = Defaults::create_default_config();
    auto it = default_config.find(key);
    if (it != default_config.end()) {
        config_values_[key] = it->second;
    }
}

void LuaManager::reset_all_configs() {
    config_values_ = Defaults::create_default_config();
}

void LuaManager::reset_category(const std::string& category) {
    std::string prefix = category + ".";
    auto default_config = Defaults::create_default_config();
    for (const auto& default_item : default_config) {
        if (default_item.first.substr(0, prefix.length()) == prefix) {
            config_values_[default_item.first] = default_item.second;
        }
    }
}

void LuaManager::add_lua_error(const std::string& error) {
    lua_errors_.push_back(error);
}

void LuaManager::add_validation_error(const std::string& error) const {
    const_cast<LuaManager*>(this)->validation_errors_.push_back(error);
}

void LuaManager::clear_errors() {
    lua_errors_.clear();
    validation_errors_.clear();
}

std::string LuaManager::get_config_file_path() const {
    // Platform-specific config path detection
    const char* home = std::getenv("HOME");
    if (home) {
        std::string config_dir = std::string(home) + "/.config/srdwm";
        return config_dir + "/config.lua";
    }
    return "./config.lua";
}

std::string LuaManager::get_default_config_path() const {
    // Platform-specific default config path
    const char* home = std::getenv("HOME");
    if (home) {
        std::string config_dir = std::string(home) + "/.config/srdwm";
        return config_dir + "/default.lua";
    }
    return "./default.lua";
}

bool LuaManager::create_default_config() const {
    std::cout << "LuaManager: Creating default configuration..." << std::endl;
    
    // Create default configuration directory
    const char* home = std::getenv("HOME");
    if (home) {
        std::string config_dir = std::string(home) + "/.config/srdwm";
        std::string config_file = config_dir + "/config.lua";
        
        // Create directory if it doesn't exist
        std::filesystem::create_directories(config_dir);
        
        // Create default config file
        std::ofstream file(config_file);
        if (file.is_open()) {
            file << "-- SRDWM Default Configuration\n";
            file << "-- Generated automatically\n\n";
            file << "-- Basic settings\n";
            file << "srd.set('general.default_layout', 'dynamic')\n";
            file << "srd.set('general.window_gap', 8)\n";
            file << "srd.set('general.border_width', 2)\n";
            file << "srd.set('general.animations', true)\n\n";
            file << "-- Key bindings\n";
            file << "srd.bind('Mod4+Return', function()\n";
            file << "    srd.spawn('alacritty')\n";
            file << "end)\n\n";
            file << "srd.bind('Mod4+Q', function()\n";
            file << "    local focused = srd.window.focused()\n";
            file << "    if focused then\n";
            file << "        srd.window.close(focused)\n";
            file << "    end\n";
            file << "end)\n\n";
            file << "srd.bind('Mod4+Space', function()\n";
            file << "    srd.layout.set('tiling')\n";
            file << "end)\n";
            file.close();
            
            std::cout << "LuaManager: Default configuration created at " << config_file << std::endl;
            return true;
        }
    }
    
    std::cout << "LuaManager: Failed to create default configuration" << std::endl;
    return false;
}

// Layout system methods
std::vector<std::string> LuaManager::get_available_layouts() const {
    if (layout_engine_) {
        return layout_engine_->get_available_layouts();
    }
    return {"tiling", "dynamic", "floating"};
}

bool LuaManager::set_layout(int monitor_id, const std::string& layout_name) {
    if (layout_engine_) {
        return layout_engine_->set_layout(monitor_id, layout_name);
    }
    std::cout << "LuaManager: Setting layout '" << layout_name << "' for monitor " << monitor_id << std::endl;
    return true;
}

std::string LuaManager::get_layout_name(int monitor_id) const {
    if (layout_engine_) {
        return layout_engine_->get_layout_name(monitor_id);
    }
    return "dynamic";
}

void LuaManager::set_layout_engine(LayoutEngine* engine) {
    layout_engine_ = engine;
    std::cout << "LuaManager: Layout engine connected" << std::endl;
}

bool LuaManager::configure_layout(const std::string& layout_name, const std::map<std::string, std::string>& config) {
    if (layout_engine_) {
        return layout_engine_->configure_layout(layout_name, config);
    }
    std::cout << "LuaManager: Configured layout '" << layout_name << "' with " << config.size() << " parameters" << std::endl;
    return true;
}


