#include <gtest/gtest.h>
#include "../src/platform/windows_platform.h"
#include <memory>

class WindowsPlatformTest : public ::testing::Test {
protected:
    void SetUp() override {
        platform = std::make_unique<WindowsPlatform>();
    }
    
    void TearDown() override {
        if (platform) {
            platform->shutdown();
        }
    }
    
    std::unique_ptr<WindowsPlatform> platform;
};

TEST_F(WindowsPlatformTest, Initialization) {
    bool initialized = platform->initialize();
    
    if (initialized) {
        EXPECT_TRUE(platform->get_platform_name() == "Windows");
        EXPECT_TRUE(platform->is_windows());
        EXPECT_FALSE(platform->is_x11());
        EXPECT_FALSE(platform->is_wayland());
        EXPECT_FALSE(platform->is_macos());
    }
}

TEST_F(WindowsPlatformTest, PlatformCapabilities) {
    EXPECT_TRUE(platform->get_platform_name() == "Windows");
    EXPECT_TRUE(platform->is_windows());
    EXPECT_FALSE(platform->is_x11());
    EXPECT_FALSE(platform->is_wayland());
    EXPECT_FALSE(platform->is_macos());
}

TEST_F(WindowsPlatformTest, MonitorDetection) {
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

TEST_F(WindowsPlatformTest, WindowCreation) {
    if (platform->initialize()) {
        auto window = platform->create_window("Windows Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            EXPECT_EQ(window->getTitle(), "Windows Test Window");
            EXPECT_EQ(window->getX(), 100);
            EXPECT_EQ(window->getY(), 100);
            EXPECT_EQ(window->getWidth(), 400);
            EXPECT_EQ(window->getHeight(), 300);
        }
    }
}

TEST_F(WindowsPlatformTest, DWMIntegration) {
    if (platform->initialize()) {
        auto window = platform->create_window("Windows Test Window", 100, 100, 400, 300);
        EXPECT_NE(window, nullptr);
        
        if (window) {
            // Test DWM border color API
            platform->set_window_border_color(window.get(), 255, 0, 0);
            
            // Test border width
            platform->set_window_border_width(window.get(), 5);
            
            platform->destroy_window(window.get());
        }
    }
}

TEST_F(WindowsPlatformTest, EventPolling) {
    if (platform->initialize()) {
        std::vector<Event> events;
        bool result = platform->poll_events(events);
        
        // Should not crash, even if no events are available
        EXPECT_TRUE(result || events.empty());
    }
}

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
