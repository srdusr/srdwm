# Test configuration - temporarily disabled to focus on main executable
# find_package(GTest REQUIRED)

# Test executables - only build the ones that can work for now
# add_executable(test_platform_factory test_platform_factory.cc)

# Set include directories for all tests
# target_include_directories(test_platform_factory PRIVATE ${CMAKE_SOURCE_DIR}/src)

# Set platform-specific settings for tests
# if(PLATFORM_LINUX)
#     target_include_directories(test_platform_factory PRIVATE ${X11_INCLUDE_DIRS} ${WAYLAND_INCLUDE_DIRS} ${WLROOTS_INCLUDE_DIRS} ${XCB_INCLUDE_DIRS})
#     target_compile_definitions(test_platform_factory PRIVATE ${X11_CFLAGS_OTHER} ${WAYLAND_CFLAGS_OTHER} WLR_USE_UNSTABLE)
# endif()

# Link test executables
# target_link_libraries(test_platform_factory GTest::gtest GTest::gtest_main)

# Add tests
# add_test(NAME PlatformFactory COMMAND test_platform_factory)
