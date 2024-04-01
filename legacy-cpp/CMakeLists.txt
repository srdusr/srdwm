cmake_minimum_required(VERSION 3.16)
project(SRDWM 
    VERSION 1.0.0 
    DESCRIPTION "SRD Window Manager - A cross-platform window manager"
    LANGUAGES CXX
)

# Set C++ standard
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Build type with proper defaults
if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type (default: Release)" FORCE)
    set_property(CACHE CMAKE_BUILD_TYPE PROPERTY STRINGS "Debug" "Release" "MinSizeRel" "RelWithDebInfo")
endif()

# Compiler warnings and optimization
option(ENABLE_WERROR "Treat warnings as errors" OFF)
if(MSVC)
    add_compile_options(/W4 /WX)
    add_definitions(-D_CRT_SECURE_NO_WARNINGS -D_SCL_SECURE_NO_WARNINGS)
else()
    add_compile_options(-Wall -Wextra -Wpedantic)
    if(ENABLE_WERROR)
        add_compile_options(-Werror)
    endif()
    if(CMAKE_CXX_COMPILER_ID MATCHES "Clang|AppleClang")
        add_compile_options(-Weverything -Wno-c++98-compat -Wno-c++98-compat-pedantic)
    endif()
    
    # Platform-specific compiler flags
    if(PLATFORM_LINUX)
        add_compile_options(-fPIC)
    endif()
    
    # Release optimizations
    if(CMAKE_BUILD_TYPE STREQUAL "Release")
        add_compile_options(-O3 -DNDEBUG)
    elseif(CMAKE_BUILD_TYPE STREQUAL "Debug")
        add_compile_options(-g -O0 -DDEBUG)
    endif()
endif()

# Find required packages
find_package(PkgConfig REQUIRED)
find_package(Lua 5.4 REQUIRED)

# Platform detection and configuration
if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
    set(PLATFORM_LINUX TRUE)
    add_definitions(-DPLATFORM_LINUX=1 -DLINUX_PLATFORM)
    
    # X11 dependencies
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(X11 REQUIRED x11 xrandr xinerama xfixes xcursor)
    # Optional Xft (font rendering)
    pkg_check_modules(XFT xft)
    if(XFT_FOUND)
        add_definitions(-DHAVE_XFT=1)
    else()
        add_definitions(-DHAVE_XFT=0)
    endif()
    pkg_check_modules(XCB REQUIRED xcb xcb-keysyms xcb-icccm xcb-ewmh xcb-randr)
    
    # Wayland dependencies (optional)
    option(ENABLE_WAYLAND "Enable Wayland support" ON)
    # Temporary: use a stub Wayland backend to ensure builds succeed across wlroots versions
    option(USE_WAYLAND_STUB "Use minimal stub Wayland backend (no wlroots runtime)" ON)
    if(ENABLE_WAYLAND)
        pkg_check_modules(WAYLAND REQUIRED wayland-client wayland-server wayland-protocols)
        pkg_check_modules(WLROOTS REQUIRED wlroots)
        add_definitions(-DHAS_WAYLAND=1 -DWAYLAND_ENABLED)
        if(USE_WAYLAND_STUB)
            add_definitions(-DUSE_WAYLAND_STUB=1)
        else()
            pkg_check_modules(PIXMAN REQUIRED pixman-1)
        endif()
    else()
        add_definitions(-DHAS_WAYLAND=0)
    endif()
    
    # Additional Linux libraries
    find_library(RT_LIBRARY rt)
    find_library(MATH_LIBRARY m)
    
elseif(CMAKE_SYSTEM_NAME STREQUAL "Windows")
    set(PLATFORM_WINDOWS TRUE)
    add_definitions(-DPLATFORM_WINDOWS=1 -DWIN32_PLATFORM -DWIN32_LEAN_AND_MEAN -DUNICODE -D_UNICODE)
    
    # Windows SDK version
    if(NOT CMAKE_VS_WINDOWS_TARGET_PLATFORM_VERSION)
        set(CMAKE_VS_WINDOWS_TARGET_PLATFORM_VERSION "10.0.19041.0")
    endif()
    
    # Windows libraries
    set(WINDOWS_LIBS user32 gdi32 dwmapi uxtheme shell32 ole32)
    add_definitions(-DWIN32_LEAN_AND_MEAN -DUNICODE -D_UNICODE)
    
elseif(CMAKE_SYSTEM_NAME STREQUAL "Darwin")
    set(PLATFORM_MACOS TRUE)
    add_definitions(-DPLATFORM_MACOS=1 -DMACOS_PLATFORM)
    
    # macOS frameworks
    find_library(COCOA_LIBRARY Cocoa)
    find_library(CORE_GRAPHICS_LIBRARY CoreGraphics)
    find_library(CARBON_LIBRARY Carbon)
    find_library(IOKIT_LIBRARY IOKit)
    find_library(QUARTZCORE_LIBRARY QuartzCore)
    
    set(MACOS_FRAMEWORKS 
        ${COCOA_LIBRARY}
        ${CORE_GRAPHICS_LIBRARY}
        ${CARBON_LIBRARY}
        ${IOKIT_LIBRARY}
        ${QUARTZCORE_LIBRARY}
    )
    
    # macOS-specific compiler flags
    add_compile_options(-x objective-c++ -fobjc-arc)
    
    # Enable dark mode support
    add_definitions(-DMAC_OS_X_VERSION_MIN_REQUIRED=101400)
    
    # Set macOS deployment target
    set(CMAKE_OSX_DEPLOYMENT_TARGET "10.15" CACHE STRING "Minimum macOS deployment version")
    
    # Enable ARC for Objective-C files
    set(CMAKE_XCODE_ATTRIBUTE_CLANG_ENABLE_OBJC_ARC "YES")
    
    # Set macOS bundle identifier
    set(MACOSX_BUNDLE_BUNDLE_NAME "SRDWM")
    set(MACOSX_BUNDLE_BUNDLE_VERSION "${SRDWM_VERSION}")
    set(MACOSX_BUNDLE_GUI_IDENTIFIER "com.srd.wm")
    set(MACOSX_BUNDLE_COPYRIGHT "Copyright © 2023 SRDWM. All rights reserved.")
    
    # Set macOS icon (if available)
    if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/assets/icon.icns")
        set(MACOSX_BUNDLE_ICON_FILE "icon.icns")
        set_source_files_properties("${CMAKE_CURRENT_SOURCE_DIR}/assets/icon.icns" 
            PROPERTIES MACOSX_PACKAGE_LOCATION "Resources")
    endif()
    
    # Set macOS-specific linker flags
    set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} -framework Cocoa -framework CoreGraphics -framework Carbon -framework IOKit -framework QuartzCore")
    
    # Set macOS bundle properties
    set_target_properties(${PROJECT_NAME} PROPERTIES
        MACOSX_BUNDLE TRUE
        MACOSX_BUNDLE_INFO_PLIST "${CMAKE_CURRENT_SOURCE_DIR}/cmake/macos/Info.plist.in"
    )
    
    if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/assets/icon.icns")
        set_target_properties(${PROJECT_NAME} PROPERTIES
            RESOURCE "${CMAKE_CURRENT_SOURCE_DIR}/assets/icon.icns"
        )
    endif()
    
    # Set macOS-specific install paths
    set(CMAKE_INSTALL_PREFIX "/Applications" CACHE PATH "Installation directory")
    
    # Add macOS-specific source files
    file(GLOB_RECURSE MACOS_SOURCES 
        "${CMAKE_CURRENT_SOURCE_DIR}/src/platform/macos/*.mm"
        "${CMAKE_CURRENT_SOURCE_DIR}/src/platform/macos/*.m"
    )
    
    # Add macOS-specific include directories
    include_directories(
        ${CMAKE_CURRENT_SOURCE_DIR}/src/platform/macos
    )
    
    # Add custom build phase to sign the app bundle
    if(CMAKE_BUILD_TYPE STREQUAL "Release")
        add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
            COMMAND codesign --force --sign - --timestamp=none $<TARGET_BUNDLE_DIR:${PROJECT_NAME}>
            COMMENT "Signing macOS application bundle..."
        )
    endif()
    
    # Add macOS-specific build phase to set the executable bit
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND chmod +x "$<TARGET_FILE:${PROJECT_NAME}>"
        COMMENT "Setting executable permissions..."
    )
    
    # Add macOS-specific build phase to set the bundle identifier
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.srd.wm" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle identifier..."
    )
    
    # Add macOS-specific build phase to set the bundle version
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${SRDWM_VERSION}" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle version..."
    )
    
    # Add macOS-specific build phase to set the bundle display name
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName SRDWM" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle display name..."
    )
    
    # Add macOS-specific build phase to set the bundle icon file
    if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/assets/icon.icns")
        add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
            COMMAND ${CMAKE_COMMAND} -E copy
                "${CMAKE_CURRENT_SOURCE_DIR}/assets/icon.icns"
                "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Resources/icon.icns"
            COMMENT "Copying icon file..."
        )
    endif()
    
    # Add macOS-specific build phase to set the bundle executable
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :CFBundleExecutable string ${PROJECT_NAME}" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle executable..."
    )
    
    # Add macOS-specific build phase to set the bundle package type
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :CFBundlePackageType string APPL" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle package type..."
    )
    
    # Add macOS-specific build phase to set the bundle signature
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :CFBundleSignature string ????" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle signature..."
    )
    
    # Add macOS-specific build phase to set the bundle info dictionary version
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :CFBundleInfoDictionaryVersion string 6.0" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle info dictionary version..."
    )
    
    # Add macOS-specific build phase to set the bundle development region
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :CFBundleDevelopmentRegion string en" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle development region..."
    )
    
    # Add macOS-specific build phase to set the bundle name
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :CFBundleName string SRDWM" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle name..."
    )
    
    # Add macOS-specific build phase to set the bundle version (short version string)
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :CFBundleShortVersionString string ${SRDWM_VERSION}" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle short version string..."
    )
    
    # Add macOS-specific build phase to set the bundle copyright
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :NSHumanReadableCopyright string Copyright © 2023 SRDWM. All rights reserved." "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle copyright..."
    )
    
    # Add macOS-specific build phase to set the bundle high-resolution capable
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :NSHighResolutionCapable bool true" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle high-resolution capable..."
    )
    
    # Add macOS-specific build phase to set the bundle application is agent (UIElement)
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :LSUIElement bool true" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle as agent application..."
    )
    
    # Add macOS-specific build phase to set the bundle background only
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :LSBackgroundOnly bool true" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle as background only..."
    )
    
    # Add macOS-specific build phase to set the bundle requires carbon
    add_custom_command(TARGET ${PROJECT_NAME} POST_BUILD
        COMMAND /usr/libexec/PlistBuddy -c "Add :LSRequiresCarbon bool true" "$<TARGET_BUNDLE_CONTENT_DIR:${PROJECT_NAME}>/Info.plist"
        COMMENT "Setting bundle requires Carbon..."
    )
endif()

# Include directories
include_directories(
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/include
    ${LUA_INCLUDE_DIR}
)

if(PLATFORM_LINUX)
    include_directories(${X11_INCLUDE_DIRS} ${XCB_INCLUDE_DIRS})
    if(XFT_FOUND)
        include_directories(${XFT_INCLUDE_DIRS})
    endif()
    if(ENABLE_WAYLAND)
        include_directories(${WAYLAND_INCLUDE_DIRS} ${WLROOTS_INCLUDE_DIRS})
        if(PIXMAN_FOUND)
            include_directories(${PIXMAN_INCLUDE_DIRS})
        endif()
        # Generate Wayland protocol headers/sources (xdg-shell)
        find_program(WAYLAND_SCANNER wayland-scanner)
        if(WAYLAND_SCANNER)
            # Locate wayland-protocols XML directory
            pkg_get_variable(WAYLAND_PROTOCOLS_DIR wayland-protocols pkgdatadir)
            if(NOT WAYLAND_PROTOCOLS_DIR)
                set(WAYLAND_PROTOCOLS_DIR "/usr/share/wayland-protocols")
            endif()
            set(XDG_SHELL_XML "${WAYLAND_PROTOCOLS_DIR}/stable/xdg-shell/xdg-shell.xml")
            set(GENERATED_WL_DIR "${CMAKE_CURRENT_BINARY_DIR}/protocols")
            file(MAKE_DIRECTORY "${GENERATED_WL_DIR}")

            set(XDG_SHELL_HEADER "${GENERATED_WL_DIR}/xdg-shell-protocol.h")
            set(XDG_SHELL_CODE   "${GENERATED_WL_DIR}/xdg-shell-protocol.c")

            add_custom_command(
                OUTPUT ${XDG_SHELL_HEADER}
                COMMAND ${WAYLAND_SCANNER} client-header ${XDG_SHELL_XML} ${XDG_SHELL_HEADER}
                DEPENDS ${XDG_SHELL_XML}
                COMMENT "Generating Wayland header: xdg-shell-protocol.h"
            )

            add_custom_command(
                OUTPUT ${XDG_SHELL_CODE}
                COMMAND ${WAYLAND_SCANNER} private-code ${XDG_SHELL_XML} ${XDG_SHELL_CODE}
                DEPENDS ${XDG_SHELL_XML}
                COMMENT "Generating Wayland source: xdg-shell-protocol.c"
            )

            add_custom_target(wayland_protocols_gen DEPENDS ${XDG_SHELL_HEADER} ${XDG_SHELL_CODE})
            include_directories(${GENERATED_WL_DIR})
            if(NOT USE_WAYLAND_STUB)
                list(APPEND SOURCES ${XDG_SHELL_CODE})
            endif()
        endif()
    endif()
endif()

# Source files
set(SOURCES
    src/main.cc
    src/core/window.cc
    src/core/window_manager.cc
    src/core/event_system.cc
    src/platform/platform_factory.cc
    src/layouts/layout_engine.cc
    src/layouts/tiling_layout.cc
    src/layouts/dynamic_layout.cc
    src/layouts/smart_placement.cc
    src/config/lua_manager.cc
    src/utils/logger.cc
)

# Platform-specific source files
if(PLATFORM_LINUX)
    list(APPEND SOURCES
        src/platform/x11_platform.cc
    )
    if(ENABLE_WAYLAND)
        if(USE_WAYLAND_STUB)
            list(APPEND SOURCES src/platform/wayland_platform_stub.cc)
        else()
            list(APPEND SOURCES src/platform/wayland_platform.cc)
        endif()
    endif()
elseif(PLATFORM_WINDOWS)
    list(APPEND SOURCES
        src/platform/windows_platform.cc
    )
elseif(PLATFORM_MACOS)
    list(APPEND SOURCES
        src/platform/macos_platform.cc
        ${MACOS_SOURCES}
    )
endif()

# Create executable
add_executable(${PROJECT_NAME} ${SOURCES})

if(PLATFORM_LINUX AND ENABLE_WAYLAND AND TARGET wayland_protocols_gen)
    add_dependencies(${PROJECT_NAME} wayland_protocols_gen)
endif()

# Set target properties
set_target_properties(${PROJECT_NAME} PROPERTIES
    CXX_STANDARD 17
    CXX_STANDARD_REQUIRED ON
    CXX_EXTENSIONS OFF
    OUTPUT_NAME srdwm
)

# Link libraries
target_link_libraries(${PROJECT_NAME} PRIVATE ${LUA_LIBRARIES})

if(PLATFORM_LINUX)
    target_link_libraries(${PROJECT_NAME} PRIVATE
        ${X11_LIBRARIES}
        ${XCB_LIBRARIES}
        ${RT_LIBRARY}
        ${MATH_LIBRARY}
        dl
        pthread
    )
    if(XFT_FOUND)
        target_link_libraries(${PROJECT_NAME} PRIVATE ${XFT_LIBRARIES})
    endif()
    
    if(ENABLE_WAYLAND)
        if(NOT USE_WAYLAND_STUB)
            target_link_libraries(${PROJECT_NAME} PRIVATE
                ${WAYLAND_LIBRARIES}
                ${WLROOTS_LIBRARIES}
            )
            target_compile_definitions(${PROJECT_NAME} PRIVATE ${WAYLAND_CFLAGS_OTHER} WLR_USE_UNSTABLE)
        else()
            target_compile_definitions(${PROJECT_NAME} PRIVATE WLR_USE_UNSTABLE)
        endif()
    endif()
    
    target_compile_definitions(${PROJECT_NAME} PRIVATE ${X11_CFLAGS_OTHER})
    
elseif(PLATFORM_WINDOWS)
    target_link_libraries(${PROJECT_NAME} PRIVATE
        ${WINDOWS_LIBS}
    )
    
elseif(PLATFORM_MACOS)
    target_link_libraries(${PROJECT_NAME} PRIVATE
        ${MACOS_FRAMEWORKS}
    )
    
    # Set macOS-specific install paths
    install(TARGETS ${PROJECT_NAME}
        BUNDLE DESTINATION .
        RUNTIME DESTINATION bin
    )
    
    # Add custom install command for macOS
    install(CODE "
        include(BundleUtilities)
        fixup_bundle(
            \"${CMAKE_INSTALL_PREFIX}/${PROJECT_NAME}.app\"
            \"\"
            \"${CMAKE_INSTALL_PREFIX}/lib${LIB_SUFFIX}\"
        )
    ")
    
    # Add custom uninstall target for macOS
    if(NOT TARGET uninstall)
        configure_file(
            "${CMAKE_CURRENT_SOURCE_DIR}/cmake/macos/Uninstall.cmake.in"
            "${CMAKE_CURRENT_BINARY_DIR}/Uninstall.cmake"
            @ONLY
        )
        
        add_custom_target(uninstall
            COMMAND ${CMAKE_COMMAND} -P "${CMAKE_CURRENT_BINARY_DIR}/Uninstall.cmake"
        )
    endif()
endif()

# Installation
install(TARGETS ${PROJECT_NAME}
    RUNTIME DESTINATION bin
    LIBRARY DESTINATION lib
    ARCHIVE DESTINATION lib
)

# Install desktop entries (Linux)
if(PLATFORM_LINUX)
    # Options to control which session files are installed
    option(SRDWM_INSTALL_X11_SESSION "Install X11 desktop session file" ON)
    option(SRDWM_INSTALL_WAYLAND_SESSION "Install Wayland desktop session file" ON)

    if(SRDWM_INSTALL_X11_SESSION)
        install(FILES
            ${CMAKE_CURRENT_SOURCE_DIR}/cmake/data/srdwm.desktop
            DESTINATION share/xsessions
        )
    endif()

    if(SRDWM_INSTALL_WAYLAND_SESSION)
        install(FILES
            ${CMAKE_CURRENT_SOURCE_DIR}/cmake/data/srdwm-wayland.desktop
            DESTINATION share/wayland-sessions
        )
    endif()
    # Compatibility symlink: SRDWM -> srdwm
    install(CODE "
        file(MAKE_DIRECTORY \"${CMAKE_INSTALL_PREFIX}/bin\")
        execute_process(COMMAND \"${CMAKE_COMMAND}\" -E create_symlink srdwm SRDWM
                        WORKING_DIRECTORY \"${CMAKE_INSTALL_PREFIX}/bin\")
    ")
endif()

# Install headers
install(DIRECTORY include/ DESTINATION include
    FILES_MATCHING PATTERN "*.h"
)

# Package configuration
include(CMakePackageConfigHelpers)
configure_package_config_file(
    ${CMAKE_CURRENT_SOURCE_DIR}/cmake/Config.cmake.in
    ${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}Config.cmake
    INSTALL_DESTINATION lib/cmake/${PROJECT_NAME}
)

write_basic_package_version_file(
    ${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}ConfigVersion.cmake
    VERSION ${PROJECT_VERSION}
    COMPATIBILITY SameMajorVersion
)

install(FILES
    ${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}Config.cmake
    ${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}ConfigVersion.cmake
    DESTINATION lib/cmake/${PROJECT_NAME}
)

# Enable testing if requested
option(BUILD_TESTING "Build the testing tree" ON)
if(BUILD_TESTING)
    enable_testing()
    add_subdirectory(tests)
endif()

# Add uninstall target
if(NOT TARGET uninstall)
    configure_file(
        "${CMAKE_CURRENT_SOURCE_DIR}/cmake/Uninstall.cmake.in"
        "${CMAKE_CURRENT_BINARY_DIR}/cmake_uninstall.cmake"
        @ONLY
    )
    
    add_custom_target(uninstall
        COMMAND ${CMAKE_COMMAND} -P ${CMAKE_CURRENT_BINARY_DIR}/cmake_uninstall.cmake
    )
endif()

# Add custom commands for building documentation
find_package(Doxygen)
if(DOXYGEN_FOUND)
    set(DOXYGEN_IN ${CMAKE_CURRENT_SOURCE_DIR}/docs/Doxyfile.in)
    set(DOXYGEN_OUT ${CMAKE_CURRENT_BINARY_DIR}/Doxyfile)
    
    configure_file(${DOXYGEN_IN} ${DOXYGEN_OUT} @ONLY)
    
    add_custom_target(docs
        COMMAND ${DOXYGEN_EXECUTABLE} ${DOXYGEN_OUT}
        WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}
        COMMENT "Generating API documentation with Doxygen"
        VERBATIM
    )
endif()

# Add custom command to generate compile_commands.json for use with clangd
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

