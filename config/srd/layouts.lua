- Layout tuning. Keys and defaults match docs/DEFAULTS.md.
local srd = require("srd")

srd.layout.configure("tiling", {
    master_ratio = 0.6,
    gaps = { inner = 8, outer = 16 },
})

srd.layout.configure("dynamic", {
    snap_threshold = 50,
    cascade_offset = 30,
    gaps = { inner = 8, outer = 16 },
})

srd.layout.configure("floating", {
    default_position = "center",
    gaps = { inner = 0, outer = 16 },
})
