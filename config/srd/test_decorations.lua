-- Test configuration for decoration and window state controls
-- This file demonstrates the cross-platform decoration and tiling/floating functionality

print("Loading decoration and window state test configuration...")

-- Global decoration settings
srd.set("general.decorations_enabled", true)
srd.set("general.border_width", 3)
srd.set("general.border_color", "#2e3440")

-- Keybindings for decoration and window state controls
srd.bind("Mod4+d", function()
    -- Toggle decorations for focused window
    local window = srd.window.focused()
    if window then
        local current = srd.window.get_decorations(window.id)
        srd.window.set_decorations(window.id, not current)
        print("Toggled decorations for window " .. window.id)
    end
end)

srd.bind("Mod4+b", function()
    -- Toggle border color for focused window
    local window = srd.window.focused()
    if window then
        srd.window.set_border_color(window.id, 255, 0, 0) -- Red border
        print("Set red border for window " .. window.id)
    end
end)

srd.bind("Mod4+n", function()
    -- Toggle border color back to normal
    local window = srd.window.focused()
    if window then
        srd.window.set_border_color(window.id, 46, 52, 64) -- Default color
        print("Reset border color for window " .. window.id)
    end
end)

srd.bind("Mod4+w", function()
    -- Toggle border width
    local window = srd.window.focused()
    if window then
        srd.window.set_border_width(window.id, 8) -- Thick border
        print("Set thick border for window " .. window.id)
    end
end)

srd.bind("Mod4+s", function()
    -- Reset border width
    local window = srd.window.focused()
    if window then
        srd.window.set_border_width(window.id, 3) -- Normal border
        print("Reset border width for window " .. window.id)
    end
end)

-- Window state controls
srd.bind("Mod4+f", function()
    -- Toggle floating state for focused window
    local window = srd.window.focused()
    if window then
        srd.window.toggle_floating(window.id)
        local floating = srd.window.is_floating(window.id)
        print("Window " .. window.id .. " floating: " .. tostring(floating))
    end
end)

srd.bind("Mod4+t", function()
    -- Set window to tiling mode
    local window = srd.window.focused()
    if window then
        srd.window.set_floating(window.id, false)
        print("Set window " .. window.id .. " to tiling mode")
    end
end)

srd.bind("Mod4+l", function()
    -- Set window to floating mode
    local window = srd.window.focused()
    if window then
        srd.window.set_floating(window.id, true)
        print("Set window " .. window.id .. " to floating mode")
    end
end)

-- Layout switching with decoration awareness
srd.bind("Mod4+1", function()
    srd.layout.set("tiling")
    print("Switched to tiling layout")
end)

srd.bind("Mod4+2", function()
    srd.layout.set("dynamic")
    print("Switched to dynamic layout")
end)

srd.bind("Mod4+3", function()
    srd.layout.set("floating")
    print("Switched to floating layout")
end)

-- Test function to apply decorations to all windows
function apply_test_decorations()
    print("Applying test decorations to all windows...")
    
    -- This would iterate through all windows in a real implementation
    -- For now, we'll just demonstrate the API
    
    -- Example: Apply red border to window with ID "1"
    srd.window.set_border_color("1", 255, 0, 0)
    srd.window.set_border_width("1", 5)
    
    -- Example: Apply blue border to window with ID "2"
    srd.window.set_border_color("2", 0, 0, 255)
    srd.window.set_border_width("2", 3)
    
    -- Example: Disable decorations for window with ID "3"
    srd.window.set_decorations("3", false)
    
    print("Test decorations applied")
end

-- Call the test function
apply_test_decorations()

print("Decoration and window state test configuration loaded successfully!")
