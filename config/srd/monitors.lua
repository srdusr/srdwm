- Per-monitor defaults (docs/DEFAULTS.md monitor.* keys).
local srd = require("srd")

srd.set("monitor.primary_layout", "dynamic")
srd.set("monitor.secondary_layout", "tiling")
srd.set("monitor.auto_detect", true)
