# SRDWM Default Configuration Reference

## Overview
This document describes all default configuration values and available options for SRDWM. These defaults provide a sensible starting point that works well across all platforms.

## Configuration Structure

### Global Settings (`general.*`)
```lua
srd.set("general.default_layout", "dynamic")           -- Default: "dynamic"
srd.set("general.smart_placement", true)               -- Default: true
srd.set("general.window_gap", 8)                       -- Default: 8
srd.set("general.border_width", 2)                     -- Default: 2
srd.set("general.animations", true)                    -- Default: true
srd.set("general.animation_duration", 200)             -- Default: 200ms
srd.set("general.focus_follows_mouse", false)          -- Default: false
srd.set("general.mouse_follows_focus", true)           -- Default: true
srd.set("general.auto_raise", false)                   -- Default: false
srd.set("general.auto_focus", true)                    -- Default: true
```

### Monitor Settings (`monitor.*`)
```lua
srd.set("monitor.primary_layout", "dynamic")           -- Default: "dynamic"
srd.set("monitor.secondary_layout", "tiling")          -- Default: "tiling"
srd.set("monitor.auto_detect", true)                   -- Default: true
srd.set("monitor.primary_workspace", 1)                -- Default: 1
srd.set("monitor.workspace_count", 10)                 -- Default: 10
```

### Window Behavior (`window.*`)
```lua
srd.set("window.focus_follows_mouse", false)           -- Default: false
srd.set("window.mouse_follows_focus", true)            -- Default: true
srd.set("window.auto_raise", false)                    -- Default: false
srd.set("window.auto_focus", true)                     -- Default: true
srd.set("window.raise_on_focus", true)                 -- Default: true
srd.set("window.remember_position", true)              -- Default: true
srd.set("window.remember_size", true)                  -- Default: true
srd.set("window.remember_state", true)                 -- Default: true
```

### Workspace Settings (`workspace.*`)
```lua
srd.set("workspace.count", 10)                         -- Default: 10
srd.set("workspace.names", {"1", "2", "3", "4", "5", "6", "7", "8", "9", "0"})
srd.set("workspace.auto_switch", false)                -- Default: false
srd.set("workspace.persistent", true)                  -- Default: true
srd.set("workspace.auto_back_and_forth", false)        -- Default: false
```

### Performance Settings (`performance.*`)
```lua
srd.set("performance.vsync", true)                     -- Default: true
srd.set("performance.max_fps", 60)                     -- Default: 60
srd.set("performance.window_cache_size", 100)          -- Default: 100
srd.set("performance.event_queue_size", 1000)          -- Default: 1000
srd.set("performance.layout_timeout", 16)              -- Default: 16ms
srd.set("performance.enable_caching", true)            -- Default: true
```

### Debug Settings (`debug.*`)
```lua
srd.set("debug.logging", true)                         -- Default: true
srd.set("debug.log_level", "info")                     -- Default: "info"
srd.set("debug.profile", false)                        -- Default: false
srd.set("debug.trace_events", false)                   -- Default: false
srd.set("debug.show_layout_bounds", false)             -- Default: false
srd.set("debug.show_window_geometry", false)           -- Default: false
```

## Layout-Specific Defaults

### Tiling Layout (`layout.tiling.*`)
```lua
srd.layout.configure("tiling", {
    split_ratio = 0.5,                                 -- Default: 0.5
    master_ratio = 0.6,                                -- Default: 0.6
    auto_swap = true,                                  -- Default: true
    gaps = {
        inner = 8,                                      -- Default: 8
        outer = 16                                      -- Default: 16
    },
    behavior = {
        new_window_master = false,                      -- Default: false
        auto_balance = true,                            -- Default: true
        preserve_ratio = true                           -- Default: true
    }
})
```

### Dynamic Layout (`layout.dynamic.*`)
```lua
srd.layout.configure("dynamic", {
    snap_threshold = 50,                                -- Default: 50
    grid_size = 6,                                      -- Default: 6
    cascade_offset = 30,                                -- Default: 30
    smart_placement = true,                             -- Default: true
    gaps = {
        inner = 8,                                      -- Default: 8
        outer = 16                                      -- Default: 16
    },
    behavior = {
        remember_positions = true,                      -- Default: true
        auto_arrange = true,                            -- Default: true
        overlap_prevention = true                       -- Default: true
    }
})
```

### Floating Layout (`layout.floating.*`)
```lua
srd.layout.configure("floating", {
    default_position = "center",                        -- Default: "center"
    remember_position = true,                           -- Default: true
    always_on_top = false,                             -- Default: false
    gaps = {
        inner = 0,                                      -- Default: 0
        outer = 16                                      -- Default: 16
    },
    behavior = {
        allow_resize = true,                            -- Default: true
        allow_move = true,                              -- Default: true
        snap_to_edges = true                            -- Default: true
    }
})
```

## Theme Defaults

### Colors (`theme.colors.*`)
```lua
srd.theme.set_colors({
    background = "#2e3440",                             -- Default: Nord dark
    foreground = "#eceff4",                             -- Default: Nord light
    primary = "#88c0d0",                                -- Default: Nord blue
    secondary = "#81a1c1",                              -- Default: Nord blue-gray
    accent = "#5e81ac",                                 -- Default: Nord blue
    error = "#bf616a",                                  -- Default: Nord red
    warning = "#ebcb8b",                                -- Default: Nord yellow
    success = "#a3be8c"                                 -- Default: Nord green
})
```

### Window Decorations (`theme.decorations.*`)
```lua
srd.theme.set_decorations({
    border = {
        width = 2,                                      -- Default: 2
        active_color = "#88c0d0",                       -- Default: Nord blue
        inactive_color = "#2e3440",                     -- Default: Nord dark
        focused_style = "solid",                        -- Default: "solid"
        unfocused_style = "solid"                       -- Default: "solid"
    },
    title_bar = {
        height = 24,                                    -- Default: 24
        show = true,                                    -- Default: true
        font = "JetBrains Mono 10",                     -- Default: "JetBrains Mono 10"
        background = "#2e3440",                         -- Default: Nord dark
        foreground = "#eceff4"                          -- Default: Nord light
    }
})
```

## Key Binding Defaults

### Essential Bindings
```lua
-- Layout switching
srd.bind("Mod4+1", function() srd.layout.set("tiling") end)      -- Default: Mod4+1
srd.bind("Mod4+2", function() srd.layout.set("dynamic") end)     -- Default: Mod4+2
srd.bind("Mod4+3", function() srd.layout.set("floating") end)    -- Default: Mod4+3

-- Window management
srd.bind("Mod4+q", function() srd.window.close() end)            -- Default: Mod4+q
srd.bind("Mod4+m", function() srd.window.minimize() end)         -- Default: Mod4+m
srd.bind("Mod4+f", function() srd.window.maximize() end)         -- Default: Mod4+f

-- Window movement (vim-style)
srd.bind("Mod4+h", function() srd.window.focus("left") end)      -- Default: Mod4+h
srd.bind("Mod4+j", function() srd.window.focus("down") end)      -- Default: Mod4+j
srd.bind("Mod4+k", function() srd.window.focus("up") end)        -- Default: Mod4+k
srd.bind("Mod4+l", function() srd.window.focus("right") end)     -- Default: Mod4+l

-- Workspace management
srd.bind("Mod4+Tab", function() srd.workspace.next() end)        -- Default: Mod4+Tab
srd.bind("Mod4+Shift+Tab", function() srd.workspace.prev() end)  -- Default: Mod4+Shift+Tab

-- Quick actions
srd.bind("Mod4+d", function() srd.spawn("rofi -show drun") end) -- Default: Mod4+d
srd.bind("Mod4+Return", function() srd.spawn("alacritty") end)  -- Default: Mod4+Return
```

## Platform-Specific Defaults

### Linux (X11/Wayland)
```lua
-- Auto-detect backend
srd.set("platform.backend", "auto")                    -- Default: "auto"

-- X11 specific
srd.set("platform.x11.use_ewmh", true)                 -- Default: true
srd.set("platform.x11.use_netwm", true)                -- Default: true

-- Wayland specific
srd.set("platform.wayland.use_xdg_shell", true)        -- Default: true
srd.set("platform.wayland.use_layer_shell", true)      -- Default: true
```

### Windows
```lua
-- Windows specific
srd.set("platform.windows.use_dwm", true)              -- Default: true
srd.set("platform.windows.use_win32", true)            -- Default: true
srd.set("platform.windows.global_hooks", true)         -- Default: true
```

### macOS
```lua
-- macOS specific
srd.set("platform.macos.use_cocoa", true)              -- Default: true
srd.set("platform.macos.use_core_graphics", true)      -- Default: true
srd.set("platform.macos.accessibility_enabled", true)  -- Default: true
```

## Configuration File Locations

### Linux
- **Config**: `~/.config/srdwm/srd/`
- **Themes**: `~/.config/srdwm/themes/`
- **Scripts**: `~/.config/srdwm/scripts/`
- **Cache**: `~/.cache/srdwm/`
- **Logs**: `~/.local/share/srdwm/logs/`

### Windows
- **Config**: `%APPDATA%\srdwm\srd\`
- **Themes**: `%APPDATA%\srdwm\themes\`
- **Scripts**: `%APPDATA%\srdwm\scripts\`
- **Cache**: `%LOCALAPPDATA%\srdwm\cache\`
- **Logs**: `%LOCALAPPDATA%\srdwm\logs\`

### macOS
- **Config**: `~/Library/Application Support/srdwm/srd/`
- **Themes**: `~/Library/Application Support/srdwm/themes/`
- **Scripts**: `~/Library/Application Support/srdwm/scripts/`
- **Cache**: `~/Library/Caches/srdwm/`
- **Logs**: `~/Library/Logs/srdwm/`

## Environment Variables

```bash
# Configuration path override
export SRDWM_CONFIG_PATH="/path/to/config"

# Theme override
export SRDWM_THEME="nord"

# Debug level
export SRDWM_DEBUG_LEVEL="debug"

# Platform override
export SRDWM_PLATFORM="wayland"

# Performance settings
export SRDWM_MAX_FPS="120"
export SRDWM_VSYNC="false"
```

## Reset to Defaults

To reset any setting to its default value:

```lua
-- Reset specific setting
srd.reset("general.window_gap")

-- Reset all settings
srd.reset_all()

-- Reset specific category
srd.reset_category("general")
```

## Validation Rules

### Numeric Values
- **Gaps**: 0-100 pixels
- **Border width**: 0-20 pixels
- **Animation duration**: 0-1000ms
- **FPS**: 30-240
- **Cache size**: 10-10000

### String Values
- **Layout names**: Must be registered layouts
- **Theme names**: Must be valid theme files
- **Font names**: Must be system fonts
- **Color values**: Must be valid hex colors

### Boolean Values
- **Features**: true/false
- **Debug options**: true/false
- **Performance options**: true/false

## Best Practices

1. **Start with defaults**: Don't change settings unless necessary
2. **Test changes**: Always test configuration changes
3. **Backup configs**: Keep backups of working configurations
4. **Use comments**: Document custom configurations
5. **Validate syntax**: Use `srd.validate_config()` before reloading

## Troubleshooting

### Common Issues
- **Config not loading**: Check file permissions and syntax
- **Settings not applying**: Verify setting names and values
- **Performance issues**: Check performance settings
- **Layout problems**: Verify layout configuration

### Debug Commands
```lua
-- Check configuration status
srd.debug.config_status()

-- Validate current configuration
srd.debug.validate_config()

-- Show current settings
srd.debug.show_settings()

-- Performance profiling
srd.debug.profile_start()
srd.debug.profile_stop()
```

This documentation provides a comprehensive reference for all default values and configuration options in SRDWM.


