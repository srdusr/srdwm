- SRDWM Key Bindings Configuration
- Every function called here is part of the real `srd` API implemented in
- crates/config/src/lib.rs - see docs/DEFAULTS.md for the full reference.

local srd = require("srd")

- Layout switching
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

- Window management (act on the focused window)
srd.bind("Mod4+q", function() srd.window.close() end, "Close the focused window")
srd.bind("Mod4+m", function() srd.window.minimize() end, "Minimize the focused window")
srd.bind("Mod4+f", function() srd.window.maximize() end, "Maximize or restore the focused window")
srd.bind("Mod4+Shift+space", function() srd.window.toggle_floating() end, "Float or unfloat the focused window")

- Window navigation (vim-style directional focus)
srd.bind("Mod4+h", function() srd.window.focus("left") end, "Focus the window to the left")
srd.bind("Mod4+j", function() srd.window.focus("down") end, "Focus the window to the down")
srd.bind("Mod4+k", function() srd.window.focus("up") end, "Focus the window to the up")
srd.bind("Mod4+l", function() srd.window.focus("right") end, "Focus the window to the right")

- Move the focused window in a direction, mirroring the focus keys above.
- `srd.window.move` has existed for a long time and simply had no default
- binding, so "move window to absolute directions" was unreachable without
- writing your own config.
srd.bind("Mod4+Shift+h", function() srd.window.move("left") end, "Move the window left")
srd.bind("Mod4+Shift+j", function() srd.window.move("down") end, "Move the window down")
srd.bind("Mod4+Shift+k", function() srd.window.move("up") end, "Move the window up")
srd.bind("Mod4+Shift+l", function() srd.window.move("right") end, "Move the window right")

- Layout switching. srdwm is dynamic-first: "dynamic" is free placement,
- the default, and tiling is one opt-in layout among several. Mod4+s
- toggles between the two, which is the pair worth a single key.
srd.bind("Mod4+s", function()
    if srd.layout.get() == "tiling" then
        srd.layout.set("dynamic")
    else
        srd.layout.set("tiling")
    end
end, "Toggle tiling and dynamic layout")

- Lock the session using srdwm's own built-in lock screen. `srd.lock()`
- talks to the compositor directly rather than shelling out to the control
- CLI, so it still works when that binary is not on PATH.
srd.bind("Mod4+Ctrl+l", function() srd.lock() end, "Lock the session")

- Workspace management
srd.bind("Mod4+Tab", function() srd.workspace.next() end, "Next workspace")
srd.bind("Mod4+Shift+Tab", function() srd.workspace.prev() end, "Previous workspace")

- Workspace switching with number keys (0 doubles as "workspace 10")
for i = 0, 9 do
    srd.bind("Mod4+" .. i, function() srd.workspace.switch(i) end, "Switch to workspace " .. i)
    srd.bind("Mod4+Shift+" .. i, function() srd.workspace.move_window(i) end, "Move window to workspace " .. i)
end

- Quick actions
srd.bind("Mod4+d", function() srd.spawn("rofi -show drun") end, "Application launcher")
srd.bind("Mod4+Return", function() srd.spawn("alacritty") end, "Open a terminal")
srd.bind("Mod4+r", function() srd.spawn("rofi -show run") end, "Run a command")

- System
srd.bind("Mod4+Shift+q", function() srd.quit() end, "Quit srdwm")

- Custom function example: toggle window gaps at runtime
local function toggle_gaps()
    if srd.get("general.window_gap") > 0 then
        srd.set("general.window_gap", 0)
        srd.notify("Gaps: Off", "info")
    else
        srd.set("general.window_gap", 8)
        srd.notify("Gaps: On", "info")
    end
end
srd.bind("Mod4+g", toggle_gaps, "Toggle window gaps")

- Media keys
srd.bind("XF86AudioRaiseVolume", function() srd.spawn("pactl set-sink-volume @DEFAULT_SINK@ +5%") end, "Volume up")
srd.bind("XF86AudioLowerVolume", function() srd.spawn("pactl set-sink-volume @DEFAULT_SINK@ -5%") end, "Volume down")
srd.bind("XF86AudioMute", function() srd.spawn("pactl set-sink-mute @DEFAULT_SINK@ toggle") end, "Mute")
srd.bind("XF86MonBrightnessUp", function() srd.spawn("brightnessctl set +5%") end, "Brightness up")
srd.bind("XF86MonBrightnessDown", function() srd.spawn("brightnessctl set 5%-") end, "Brightness down")

print("Key bindings configuration loaded")
