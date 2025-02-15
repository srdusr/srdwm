- Theme presets for srdwm. Pick one by leaving its `apply(...)` call
- uncommented at the bottom and commenting out the others - exactly one
- should be active. Copying a whole preset table, editing values, and
- pointing the bottom call at the copy works too; nothing here is special
- beyond being a plain Lua table passed to srd.theme.set_colors/
- set_decorations.
--
- `border.inactive_dim` is a factor applied to `active_color` for the
- unfocused-window border, not a second explicit colour - 0.0 fades it to
- black, 1.0 makes it identical to focused. See docs/DEFAULTS.md.
--
- `border.width = 4`, not a thinner value: srdwm rounds the border's own
- corner via a CPU-rasterised bitmap, not a real-resolution GPU shader, so
- a 2px-thick strip only has 2 rows to draw a curve into - not enough to
- read as curved at all, just a slightly-softened square. 4px is the
-- minimum found actually visible as a real curve; go lower and the corner
- starts looking square again regardless of `radius`.
--
- `title_bar.height` below is *not* the real titlebar band - that's
- `srdwm_core::TITLEBAR_HEIGHT` (currently `32`), a compile-time constant
- shared between hit-testing and rendering so the two can never disagree.
- Kept here at the same value purely so this file doesn't read as
- disagreeing with the real height; changing it has no effect.
--
- `title_bar.button_style` - "traffic_lights" (default: filled, coloured
- macOS-style dots, zoom-arrow maximize) or "traditional" (plain glyphs
- straight on the titlebar background, no fill except a neutral hover
- backdrop, square maximize) - see `ThemeConfig::traffic_light_buttons`'s
- own doc comment. Independent of `button_side`: either style can sit on
- either side, though `traffic_lights` + left and `traditional` + right
- are this file's own two ready-made combinations below, matching macOS
- and Windows/GNOME convention respectively.

local srd = require("srd")

local function apply(preset)
    srd.theme.set_colors(preset.colors)
    srd.theme.set_decorations(preset.decorations)
end

- ---------------------------------------------------------------------
- Nord (dark) - srdwm's own compiled-in defaults (`ThemeConfig::
- default`), written out explicitly so the preset is visible and
- editable rather than left implicit.
- ---------------------------------------------------------------------
local nord = {
    colors = {
        background = "#2e3440",
        foreground = "#eceff4",
        primary = "#88c0d0",
        secondary = "#81a1c1",
        accent = "#5e81ac",
        error = "#bf616a",
        warning = "#ebcb8b",
        success = "#a3be8c",
    },
    decorations = {
        border = {
            width = 4,
            radius = 12,
            active_color = "#88c0d0",
            inactive_dim = 0.35,
        },
        title_bar = {
            height = 32,
            show = true,
            background = "#2e3440",
            foreground_focused = "#88c0d0",
            foreground_unfocused = "#4c566a",
            text_align = "left",
            button_side = "right",
            button_glyph = "hover",
        },
    },
}

- ---------------------------------------------------------------------
- macOS - centered title, left-aligned traffic-light buttons (real
- macOS convention, not this system's own GTK button-layout convention --
- see `title_bar.button_side`'s own doc comment in `crates/core/src/
- theme.rs` if picking a different button order/side). Grey titlebar
- background matches a real GTK dark theme's own CSD row (measured live
- off a real WhiteSur-Dark screenshot, (44,44,44)) rather than fighting
- it with an accent colour, so srdwm's own decorated windows read as the
- same family of chrome as Firefox/Nemo/every other GTK app's CSD, not a
- visibly different one sitting right next to them.
--
- For the *system* half of a full macOS look (which side Firefox/GTK
- apps' own CSD buttons draw on, real traffic-light gradients on GTK
- apps' own dots) see docs/DEFAULTS.md's "macOS look" section - that
- part is outside srdwm entirely (a `gsettings` value plus a userChrome.
- css/gtk.css of your own), so it isn't and can't be shipped from here.
- ---------------------------------------------------------------------
local macos = {
    colors = {
        background = "#1e1e1e",
        foreground = "#f5f5f5",
        primary = "#0a84ff",
        secondary = "#5e5ce6",
        accent = "#64d2ff",
        error = "#ff453a",
        warning = "#ffd60a",
        success = "#32d74b",
    },
    decorations = {
        border = {
            width = 4,
            radius = 12,
            active_color = "#0a84ff",
            inactive_dim = 0.35,
        },
        title_bar = {
            height = 32,
            show = true,
            background = "#2c2c2c",
            foreground_focused = "#f5f5f5",
            foreground_unfocused = "#9a9a9a",
            text_align = "center",
            button_side = "left",
            button_glyph = "hover",
        },
    },
}

- ---------------------------------------------------------------------
- Traditional - plain glyphs, right-aligned, no traffic lights: this
- project's own original look and the real Windows/GNOME convention, as
- distinct from `macos` above. Otherwise identical to `nord`; only
- `button_style` differs.
- ---------------------------------------------------------------------
local traditional = {
    colors = nord.colors,
    decorations = {
        border = nord.decorations.border,
        title_bar = {
            height = 32,
            show = true,
            background = "#2e3440",
            foreground_focused = "#88c0d0",
            foreground_unfocused = "#4c566a",
            text_align = "left",
            button_side = "right",
            button_glyph = "hover",
            button_style = "traditional",
        },
    },
}

- Pick exactly one. Comment out the other two.
apply(nord)
- apply(macos)
- apply(traditional)
