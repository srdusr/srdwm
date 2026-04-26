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
srd.bind("Mod4+q", function() srd.window.close() end)
srd.bind("Mod4+m", function() srd.window.minimize() end)
srd.bind("Mod4+f", function() srd.window.maximize() end)
srd.bind("Mod4+Shift+space", function() srd.window.toggle_floating() end)

- Window navigation (vim-style directional focus)
srd.bind("Mod4+h", function() srd.window.focus("left") end)
srd.bind("Mod4+j", function() srd.window.focus("down") end)
srd.bind("Mod4+k", function() srd.window.focus("up") end)
srd.bind("Mod4+l", function() srd.window.focus("right") end)

- Move the focused window in a direction, mirroring the focus keys above.
- `srd.window.move` has existed for a long time and simply had no default
- binding, so "move window to absolute directions" was unreachable without
- writing your own config.
srd.bind("Mod4+Shift+h", function() srd.window.move("left") end)
srd.bind("Mod4+Shift+j", function() srd.window.move("down") end)
srd.bind("Mod4+Shift+k", function() srd.window.move("up") end)
srd.bind("Mod4+Shift+l", function() srd.window.move("right") end)

- Layout switching. srdwm is dynamic-first: "dynamic" is free placement,
- the default, and tiling is one opt-in layout among several. Mod4+s
- toggles between the two, which is the pair worth a single key.
srd.bind("Mod4+s", function()
    if srd.layout.get() == "tiling" then
        srd.layout.set("dynamic")
    else
        srd.layout.set("tiling")
    end
end)

- Lock the session using srdwm's own built-in lock screen. `srd.lock()`
- talks to the compositor directly rather than shelling out to the control
- CLI, so it still works when that binary is not on PATH.
srd.bind("Mod4+Ctrl+l", function() srd.lock() end)

- Workspace management
srd.bind("Mod4+Tab", function() srd.workspace.next() end)
srd.bind("Mod4+Shift+Tab", function() srd.workspace.prev() end)

- Workspace switching with number keys (0 doubles as "workspace 10")
for i = 0, 9 do
    srd.bind("Mod4+" .. i, function() srd.workspace.switch(i) end)
    srd.bind("Mod4+Shift+" .. i, function() srd.workspace.move_window(i) end)
end

- Quick actions
srd.bind("Mod4+d", function() srd.spawn("rofi -show drun") end)
srd.bind("Mod4+Return", function() srd.spawn("alacritty") end)
srd.bind("Mod4+r", function() srd.spawn("rofi -show run") end)

- System
srd.bind("Mod4+Shift+q", function() srd.quit() end)

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
srd.bind("Mod4+g", toggle_gaps)

- Media keys
srd.bind("XF86AudioRaiseVolume", function() srd.spawn("pactl set-sink-volume @DEFAULT_SINK@ +5%") end)
srd.bind("XF86AudioLowerVolume", function() srd.spawn("pactl set-sink-volume @DEFAULT_SINK@ -5%") end)
srd.bind("XF86AudioMute", function() srd.spawn("pactl set-sink-mute @DEFAULT_SINK@ toggle") end)
srd.bind("XF86MonBrightnessUp", function() srd.spawn("brightnessctl set +5%") end)
srd.bind("XF86MonBrightnessDown", function() srd.spawn("brightnessctl set 5%-") end)

print("Key bindings configuration loaded")
