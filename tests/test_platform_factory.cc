#include <gtest/gtest.h>
#include "../src/platform/platform_factory.h"
#include "../src/platform/platform.h"

class PlatformFactoryTest : public ::testing::Test {
protected:
    void SetUp() override {
        // Set up test environment
    }
};

TEST_F(PlatformFactoryTest, PlatformDetection) {
    auto platform = PlatformFactory::create_platform();
    
    // Should create a valid platform
    EXPECT_NE(platform, nullptr);
}

TEST_F(PlatformFactoryTest, PlatformCreation) {
    auto platform = PlatformFactory::create_platform();
    
    EXPECT_NE(platform, nullptr);
    EXPECT_TRUE(platform->initialize());
}

TEST_F(PlatformFactoryTest, PlatformName) {
    auto platform = PlatformFactory::create_platform();
    
    std::string name = platform->get_platform_name();
    EXPECT_FALSE(name.empty());
    
    // Should be one of the expected platform names
    EXPECT_TRUE(name == "X11" || name == "Wayland" || name == "Windows" || name == "macOS");
}

TEST_F(PlatformFactoryTest, PlatformCapabilities) {
    auto platform = PlatformFactory::create_platform();
    
    // Test platform-specific capabilities
    if (platform->is_x11()) {
        EXPECT_TRUE(platform->get_platform_name() == "X11");
        EXPECT_FALSE(platform->is_wayland());
        EXPECT_FALSE(platform->is_windows());
        EXPECT_FALSE(platform->is_macos());
    } else if (platform->is_wayland()) {
        EXPECT_TRUE(platform->get_platform_name() == "Wayland");
        EXPECT_FALSE(platform->is_x11());
        EXPECT_FALSE(platform->is_windows());
        EXPECT_FALSE(platform->is_macos());
    } else if (platform->is_windows()) {
        EXPECT_TRUE(platform->get_platform_name() == "Windows");
        EXPECT_FALSE(platform->is_x11());
        EXPECT_FALSE(platform->is_wayland());
        EXPECT_FALSE(platform->is_macos());
    } else if (platform->is_macos()) {
        EXPECT_TRUE(platform->get_platform_name() == "macOS");
        EXPECT_FALSE(platform->is_x11());
        EXPECT_FALSE(platform->is_wayland());
        EXPECT_FALSE(platform->is_windows());
    }
}

TEST_F(PlatformFactoryTest, MonitorDetection) {
    auto platform = PlatformFactory::create_platform();
    
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

TEST_F(PlatformFactoryTest, EventPolling) {
    auto platform = PlatformFactory::create_platform();
    
    std::vector<Event> events;
    bool result = platform->poll_events(events);
    
    // Should not crash, even if no events are available
    EXPECT_TRUE(result || events.empty());
}

TEST_F(PlatformFactoryTest, WindowCreation) {
    auto platform = PlatformFactory::create_platform();
    
    auto window = platform->create_window("Test Window", 100, 100, 400, 300);
    EXPECT_NE(window, nullptr);
    
    if (window) {
        EXPECT_EQ(window->getTitle(), "Test Window");
        EXPECT_EQ(window->getX(), 100);
        EXPECT_EQ(window->getY(), 100);
        EXPECT_EQ(window->getWidth(), 400);
        EXPECT_EQ(window->getHeight(), 300);
    }
}

TEST_F(PlatformFactoryTest, WindowManagement) {
    auto platform = PlatformFactory::create_platform();
    
    auto window = platform->create_window("Test Window", 100, 100, 400, 300);
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

TEST_F(PlatformFactoryTest, DecorationControls) {
    auto platform = PlatformFactory::create_platform();
    
    auto window = platform->create_window("Test Window", 100, 100, 400, 300);
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

TEST_F(PlatformFactoryTest, InputHandling) {
    auto platform = PlatformFactory::create_platform();
    
    // Test keyboard grabbing
    platform->grab_keyboard();
    platform->ungrab_keyboard();
    
    // Test pointer grabbing
    platform->grab_pointer();
    platform->ungrab_pointer();
}

TEST_F(PlatformFactoryTest, PlatformShutdown) {
    auto platform = PlatformFactory::create_platform();
    
    // Should not crash on shutdown
    platform->shutdown();
}

int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
