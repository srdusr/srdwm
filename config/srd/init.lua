-- SRDWM Main Configuration Entry Point
-- This file is loaded first and sets up the basic configuration

local srd = require("srd")

-- Load all configuration modules
srd.load("keybindings")
srd.load("layouts") 
srd.load("themes")
srd.load("rules")
srd.load("monitors")
srd.load("startup")

-- Global settings
srd.set("general.default_layout", "dynamic")
srd.set("general.smart_placement", true)
srd.set("general.window_gap", 8)
srd.set("general.border_width", 2)
srd.set("general.animations", true)
srd.set("general.animation_duration", 200)

-- Monitor settings
srd.set("monitor.primary_layout", "dynamic")
srd.set("monitor.secondary_layout", "tiling")

-- Window behavior
srd.set("window.focus_follows_mouse", false)
srd.set("window.mouse_follows_focus", true)
srd.set("window.auto_raise", false)
srd.set("window.auto_focus", true)

-- Workspace settings
srd.set("workspace.count", 10)
srd.set("workspace.names", {
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "0"
})

-- Performance settings
srd.set("performance.vsync", true)
srd.set("performance.max_fps", 60)
srd.set("performance.window_cache_size", 100)

-- Debug settings
srd.set("debug.logging", true)
srd.set("debug.log_level", "info")
srd.set("debug.profile", false)

-- Print startup message
srd.notify("SRDWM Configuration Loaded", "info")
print("SRDWM configuration loaded successfully")

