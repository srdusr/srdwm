- Nord-inspired theme (matches the built-in defaults; edit freely).
local srd = require("srd")

srd.theme.set_colors({
    background = "#2e3440",
    foreground = "#eceff4",
    primary = "#88c0d0",
    secondary = "#81a1c1",
    accent = "#5e81ac",
    error = "#bf616a",
    warning = "#ebcb8b",
    success = "#a3be8c",
})

srd.theme.set_decorations({
    border = {
        width = 2,
        active_color = "#88c0d0",
        inactive_color = "#4c566a",
    },
    title_bar = {
        height = 30,
        show = true,
        background = "#2e3440",
        foreground = "#eceff4",
    },
})
