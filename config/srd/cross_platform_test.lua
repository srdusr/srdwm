-- Cross-Platform Test Configuration
-- This file demonstrates all the cross-platform functionality of SRDWM

print("Loading cross-platform test configuration...")

-- Platform detection and configuration
local platform = srd.get_platform()
print("Detected platform: " .. platform)

-- Platform-specific settings
if platform == "wayland" then
    print("Configuring for Wayland/XWayland...")
    srd.set("general.decorations_enabled", true)
    srd.set("general.border_width", 2)
    srd.set("general.border_color", "#2e3440")
    
    -- Wayland-specific keybindings
    srd.bind("Mod4+w", function()
        -- Toggle server-side decorations
        local window = srd.window.focused()
        if window then
            local current = srd.window.get_decorations(window.id)
            srd.window.set_decorations(window.id, not current)
            print("Wayland: Toggled server-side decorations")
        end
    end)
    
elseif platform == "x11" then
    print("Configuring for X11...")
    srd.set("general.decorations_enabled", true)
    srd.set("general.border_width", 3)
    srd.set("general.border_color", "#2e3440")
    
    -- X11-specific keybindings
    srd.bind("Mod4+x", function()
        -- Toggle frame windows
        local window = srd.window.focused()
        if window then
            local current = srd.window.get_decorations(window.id)
            srd.window.set_decorations(window.id, not current)
            print("X11: Toggled frame windows")
        end
    end)
    
elseif platform == "windows" then
    print("Configuring for Windows...")
    srd.set("general.decorations_enabled", true)
    srd.set("general.border_width", 2)
    srd.set("general.border_color", "#2e3440")
    
    -- Windows-specific keybindings
    srd.bind("Mod4+d", function()
        -- Toggle DWM border colors
        local window = srd.window.focused()
        if window then
            srd.window.set_border_color(window.id, 255, 0, 0) -- Red border
            print("Windows: Set DWM border color")
        end
    end)
    
elseif platform == "macos" then
    print("Configuring for macOS...")
    srd.set("general.decorations_enabled", false) -- Limited decoration support
    srd.set("general.border_width", 1)
    srd.set("general.border_color", "#2e3440")
    
    -- macOS-specific keybindings
    srd.bind("Mod4+m", function()
        -- Toggle overlay windows (macOS limitation)
        local window = srd.window.focused()
        if window then
            print("macOS: Overlay window toggle requested")
        end
    end)
end

-- Universal cross-platform keybindings
srd.bind("Mod4+1", function()
    srd.layout.set("tiling")
    print("Switched to tiling layout")
end)

srd.bind("Mod4+2", function()
    srd.layout.set("dynamic")
    print("Switched to dynamic layout (smart placement)")
end)

srd.bind("Mod4+3", function()
    srd.layout.set("floating")
    print("Switched to floating layout")
end)

-- Window state controls (cross-platform)
srd.bind("Mod4+f", function()
    local window = srd.window.focused()
    if window then
        srd.window.toggle_floating(window.id)
        local floating = srd.window.is_floating(window.id)
        print("Window floating state: " .. tostring(floating))
    end
end)

srd.bind("Mod4+t", function()
    local window = srd.window.focused()
    if window then
        srd.window.set_floating(window.id, false)
        print("Set window to tiling mode")
    end
end)

srd.bind("Mod4+l", function()
    local window = srd.window.focused()
    if window then
        srd.window.set_floating(window.id, true)
        print("Set window to floating mode")
    end
end)

-- Decoration controls (cross-platform)
srd.bind("Mod4+b", function()
    local window = srd.window.focused()
    if window then
        srd.window.set_border_color(window.id, 0, 255, 0) -- Green border
        print("Set green border")
    end
end)

srd.bind("Mod4+n", function()
    local window = srd.window.focused()
    if window then
        srd.window.set_border_color(window.id, 0, 0, 255) -- Blue border
        print("Set blue border")
    end
end)

srd.bind("Mod4+r", function()
    local window = srd.window.focused()
    if window then
        srd.window.set_border_color(window.id, 255, 0, 0) -- Red border
        print("Set red border")
    end
end)

srd.bind("Mod4+0", function()
    local window = srd.window.focused()
    if window then
        -- Reset to default colors
        srd.window.set_border_color(window.id, 46, 52, 64) -- Default color
        srd.window.set_border_width(window.id, 2)
        print("Reset to default decoration")
    end
end)

-- Smart placement test
srd.bind("Mod4+s", function()
    print("Smart placement test:")
    print("- Grid placement: Windows 11-style window arrangement")
    print("- Cascade placement: Overlapping window management")
    print("- Snap-to-edge: Edge snapping functionality")
    print("- Overlap detection: Prevents window overlap")
end)

-- Platform-specific feature tests
function test_platform_features()
    print("Testing platform-specific features...")
    
    if platform == "wayland" then
        print("Wayland features:")
        print("- zxdg-decoration protocol support")
        print("- XWayland integration")
        print("- Layer shell support")
        print("- Server-side decorations")
        
    elseif platform == "x11" then
        print("X11 features:")
        print("- Frame window reparenting")
        print("- Custom titlebar drawing")
        print("- Border customization")
        print("- Event handling")
        
    elseif platform == "windows" then
        print("Windows features:")
        print("- DWM border color API")
        print("- Native decoration toggling")
        print("- Global keyboard/mouse hooks")
        print("- Window class management")
        
    elseif platform == "macos" then
        print("macOS features:")
        print("- Accessibility APIs")
        print("- Event tap system")
        print("- Core Graphics integration")
        print("- Limited decoration support")
    end
end

-- Test function for decoration system
function test_decoration_system()
    print("Testing decoration system...")
    
    -- Test decoration toggling
    srd.window.set_decorations("test_window", true)
    local has_decorations = srd.window.get_decorations("test_window")
    print("Decoration test: " .. tostring(has_decorations))
    
    -- Test border color
    srd.window.set_border_color("test_window", 128, 128, 128)
    print("Border color test: RGB(128, 128, 128)")
    
    -- Test border width
    srd.window.set_border_width("test_window", 5)
    print("Border width test: 5px")
end

-- Test function for window state system
function test_window_state_system()
    print("Testing window state system...")
    
    -- Test floating state
    srd.window.set_floating("test_window", true)
    local is_floating = srd.window.is_floating("test_window")
    print("Floating state test: " .. tostring(is_floating))
    
    -- Test toggle
    srd.window.toggle_floating("test_window")
    is_floating = srd.window.is_floating("test_window")
    print("Toggle test: " .. tostring(is_floating))
end

-- Run tests
test_platform_features()
test_decoration_system()
test_window_state_system()

print("Cross-platform test configuration loaded successfully!")
print("Use Mod4+1/2/3 to switch layouts")
print("Use Mod4+f to toggle floating")
print("Use Mod4+b/n/r to change border colors")
print("Use Mod4+0 to reset decorations")
print("Use Mod4+s for smart placement info")
