#include <iostream>
#include <memory>
#include <string>

// Include Lua manager
#include "config/lua_manager.h"

// Include layout engine
#include "layouts/layout_engine.h"

// Include window manager
#include "core/window_manager.h"

// Include platform factory
#include "platform/platform_factory.h"

int main(int argc, char* argv[]) {
    std::cout << "SRDWM starting up..." << std::endl;

    // Print platform information
    PlatformFactory::print_platform_info();

    // Initialize layout engine
    auto layout_engine = std::make_unique<LayoutEngine>();
    std::cout << "Layout engine created" << std::endl;
    
    // Add a default monitor
    Monitor default_monitor{0, 0, 0, 1920, 1080, "Default", 60};
    layout_engine->add_monitor(default_monitor);
    std::cout << "Default monitor added to layout engine" << std::endl;
    
    // Initialize Lua manager
    g_lua_manager = std::make_unique<LuaManager>();
    if (!g_lua_manager->initialize()) {
        std::cerr << "Failed to initialize Lua manager" << std::endl;
        return 1;
    }
    
    // Connect layout engine to Lua manager
    g_lua_manager->set_layout_engine(layout_engine.get());
    std::cout << "Layout engine connected to Lua manager" << std::endl;
    
    // Initialize window manager
    auto window_manager = std::make_unique<SRDWindowManager>();
    std::cout << "SRDWindow manager created" << std::endl;
    
    // Connect components
    window_manager->set_layout_engine(layout_engine.get());
    window_manager->set_lua_manager(g_lua_manager.get());
    std::cout << "Components connected to window manager" << std::endl;

    // Initialize default workspaces
    window_manager->add_workspace("Main");
    window_manager->add_workspace("Web");
    window_manager->add_workspace("Code");
    window_manager->add_workspace("Media");
    std::cout << "Default workspaces created" << std::endl;

    // Load configuration
    std::string config_path = "./config/srdwm.lua";
    if (!g_lua_manager->load_config_file(config_path)) {
        std::cout << "Failed to load configuration, using defaults" << std::endl;
        // Set default configuration
        g_lua_manager->set_string("general.default_layout", "tiling");
        g_lua_manager->set_bool("general.smart_placement", true);
        g_lua_manager->set_int("general.window_gap", 8);
        g_lua_manager->set_int("general.border_width", 2);
        g_lua_manager->set_bool("general.animations", true);
        g_lua_manager->set_int("general.animation_duration", 200);
    }

    // Display current configuration
    std::cout << "\nCurrent Configuration:" << std::endl;
    std::cout << "Default Layout: " << g_lua_manager->get_string("general.default_layout", "tiling") << std::endl;
    std::cout << "Smart Placement: " << (g_lua_manager->get_bool("general.smart_placement", true) ? "enabled" : "disabled") << std::endl;
    std::cout << "Window Gap: " << g_lua_manager->get_int("general.window_gap", 8) << " pixels" << std::endl;
    std::cout << "Border Width: " << g_lua_manager->get_int("general.border_width", 2) << " pixels" << std::endl;
    std::cout << "Animations: " << (g_lua_manager->get_bool("general.animations", true) ? "enabled" : "disabled") << std::endl;
    std::cout << "Animation Duration: " << g_lua_manager->get_int("general.animation_duration", 200) << " ms" << std::endl;

    // Platform initialization
    std::cout << "\nInitializing platform..." << std::endl;
    
    // Create platform with automatic detection
    auto platform = PlatformFactory::create_platform();
    if (!platform) {
        std::cerr << "Failed to create platform" << std::endl;
        return 1;
    }
    
    std::cout << "Platform created: " << platform->get_platform_name() << std::endl;
    
    // Initialize platform
    if (!platform->initialize()) {
        std::cerr << "Failed to initialize platform" << std::endl;
        return 1;
    }
    
    std::cout << "Platform initialized successfully" << std::endl;
    
    // Connect platform to window manager
    window_manager->set_platform(platform.get());
    
    // Set up key bindings
    std::cout << "\nSetting up key bindings..." << std::endl;
    
    // Workspace switching
    window_manager->bind_key("Mod4+1", [&]() { window_manager->switch_to_workspace(0); });
    window_manager->bind_key("Mod4+2", [&]() { window_manager->switch_to_workspace(1); });
    window_manager->bind_key("Mod4+3", [&]() { window_manager->switch_to_workspace(2); });
    window_manager->bind_key("Mod4+4", [&]() { window_manager->switch_to_workspace(3); });
    
    // Workspace management
    window_manager->bind_key("Mod4+Shift+1", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) window_manager->move_window_to_workspace(focused, 0);
    });
    window_manager->bind_key("Mod4+Shift+2", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) window_manager->move_window_to_workspace(focused, 1);
    });
    window_manager->bind_key("Mod4+Shift+3", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) window_manager->move_window_to_workspace(focused, 2);
    });
    window_manager->bind_key("Mod4+Shift+4", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) window_manager->move_window_to_workspace(focused, 3);
    });
    
    // Window focus cycling
    window_manager->bind_key("Mod4+Tab", [&]() { 
        window_manager->focus_next_window();
    });
    window_manager->bind_key("Mod4+Shift+Tab", [&]() { 
        window_manager->focus_previous_window();
    });
    
    // Layout switching
    window_manager->bind_key("Mod4+t", [&]() { 
        layout_engine->set_layout(0, "tiling");
        window_manager->arrange_windows();
    });
    window_manager->bind_key("Mod4+d", [&]() { 
        layout_engine->set_layout(0, "dynamic");
        window_manager->arrange_windows();
    });
    window_manager->bind_key("Mod4+s", [&]() { 
        layout_engine->set_layout(0, "smart_placement");
        window_manager->arrange_windows();
    });
    
    // Window management
    window_manager->bind_key("Mod4+q", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) window_manager->close_window(focused);
    });
    window_manager->bind_key("Mod4+m", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) window_manager->maximize_window(focused);
    });
    
    // Window floating and tiling
    window_manager->bind_key("Mod4+f", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            window_manager->toggle_window_floating(focused);
        }
    });
    
    // Window movement with arrow keys
    window_manager->bind_key("Mod4+Shift+Left", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_x = focused->getX() - 50;
            window_manager->move_window(focused, new_x, focused->getY());
        }
    });
    window_manager->bind_key("Mod4+Shift+Right", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_x = focused->getX() + 50;
            window_manager->move_window(focused, new_x, focused->getY());
        }
    });
    window_manager->bind_key("Mod4+Shift+Up", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_y = focused->getY() - 50;
            window_manager->move_window(focused, focused->getX(), new_y);
        }
    });
    window_manager->bind_key("Mod4+Shift+Down", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_y = focused->getY() + 50;
            window_manager->move_window(focused, focused->getX(), new_y);
        }
    });
    
    // Window resizing with arrow keys
    window_manager->bind_key("Mod4+Ctrl+Left", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_width = focused->getWidth() - 50;
            if (new_width >= 100) {
                window_manager->resize_window(focused, new_width, focused->getHeight());
            }
        }
    });
    window_manager->bind_key("Mod4+Ctrl+Right", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_width = focused->getWidth() + 50;
            window_manager->resize_window(focused, new_width, focused->getHeight());
        }
    });
    window_manager->bind_key("Mod4+Ctrl+Up", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_height = focused->getHeight() - 50;
            if (new_height >= 100) {
                window_manager->resize_window(focused, focused->getWidth(), new_height);
            }
        }
    });
    window_manager->bind_key("Mod4+Ctrl+Down", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) {
            int new_height = focused->getHeight() + 50;
            window_manager->resize_window(focused, focused->getWidth(), new_height);
        }
    });
    
    // Additional window operations
    window_manager->bind_key("Mod4+space", [&]() { 
        auto* focused = window_manager->get_focused_window();
        if (focused) window_manager->minimize_window(focused);
    });
    
    window_manager->bind_key("Mod4+Return", [&]() { 
        // TODO: Launch terminal
        std::cout << "Launch terminal" << std::endl;
    });
    
    window_manager->bind_key("Mod4+d", [&]() { 
        // TODO: Launch application launcher
        std::cout << "Launch application launcher" << std::endl;
    });
    
    // Quick layout presets
    window_manager->bind_key("Mod4+Shift+t", [&]() { 
        layout_engine->set_layout(0, "tiling");
        window_manager->arrange_windows();
    });
    window_manager->bind_key("Mod4+Shift+d", [&]() { 
        layout_engine->set_layout(0, "dynamic");
        window_manager->arrange_windows();
    });
    window_manager->bind_key("Mod4+Shift+s", [&]() { 
        layout_engine->set_layout(0, "smart_placement");
        window_manager->arrange_windows();
    });
    
    // Exit
    window_manager->bind_key("Mod4+Shift+q", [&]() { 
        std::cout << "Exit key combination pressed" << std::endl;
        // TODO: Implement proper cleanup and exit
    });
    
    std::cout << "Key bindings configured" << std::endl;
    
    // Set initial layout
    std::string default_layout = g_lua_manager->get_string("general.default_layout", "tiling");
    layout_engine->set_layout(0, default_layout);
    
    // Arrange initial windows
    window_manager->arrange_windows();
    
    std::cout << "\nSRDWM initialization complete!" << std::endl;
    std::cout << "\nAvailable Key Bindings:" << std::endl;
    std::cout << "  Mod4+1-4          - Switch to workspace 1-4" << std::endl;
    std::cout << "  Mod4+Shift+1-4    - Move focused window to workspace 1-4" << std::endl;
    std::cout << "  Mod4+t/d/s        - Switch to tiling/dynamic/smart placement layout" << std::endl;
    std::cout << "  Mod4+Shift+t/d/s  - Quick layout presets" << std::endl;
    std::cout << "  Mod4+Tab          - Focus next window" << std::endl;
    std::cout << "  Mod4+Shift+Tab    - Focus previous window" << std::endl;
    std::cout << "  Mod4+f            - Toggle window floating" << std::endl;
    std::cout << "  Mod4+q            - Close focused window" << std::endl;
    std::cout << "  Mod4+m            - Maximize focused window" << std::endl;
    std::cout << "  Mod4+space        - Minimize focused window" << std::endl;
    std::cout << "  Mod4+Shift+Arrows - Move focused window" << std::endl;
    std::cout << "  Mod4+Ctrl+Arrows  - Resize focused window" << std::endl;
    std::cout << "  Mod4+Return       - Launch terminal" << std::endl;
    std::cout << "  Mod4+d            - Launch application launcher" << std::endl;
    std::cout << "  Mod4+Shift+q      - Exit SRDWM" << std::endl;
    std::cout << "\nMouse Controls:" << std::endl;
    std::cout << "  Left click + drag on titlebar - Move window" << std::endl;
    std::cout << "  Left click + drag on edges    - Resize window" << std::endl;
    
    // Main event loop
    try {
        window_manager->run();
    } catch (const std::exception& e) {
        std::cerr << "Error in main loop: " << e.what() << std::endl;
    }

    std::cout << "SRDWM shutting down." << std::endl;

    // Clean up platform
    platform->shutdown();
    platform.reset();

    // Clean up Lua manager
    g_lua_manager->shutdown();
    g_lua_manager.reset();

    std::cout << "Cleanup completed." << std::endl;

    return 0;
}
