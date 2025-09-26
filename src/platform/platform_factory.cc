#include "platform_factory.h"
#include <iostream>
#include <cstring>
#include <cstdlib>
#include <algorithm>

// Platform-specific includes
#ifdef LINUX_PLATFORM
    #include "x11_platform.h"
    #ifdef WAYLAND_ENABLED
        #include "wayland_platform.h"
    #endif
#elif defined(WIN32_PLATFORM)
    #include "windows_platform.h"
#elif defined(MACOS_PLATFORM)
    #include "macos_platform.h"
#endif

std::unique_ptr<Platform> PlatformFactory::create_platform() {
    #ifdef _WIN32
        std::cout << "Creating SRDWindows platform..." << std::endl;
        return std::make_unique<SRDWindowsPlatform>();
        
    #elif defined(__APPLE__)
        std::cout << "Creating macOS platform..." << std::endl;
        return std::make_unique<MacOSPlatform>();
        
    #else
        // Linux: detect X11 vs Wayland
        return detect_linux_platform();
    #endif
}

std::unique_ptr<Platform> PlatformFactory::create_platform(const std::string& platform_name) {
    std::cout << "Creating platform: " << platform_name << std::endl;
    
    if (platform_name == "x11" || platform_name == "X11") {
        #ifdef LINUX_PLATFORM
            return std::make_unique<X11Platform>();
        #else
            std::cerr << "X11 platform not available on this system" << std::endl;
            return nullptr;
        #endif
        
    } else if (platform_name == "wayland" || platform_name == "Wayland") {
        #ifdef WAYLAND_ENABLED
            return std::make_unique<WaylandPlatform>();
        #else
            std::cerr << "Wayland platform not available on this system" << std::endl;
            return nullptr;
        #endif
        
    } else if (platform_name == "windows" || platform_name == "SRDWindows") {
        #ifdef WIN32_PLATFORM
            return std::make_unique<SRDWindowsPlatform>();
        #else
            std::cerr << "SRDWindows platform not available on this system" << std::endl;
            return nullptr;
        #endif
        
    } else if (platform_name == "macos" || platform_name == "macOS") {
        #ifdef MACOS_PLATFORM
            return std::make_unique<MacOSPlatform>();
        #else
            std::cerr << "macOS platform not available on this system" << std::endl;
            return nullptr;
        #endif
        
    } else {
        std::cerr << "Unknown platform: " << platform_name << std::endl;
        std::cerr << "Available platforms: x11, wayland, windows, macos" << std::endl;
        return nullptr;
    }
}

std::unique_ptr<Platform> PlatformFactory::detect_linux_platform() {
    std::cout << "Detecting Linux platform..." << std::endl;
    
    // Check environment variables for Wayland
    const char* wayland_display = std::getenv("WAYLAND_DISPLAY");
    const char* xdg_session_type = std::getenv("XDG_SESSION_TYPE");
    const char* display = std::getenv("DISPLAY");
    
    std::cout << "Environment variables:" << std::endl;
    std::cout << "  WAYLAND_DISPLAY: " << (wayland_display ? wayland_display : "not set") << std::endl;
    std::cout << "  XDG_SESSION_TYPE: " << (xdg_session_type ? xdg_session_type : "not set") << std::endl;
    std::cout << "  DISPLAY: " << (display ? display : "not set") << std::endl;
    
    // Try Wayland first if environment suggests it
    if (wayland_display || (xdg_session_type && strcmp(xdg_session_type, "wayland") == 0)) {
        std::cout << "Wayland environment detected, attempting Wayland initialization..." << std::endl;
        
        #ifdef WAYLAND_ENABLED
            auto wayland_platform = std::make_unique<WaylandPlatform>();
            if (wayland_platform->initialize()) {
                std::cout << "✓ Wayland platform initialized successfully" << std::endl;
                return wayland_platform;
            } else {
                std::cout << "✗ Wayland initialization failed, falling back to X11" << std::endl;
            }
        #else
            std::cout << "Wayland support not compiled in, falling back to X11" << std::endl;
        #endif
    }
    
    // Fall back to X11
    std::cout << "Attempting X11 initialization..." << std::endl;
    
    #ifdef LINUX_PLATFORM
        auto x11_platform = std::make_unique<X11Platform>();
        if (x11_platform->initialize()) {
            std::cout << "✓ X11 platform initialized successfully" << std::endl;
            return x11_platform;
        } else {
            std::cout << "✗ X11 initialization failed" << std::endl;
        }
    #else
        std::cout << "X11 support not compiled in" << std::endl;
    #endif
    
    std::cerr << "Failed to initialize any platform backend" << std::endl;
    return nullptr;
}

std::vector<std::string> PlatformFactory::get_available_platforms() {
    std::vector<std::string> platforms;
    
    #ifdef LINUX_PLATFORM
        platforms.push_back("x11");
        #ifdef WAYLAND_ENABLED
            platforms.push_back("wayland");
        #endif
    #endif
    
    #ifdef WIN32_PLATFORM
        platforms.push_back("windows");
    #endif
    
    #ifdef MACOS_PLATFORM
        platforms.push_back("macos");
    #endif
    
    return platforms;
}

std::string PlatformFactory::get_current_platform() {
    #ifdef _WIN32
        return "windows";
    #elif defined(__APPLE__)
        return "macos";
    #else
        // Check if we're running on Wayland
        const char* wayland_display = std::getenv("WAYLAND_DISPLAY");
        const char* xdg_session_type = std::getenv("XDG_SESSION_TYPE");
        
        if (wayland_display || (xdg_session_type && strcmp(xdg_session_type, "wayland") == 0)) {
            return "wayland";
        } else {
            return "x11";
        }
    #endif
}

bool PlatformFactory::is_platform_available(const std::string& platform_name) {
    auto available = get_available_platforms();
    return std::find(available.begin(), available.end(), platform_name) != available.end();
}

void PlatformFactory::print_platform_info() {
    std::cout << "\n=== Platform Information ===" << std::endl;
    std::cout << "Current platform: " << get_current_platform() << std::endl;
    
    auto available = get_available_platforms();
    std::cout << "Available platforms: ";
    for (size_t i = 0; i < available.size(); ++i) {
        if (i > 0) std::cout << ", ";
        std::cout << available[i];
    }
    std::cout << std::endl;
    
    std::cout << "Environment variables:" << std::endl;
    const char* wayland_display = std::getenv("WAYLAND_DISPLAY");
    const char* xdg_session_type = std::getenv("XDG_SESSION_TYPE");
    const char* display = std::getenv("DISPLAY");
    
    std::cout << "  WAYLAND_DISPLAY: " << (wayland_display ? wayland_display : "not set") << std::endl;
    std::cout << "  XDG_SESSION_TYPE: " << (xdg_session_type ? xdg_session_type : "not set") << std::endl;
    std::cout << "  DISPLAY: " << (display ? display : "not set") << std::endl;
    
    #ifdef LINUX_PLATFORM
        std::cout << "Linux platform support: Enabled" << std::endl;
        #ifdef WAYLAND_ENABLED
            std::cout << "Wayland support: Enabled" << std::endl;
        #else
            std::cout << "Wayland support: Disabled" << std::endl;
        #endif
    #else
        std::cout << "Linux platform support: Disabled" << std::endl;
    #endif
    
    #ifdef WIN32_PLATFORM
        std::cout << "SRDWindows platform support: Enabled" << std::endl;
    #else
        std::cout << "SRDWindows platform support: Disabled" << std::endl;
    #endif
    
    #ifdef MACOS_PLATFORM
        std::cout << "macOS platform support: Enabled" << std::endl;
    #else
        std::cout << "macOS platform support: Disabled" << std::endl;
    #endif
    
    std::cout << "=============================" << std::endl;
}
