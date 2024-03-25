#include <gtest/gtest.h>
#include "../src/platform/macos_platform.h"
#include <memory>

class MacOSPlatformTest : public ::testing::Test {
protected:
    void SetUp() override {
        platform = std::make_unique<MacOSPlatform>();
    }
    
    void TearDown() override {
        if (platform) {
            platform->shutdown();
        }
    }
    
    std::unique_ptr<MacOSPlatform> platform;
};

TEST_F(MacOSPlatformTest, Initialization) {
    bool initialized = platform->initialize();
    
    if (initialized) {
        EXPECT_TRUE(platform->get_platform_name() == "macOS");
        EXPECT_TRUE(platform->is_macos());
        EXPECT_FALSE(platform->is_x11());
        EXPECT_FALSE(platform->is_wayland());
        EXPECT_FALSE(platform->is_windows());
    }
}

TEST_F(MacOSPlatformTest, PlatformCapabilities) {
    EXPECT_TRUE(platform->get_platform_name() == "macOS");
    EXPECT_TRUE(platform->is_macos());
    EXPECT_FALSE(platform->is_x11());
    EXPECT_FALSE(platform->is_wayland());
    EXPECT_FALSE(platform->is_windows());
}

TEST_F(MacOSPlatformTest, MonitorDetection) {
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

TEST_F(MacOSPlatformTest, WindowCreation) {
    if (platform->initialize()) {
        auto window = platform->create_window("macOS Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            EXPECT_EQ(window->getTitle(), "macOS Test Window");
            EXPECT_EQ(window->getX(), 100);
            EXPECT_EQ(window->getY(), 100);
            EXPECT_EQ(window->getWidth(), 400);
            EXPECT_EQ(window->getHeight(), 300);
        }
    }
}

TEST_F(MacOSPlatformTest, AccessibilityAPIs) {
    if (platform->initialize()) {
        auto window = platform->create_window("macOS Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            // Test accessibility-based window management
            platform->set_window_position(window.get(), 200, 200);
            platform->set_window_size(window.get(), 500, 400);
            platform->focus_window(window.get());
            
            platform->destroy_window(window.get());
        }
    }
}

TEST_F(MacOSPlatformTest, EventPolling) {
    if (platform->initialize()) {
        std::vector<Event> events;
        bool result = platform->poll_events(events);
        
        // Should not crash, even if no events are available
        EXPECT_TRUE(result || events.empty());
    }
}

TEST_F(MacOSPlatformTest, OverlayWindows) {
    if (platform->initialize()) {
        auto window = platform->create_window("macOS Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            // Test overlay window creation for custom decorations
            // This is the macOS workaround for custom decorations
            platform->set_window_decorations(window.get(), true);
            platform->set_window_border_color(window.get(), 255, 0, 0);
            platform->set_window_border_width(window.get(), 5);
            
            platform->destroy_window(window.get());
        }
    }
}

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
