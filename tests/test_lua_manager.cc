#include <gtest/gtest.h>
#include "../src/config/lua_manager.h"
#include <memory>

class LuaManagerTest : public ::testing::Test {
protected:
    void SetUp() override {
        lua_manager = std::make_unique<LuaManager>();
        EXPECT_TRUE(lua_manager->initialize());
    }
    
    void TearDown() override {
        if (lua_manager) {
            lua_manager->shutdown();
        }
    }
    
    std::unique_ptr<LuaManager> lua_manager;
};

TEST_F(LuaManagerTest, Initialization) {
    EXPECT_TRUE(lua_manager->initialize());
    EXPECT_TRUE(lua_manager->is_initialized());
}

TEST_F(LuaManagerTest, ConfigurationValues) {
    // Test string values
    lua_manager->set_string("test.string", "test_value");
    EXPECT_EQ(lua_manager->get_string("test.string", ""), "test_value");
    EXPECT_EQ(lua_manager->get_string("test.nonexistent", "default"), "default");
    
    // Test integer values
    lua_manager->set_int("test.int", 42);
    EXPECT_EQ(lua_manager->get_int("test.int", 0), 42);
    EXPECT_EQ(lua_manager->get_int("test.nonexistent", 100), 100);
    
    // Test boolean values
    lua_manager->set_bool("test.bool", true);
    EXPECT_TRUE(lua_manager->get_bool("test.bool", false));
    EXPECT_FALSE(lua_manager->get_bool("test.nonexistent", false));
}

TEST_F(LuaManagerTest, KeyBindings) {
    // Test key binding
    bool callback_called = false;
    lua_manager->bind_key("Mod4+Return", [&callback_called]() {
        callback_called = true;
    });
    
    // Simulate key press
    lua_manager->handle_key_press("Mod4+Return");
    EXPECT_TRUE(callback_called);
}

TEST_F(LuaManagerTest, LayoutManagement) {
    // Test layout setting
    lua_manager->set_layout(0, "tiling");
    EXPECT_EQ(lua_manager->get_layout(0), "tiling");
    
    lua_manager->set_layout(1, "dynamic");
    EXPECT_EQ(lua_manager->get_layout(1), "dynamic");
}

TEST_F(LuaManagerTest, WindowDecorationControls) {
    // Test decoration controls
    EXPECT_TRUE(lua_manager->set_window_decorations("test_window", true));
    EXPECT_TRUE(lua_manager->get_window_decorations("test_window"));
    
    EXPECT_TRUE(lua_manager->set_window_decorations("test_window", false));
    EXPECT_FALSE(lua_manager->get_window_decorations("test_window"));
    
    // Test border color
    EXPECT_TRUE(lua_manager->set_window_border_color("test_window", 255, 0, 0));
    
    // Test border width
    EXPECT_TRUE(lua_manager->set_window_border_width("test_window", 5));
}

TEST_F(LuaManagerTest, WindowStateControls) {
    // Test floating state
    EXPECT_TRUE(lua_manager->set_window_floating("test_window", true));
    EXPECT_TRUE(lua_manager->is_window_floating("test_window"));
    
    EXPECT_TRUE(lua_manager->set_window_floating("test_window", false));
    EXPECT_FALSE(lua_manager->is_window_floating("test_window"));
    
    // Test toggle
    EXPECT_TRUE(lua_manager->toggle_window_floating("test_window"));
    EXPECT_TRUE(lua_manager->is_window_floating("test_window"));
}

TEST_F(LuaManagerTest, ConfigurationLoading) {
    // Test loading configuration from string
    std::string config = R"(
        srd.set("test.loaded", true)
        srd.set("test.value", 123)
        srd.set("test.string", "loaded_value")
    )";
    
    EXPECT_TRUE(lua_manager->load_config_string(config));
    EXPECT_TRUE(lua_manager->get_bool("test.loaded", false));
    EXPECT_EQ(lua_manager->get_int("test.value", 0), 123);
    EXPECT_EQ(lua_manager->get_string("test.string", ""), "loaded_value");
}

TEST_F(LuaManagerTest, ErrorHandling) {
    // Test invalid Lua code
    std::string invalid_config = "invalid lua code {";
    EXPECT_FALSE(lua_manager->load_config_string(invalid_config));
    
    // Test error retrieval
    auto errors = lua_manager->get_errors();
    EXPECT_FALSE(errors.empty());
}

TEST_F(LuaManagerTest, ThemeConfiguration) {
    // Test theme colors
    std::map<std::string, std::string> colors = {
        {"background", "#2e3440"},
        {"foreground", "#eceff4"},
        {"accent", "#88c0d0"}
    };
    
    EXPECT_TRUE(lua_manager->set_theme_colors(colors));
    
    // Test theme decorations
    std::map<std::string, LuaConfigValue> decorations = {
        {"border_width", {LuaConfigValue::Type::Number, "", 3.0, false, {}, ""}},
        {"border_color", {LuaConfigValue::Type::String, "#2e3440", 0.0, false, {}, ""}}
    };
    
    EXPECT_TRUE(lua_manager->set_theme_decorations(decorations));
}

TEST_F(LuaManagerTest, ConfigurationReloading) {
    // Set initial configuration
    lua_manager->set_string("test.reload", "initial");
    EXPECT_EQ(lua_manager->get_string("test.reload", ""), "initial");
    
    // Reload configuration
    EXPECT_TRUE(lua_manager->reload_config());
    
    // Should be reset to default
    EXPECT_EQ(lua_manager->get_string("test.reload", "default"), "default");
}

TEST_F(LuaManagerTest, LuaAPIFunctions) {
    // Test Lua API registration
    std::string api_test = R"(
        -- Test srd.set
        srd.set("api.test", "value")
        
        -- Test srd.get
        local value = srd.get("api.test")
        if value ~= "value" then
            error("srd.get failed")
        end
        
        -- Test srd.bind
        srd.bind("Mod4+Test", function()
            srd.set("api.callback", "called")
        end)
    )";
    
    EXPECT_TRUE(lua_manager->load_config_string(api_test));
    EXPECT_EQ(lua_manager->get_string("api.test", ""), "value");
}

TEST_F(LuaManagerTest, WindowAPIFunctions) {
    // Test window API functions
    std::string window_api_test = R"(
        -- Test window decoration controls
        srd.window.set_decorations("test_window", true)
        srd.window.set_border_color("test_window", 255, 0, 0)
        srd.window.set_border_width("test_window", 5)
        
        -- Test window state controls
        srd.window.set_floating("test_window", true)
        srd.window.toggle_floating("test_window")
    )";
    
    EXPECT_TRUE(lua_manager->load_config_string(window_api_test));
}

TEST_F(LuaManagerTest, LayoutAPIFunctions) {
    // Test layout API functions
    std::string layout_api_test = R"(
        -- Test layout setting
        srd.layout.set("tiling")
        
        -- Test layout configuration
        srd.layout.configure("tiling", {
            gap = "10",
            border_width = "2"
        })
    )";
    
    EXPECT_TRUE(lua_manager->load_config_string(layout_api_test));
}

TEST_F(LuaManagerTest, PerformanceTest) {
    // Test performance with many configuration values
    for (int i = 0; i < 1000; ++i) {
        std::string key = "perf.test." + std::to_string(i);
        lua_manager->set_int(key, i);
    }
    
    // Verify all values
    for (int i = 0; i < 1000; ++i) {
        std::string key = "perf.test." + std::to_string(i);
        EXPECT_EQ(lua_manager->get_int(key, -1), i);
    }
}

TEST_F(LuaManagerTest, MemoryManagement) {
    // Test memory management with large configurations
    std::string large_config;
    for (int i = 0; i < 100; ++i) {
        large_config += "srd.set(\"large.test." + std::to_string(i) + "\", " + std::to_string(i) + ")\n";
    }
    
    EXPECT_TRUE(lua_manager->load_config_string(large_config));
    
    // Reload to test cleanup
    EXPECT_TRUE(lua_manager->reload_config());
}

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
