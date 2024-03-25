#ifndef SRDWM_PLATFORM_FACTORY_H
#define SRDWM_PLATFORM_FACTORY_H

#include "platform.h"
#include <memory>
#include <string>
#include <vector>

// Platform factory for creating platform-specific implementations
class PlatformFactory {
public:
    // Create platform with automatic detection
    static std::unique_ptr<Platform> create_platform();
    
    // Create specific platform by name
    static std::unique_ptr<Platform> create_platform(const std::string& platform_name);
    
    // Get list of available platforms
    static std::vector<std::string> get_available_platforms();
    
    // Get current platform name
    static std::string get_current_platform();
    
    // Check if platform is available
    static bool is_platform_available(const std::string& platform_name);
    
    // Print platform information
    static void print_platform_info();

private:
    // Linux platform detection
    static std::unique_ptr<Platform> detect_linux_platform();
};

#endif // SRDWM_PLATFORM_FACTORY_H


