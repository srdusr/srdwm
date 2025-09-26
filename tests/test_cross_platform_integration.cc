#include <gtest/gtest.h>
#include "../src/platform/platform_factory.h"
#include "../src/layouts/smart_placement.h"
#include "../src/config/lua_manager.h"
#include "../src/core/window_manager.h"
#include <memory>

class CrossPlatformIntegrationTest : public ::testing::Test {
protected:
    void SetUp() override {
        // Initialize platform
        platform = PlatformFactory::create_platform();
        EXPECT_NE(platform, nullptr);
        
        // Initialize Lua manager
        lua_manager = std::make_unique<LuaManager>();
        EXPECT_TRUE(lua_manager->initialize());
        
        // Initialize window manager
        window_manager = std::make_unique<WindowManager>();
        EXPECT_TRUE(window_manager->initialize());
    }
    
    void TearDown() override {
        if (window_manager) {
            window_manager->shutdown();
        }
        if (lua_manager) {
            lua_manager->shutdown();
        }
        if (platform) {
            platform->shutdown();
        }
    }
    
    std::unique_ptr<Platform> platform;
    std::unique_ptr<LuaManager> lua_manager;
    std::unique_ptr<WindowManager> window_manager;
};

TEST_F(CrossPlatformIntegrationTest, PlatformDetection) {
    auto detected_platform = PlatformFactory::detect_platform();
    
    // Should detect one of the supported platforms
    EXPECT_TRUE(detected_platform == PlatformType::Linux_X11 || 
                detected_platform == PlatformType::Linux_Wayland ||
                detected_platform == PlatformType::Windows ||
                detected_platform == PlatformType::macOS);
    
    // Platform name should match detection
    std::string platform_name = platform->get_platform_name();
    EXPECT_FALSE(platform_name.empty());
}

TEST_F(CrossPlatformIntegrationTest, WindowLifecycle) {
    // Create window through platform
    auto window = platform->create_window("Integration Test Window", 100, 100, 400, 300);
    EXPECT_NE(window, nullptr);
    
    if (window) {
        // Test window properties
        EXPECT_EQ(window->getTitle(), "Integration Test Window");
        EXPECT_EQ(window->getX(), 100);
        EXPECT_EQ(window->getY(), 100);
        EXPECT_EQ(window->getWidth(), 400);
        EXPECT_EQ(window->getHeight(), 300);
        
        // Test window management
        platform->set_window_position(window.get(), 200, 200);
        platform->set_window_size(window.get(), 500, 400);
        platform->focus_window(window.get());
        
        // Test decoration controls
        platform->set_window_decorations(window.get(), true);
        EXPECT_TRUE(platform->get_window_decorations(window.get()));
        
        platform->set_window_border_color(window.get(), 255, 0, 0);
        platform->set_window_border_width(window.get(), 5);
        
        // Clean up
        platform->destroy_window(window.get());
    }
}

TEST_F(CrossPlatformIntegrationTest, SmartPlacementIntegration) {
    // Get monitor information
    auto monitors = platform->get_monitors();
    EXPECT_FALSE(monitors.empty());
    
    Monitor monitor = monitors[0];
    
    // Create test windows
    std::vector<std::unique_ptr<Window>> existing_windows;
    for (int i = 0; i < 3; ++i) {
        auto window = platform->create_window(
            "Test Window " + std::to_string(i), 
            100 + i * 50, 100 + i * 50, 
            400, 300
        );
        EXPECT_NE(window, nullptr);
        existing_windows.push_back(std::move(window));
    }
    
    // Test smart placement
    auto new_window = platform->create_window("Smart Placement Test", 0, 0, 400, 300);
    EXPECT_NE(new_window, nullptr);
    
    if (new_window) {
        // Test grid placement
        auto grid_result = SmartPlacement::place_in_grid(new_window.get(), monitor, existing_windows);
        EXPECT_TRUE(grid_result.success);
        
        // Test cascade placement
        auto cascade_result = SmartPlacement::cascade_place(new_window.get(), monitor, existing_windows);
        EXPECT_TRUE(cascade_result.success);
        
        // Test smart tile
        auto smart_result = SmartPlacement::smart_tile(new_window.get(), monitor, existing_windows);
        EXPECT_TRUE(smart_result.success);
        
        platform->destroy_window(new_window.get());
    }
    
    // Clean up existing windows
    for (auto& window : existing_windows) {
        platform->destroy_window(window.get());
    }
}

TEST_F(CrossPlatformIntegrationTest, LuaConfigurationIntegration) {
    // Test Lua configuration with platform integration
    std::string config = R"(
        -- Platform detection
        local platform = srd.get_platform()
        srd.set("detected_platform", platform)
        
        -- Platform-specific settings
        if platform == "x11" then
            srd.set("border_width", 3)
            srd.set("decorations_enabled", true)
        elseif platform == "wayland" then
            srd.set("border_width", 2)
            srd.set("decorations_enabled", true)
        elseif platform == "windows" then
            srd.set("border_width", 2)
            srd.set("decorations_enabled", true)
        elseif platform == "macos" then
            srd.set("border_width", 1)
            srd.set("decorations_enabled", false)
        end
        
        -- Window decoration controls
        srd.window.set_decorations("test_window", true)
        srd.window.set_border_color("test_window", 255, 0, 0)
        srd.window.set_border_width("test_window", 5)
        
        -- Window state controls
        srd.window.set_floating("test_window", true)
        srd.window.toggle_floating("test_window")
    )";
    
    EXPECT_TRUE(lua_manager->load_config_string(config));
    
    // Verify platform detection
    std::string detected_platform = lua_manager->get_string("detected_platform", "");
    EXPECT_FALSE(detected_platform.empty());
    EXPECT_TRUE(detected_platform == "x11" || 
                detected_platform == "wayland" || 
                detected_platform == "windows" || 
                detected_platform == "macos");
}

TEST_F(CrossPlatformIntegrationTest, EventSystemIntegration) {
    // Test event polling
    std::vector<Event> events;
    bool result = platform->poll_events(events);
    
    // Should not crash, even if no events are available
    EXPECT_TRUE(result || events.empty());
    
    // Test event processing
    if (!events.empty()) {
        for (const auto& event : events) {
            // Process events through window manager
            window_manager->process_event(event);
        }
    }
}

TEST_F(CrossPlatformIntegrationTest, MonitorIntegration) {
    // Test monitor detection
    auto monitors = platform->get_monitors();
    EXPECT_FALSE(monitors.empty());
    
    for (const auto& monitor : monitors) {
        EXPECT_GT(monitor.id, 0);
        EXPECT_FALSE(monitor.name.empty());
        EXPECT_GT(monitor.width, 0);
        EXPECT_GT(monitor.height, 0);
        EXPECT_GT(monitor.refresh_rate, 0);
        
        // Test smart placement with monitor
        auto window = platform->create_window("Monitor Test", 0, 0, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            std::vector<std::unique_ptr<Window>> empty_windows;
            auto result = SmartPlacement::place_in_grid(window.get(), monitor, empty_windows);
            EXPECT_TRUE(result.success);
            
            platform->destroy_window(window.get());
        }
    }
}

TEST_F(CrossPlatformIntegrationTest, InputHandlingIntegration) {
    // Test input grabbing
    platform->grab_keyboard();
    platform->grab_pointer();
    
    // Test input release
    platform->ungrab_keyboard();
    platform->ungrab_pointer();
}

TEST_F(CrossPlatformIntegrationTest, ConfigurationReloading) {
    // Test configuration reloading with platform integration
    std::string initial_config = R"(
        srd.set("test.value", "initial")
        srd.set("test.platform", srd.get_platform())
    )";
    
    EXPECT_TRUE(lua_manager->load_config_string(initial_config));
    EXPECT_EQ(lua_manager->get_string("test.value", ""), "initial");
    
    // Reload configuration
    EXPECT_TRUE(lua_manager->reload_config());
    
    // Should be reset to default
    EXPECT_EQ(lua_manager->get_string("test.value", "default"), "default");
}

TEST_F(CrossPlatformIntegrationTest, ErrorHandling) {
    // Test error handling with invalid inputs
    Window* invalid_window = nullptr;
    
    // These should not crash
    platform->set_window_decorations(invalid_window, true);
    platform->set_window_border_color(invalid_window, 255, 0, 0);
    platform->set_window_border_width(invalid_window, 5);
    platform->get_window_decorations(invalid_window);
    
    // Test invalid Lua configuration
    std::string invalid_config = "invalid lua code {";
    EXPECT_FALSE(lua_manager->load_config_string(invalid_config));
    
    // Should have errors
    auto errors = lua_manager->get_errors();
    EXPECT_FALSE(errors.empty());
}

TEST_F(CrossPlatformIntegrationTest, PerformanceTest) {
    // Test performance with multiple windows and operations
    std::vector<std::unique_ptr<Window>> windows;
    
    // Create multiple windows
    for (int i = 0; i < 10; ++i) {
        auto window = platform->create_window(
            "Performance Test " + std::to_string(i), 
            100 + i * 10, 100 + i * 10, 
            400, 300
        );
        EXPECT_NE(window, nullptr);
        windows.push_back(std::move(window));
    }
    
    // Perform operations on all windows
    for (auto& window : windows) {
        platform->set_window_position(window.get(), 200, 200);
        platform->set_window_size(window.get(), 500, 400);
        platform->set_window_decorations(window.get(), true);
        platform->set_window_border_color(window.get(), 255, 0, 0);
        platform->set_window_border_width(window.get(), 5);
    }
    
    // Clean up
    for (auto& window : windows) {
        platform->destroy_window(window.get());
    }
}

TEST_F(CrossPlatformIntegrationTest, ShutdownSequence) {
    // Test proper shutdown sequence
    EXPECT_TRUE(window_manager->shutdown());
    EXPECT_TRUE(lua_manager->shutdown());
    platform->shutdown();
}

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
