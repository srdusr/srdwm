#include <gtest/gtest.h>
#include "../src/platform/x11_platform.h"
#include <memory>

class X11PlatformTest : public ::testing::Test {
protected:
    void SetUp() override {
        platform = std::make_unique<X11Platform>();
    }
    
    void TearDown() override {
        if (platform) {
            platform->shutdown();
        }
    }
    
    std::unique_ptr<X11Platform> platform;
};

TEST_F(X11PlatformTest, Initialization) {
    // Note: This test may fail if no X11 display is available
    // In a real environment, this should work
    bool initialized = platform->initialize();
    
    if (initialized) {
        EXPECT_TRUE(platform->get_platform_name() == "X11");
        EXPECT_TRUE(platform->is_x11());
        EXPECT_FALSE(platform->is_wayland());
        EXPECT_FALSE(platform->is_windows());
        EXPECT_FALSE(platform->is_macos());
    }
}

TEST_F(X11PlatformTest, PlatformCapabilities) {
    EXPECT_TRUE(platform->get_platform_name() == "X11");
    EXPECT_TRUE(platform->is_x11());
    EXPECT_FALSE(platform->is_wayland());
    EXPECT_FALSE(platform->is_windows());
    EXPECT_FALSE(platform->is_macos());
}

TEST_F(X11PlatformTest, MonitorDetection) {
    if (platform->initialize()) {
        auto monitors = platform->get_monitors();
        EXPECT_FALSE(monitors.empty());
        
        for (const auto& monitor : monitors) {
            EXPECT_GT(monitor.id, 0);
            EXPECT_FALSE(monitor.name.empty());
            EXPECT_GT(monitor.width, 0);
            EXPECT_GT(monitor.height, 0);
            EXPECT_GT(monitor.refresh_rate, 0);
        }
    }
}

TEST_F(X11PlatformTest, WindowCreation) {
    if (platform->initialize()) {
        auto window = platform->create_window("X11 Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            EXPECT_EQ(window->getTitle(), "X11 Test Window");
            EXPECT_EQ(window->getX(), 100);
            EXPECT_EQ(window->getY(), 100);
            EXPECT_EQ(window->getWidth(), 400);
            EXPECT_EQ(window->getHeight(), 300);
        }
    }
}

TEST_F(X11PlatformTest, WindowManagement) {
    if (platform->initialize()) {
        auto window = platform->create_window("X11 Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            // Test window positioning
            platform->set_window_position(window.get(), 200, 200);
            EXPECT_EQ(window->getX(), 200);
            EXPECT_EQ(window->getY(), 200);
            
            // Test window sizing
            platform->set_window_size(window.get(), 500, 400);
            EXPECT_EQ(window->getWidth(), 500);
            EXPECT_EQ(window->getHeight(), 400);
            
            // Test window focusing
            platform->focus_window(window.get());
            
            // Test window destruction
            platform->destroy_window(window.get());
        }
    }
}

TEST_F(X11PlatformTest, DecorationControls) {
    if (platform->initialize()) {
        auto window = platform->create_window("X11 Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            // Test decoration toggling
            platform->set_window_decorations(window.get(), true);
            EXPECT_TRUE(platform->get_window_decorations(window.get()));
            
            platform->set_window_decorations(window.get(), false);
            EXPECT_FALSE(platform->get_window_decorations(window.get()));
            
            // Test border color
            platform->set_window_border_color(window.get(), 255, 0, 0);
            
            // Test border width
            platform->set_window_border_width(window.get(), 5);
            
            platform->destroy_window(window.get());
        }
    }
}

TEST_F(X11PlatformTest, EventPolling) {
    if (platform->initialize()) {
        std::vector<Event> events;
        bool result = platform->poll_events(events);
        
        // Should not crash, even if no events are available
        EXPECT_TRUE(result || events.empty());
    }
}

TEST_F(X11PlatformTest, InputHandling) {
    if (platform->initialize()) {
        // Test keyboard grabbing
        platform->grab_keyboard();
        platform->ungrab_keyboard();
        
        // Test pointer grabbing
        platform->grab_pointer();
        platform->ungrab_pointer();
    }
}

TEST_F(X11PlatformTest, FrameWindowCreation) {
    if (platform->initialize()) {
        auto window = platform->create_window("X11 Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            // Test frame window creation
            platform->set_window_decorations(window.get(), true);
            
            // Test titlebar drawing
            // This is an internal method, but we can test that it doesn't crash
            // platform->draw_titlebar(window.get());
            
            platform->destroy_window(window.get());
        }
    }
}

TEST_F(X11PlatformTest, WindowStateOperations) {
    if (platform->initialize()) {
        auto window = platform->create_window("X11 Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            // Test window operations
            platform->minimize_window(window.get());
            platform->maximize_window(window.get());
            platform->close_window(window.get());
        }
    }
}

TEST_F(X11PlatformTest, MultipleWindows) {
    if (platform->initialize()) {
        std::vector<std::unique_ptr<Window>> windows;
        
        // Create multiple windows
        for (int i = 0; i < 3; ++i) {
            auto window = platform->create_window(
                "X11 Test Window " + std::to_string(i), 
                100 + i * 50, 100 + i * 50, 
                400, 300
            );
            EXPECT_NE(window, nullptr);
            windows.push_back(std::move(window));
        }
        
        // Test that all windows exist
        EXPECT_EQ(windows.size(), 3);
        
        // Clean up
        for (auto& window : windows) {
            platform->destroy_window(window.get());
        }
    }
}

TEST_F(X11PlatformTest, ErrorHandling) {
    // Test with invalid window
    Window* invalid_window = nullptr;
    
    // These should not crash
    platform->set_window_decorations(invalid_window, true);
    platform->set_window_border_color(invalid_window, 255, 0, 0);
    platform->set_window_border_width(invalid_window, 5);
    platform->get_window_decorations(invalid_window);
}

TEST_F(X11PlatformTest, Shutdown) {
    if (platform->initialize()) {
        // Should not crash on shutdown
        platform->shutdown();
    }
}

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
