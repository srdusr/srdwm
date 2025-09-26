-- SRDWM Key Bindings Configuration
-- Define all keyboard shortcuts and their actions

local srd = require("srd")

-- Layout switching
srd.bind("Mod4+1", function() 
    srd.layout.set("tiling")
    srd.notify("Layout: Tiling", "info")
end)

srd.bind("Mod4+2", function() 
    srd.layout.set("dynamic")
    srd.notify("Layout: Dynamic", "info")
end)

srd.bind("Mod4+3", function() 
    srd.layout.set("floating")
    srd.notify("Layout: Floating", "info")
end)

-- Window management
srd.bind("Mod4+q", function() 
    local window = srd.window.focused()
    if window then
        window:close()
    end
end)

srd.bind("Mod4+m", function() 
    local window = srd.window.focused()
    if window then
        window:minimize()
    end
end)

srd.bind("Mod4+f", function() 
    local window = srd.window.focused()
    if window then
        window:maximize()
    end
end)

-- Window movement (vim-style)
srd.bind("Mod4+h", function() 
    srd.window.focus("left")
end)

srd.bind("Mod4+j", function() 
    srd.window.focus("down")
end)

srd.bind("Mod4+k", function() 
    srd.window.focus("up")
end)

srd.bind("Mod4+l", function() 
    srd.window.focus("right")
end)

-- Window resizing
srd.bind("Mod4+Shift+h", function() 
    local window = srd.window.focused()
    if window then
        local x, y, w, h = window:get_geometry()
        window:set_geometry(x - 10, y, w + 10, h)
    end
end)

srd.bind("Mod4+Shift+j", function() 
    local window = srd.window.focused()
    if window then
        local x, y, w, h = window:get_geometry()
        window:set_geometry(x, y, w, h + 10)
    end
end)

srd.bind("Mod4+Shift+k", function() 
    local window = srd.window.focused()
    if window then
        local x, y, w, h = window:get_geometry()
        window:set_geometry(x, y - 10, w, h + 10)
    end
end)

srd.bind("Mod4+Shift+l", function() 
    local window = srd.window.focused()
    if window then
        local x, y, w, h = window:get_geometry()
        window:set_geometry(x, y, w + 10, h)
    end
end)

-- Workspace management
srd.bind("Mod4+Tab", function() 
    srd.workspace.next()
end)

srd.bind("Mod4+Shift+Tab", function() 
    srd.workspace.prev()
end)

-- Workspace switching with number keys
for i = 1, 10 do
    srd.bind("Mod4+" .. i, function()
        srd.workspace.switch(i)
    end)
    
    srd.bind("Mod4+Shift+" .. i, function()
        local window = srd.window.focused()
        if window then
            srd.workspace.move_window(window, i)
        end
    end)
end

-- Monitor switching
srd.bind("Mod4+Left", function() 
    srd.monitor.focus("left")
end)

srd.bind("Mod4+Right", function() 
    srd.monitor.focus("right")
end)

-- Window state toggles
srd.bind("Mod4+Shift+space", function() 
    local window = srd.window.focused()
    if window then
        if window:is_floating() then
            window:set_tiling()
        else
            window:set_floating()
        end
    end
end)

srd.bind("Mod4+Shift+f", function() 
    local window = srd.window.focused()
    if window then
        if window:is_fullscreen() then
            window:unset_fullscreen()
        else
            window:set_fullscreen()
        end
    end
end)

-- Quick actions
srd.bind("Mod4+d", function() 
    srd.spawn("rofi -show drun")
end)

srd.bind("Mod4+Return", function() 
    srd.spawn("alacritty")
end)

srd.bind("Mod4+r", function() 
    srd.spawn("rofi -show run")
end)

srd.bind("Mod4+w", function() 
    srd.spawn("firefox")
end)

-- System actions
srd.bind("Mod4+Shift+q", function() 
    srd.quit()
end)

srd.bind("Mod4+Shift+r", function() 
    srd.reload_config()
end)

srd.bind("Mod4+Shift+l", function() 
    srd.spawn("i3lock")
end)

-- Custom functions
function toggle_gaps()
    local current_gap = srd.get("general.window_gap")
    if current_gap > 0 then
        srd.set("general.window_gap", 0)
        srd.notify("Gaps: Off", "info")
    else
        srd.set("general.window_gap", 8)
        srd.notify("Gaps: On", "info")
    end
end

srd.bind("Mod4+g", toggle_gaps)

-- Volume control
srd.bind("XF86AudioRaiseVolume", function()
    srd.spawn("pactl set-sink-volume @DEFAULT_SINK@ +5%")
end)

srd.bind("XF86AudioLowerVolume", function()
    srd.spawn("pactl set-sink-volume @DEFAULT_SINK@ -5%")
end)

srd.bind("XF86AudioMute", function()
    srd.spawn("pactl set-sink-mute @DEFAULT_SINK@ toggle")
end)

-- Brightness control
srd.bind("XF86MonBrightnessUp", function()
    srd.spawn("brightnessctl set +5%")
end)

srd.bind("XF86MonBrightnessDown", function()
    srd.spawn("brightnessctl set 5%-")
end)

-- Print key bindings loaded
print("Key bindings configuration loaded")

