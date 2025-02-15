# SRDWM Default Configuration Reference

## Overview
This document describes all default configuration values and available options for SRDWM. These defaults provide a sensible starting point that works well across all platforms.

## Configuration Structure

### Global Settings (`general.*`)
```lua
srd.set("general.default_layout", "dynamic")           -- Default: "dynamic"
srd.set("general.window_gap", 8)                       -- Default: 8
srd.set("general.animations", true)                    -- Default: true
srd.set("general.animation_duration", 200)             -- Default: 200ms
srd.set("general.shadows", true)                       -- Default: true
srd.set("general.resize_margin", 6)                    -- Default: 6px
srd.set("general.rounded_corners", true)               -- Default: true on GLES/winit, false on udev/Pixman (opt-in there)
srd.set("general.focus_follows_mouse", false)          -- Default: false - hover a window to focus it, no click needed
srd.set("general.auto_raise", false)                   -- Default: false - also raise on hover-focus, not just focus
```
`general.smart_placement`/`general.border_width` are not listed: neither
is implemented - new-window placement always uses smart placement
unconditionally (no toggle exists), and the real, working border-width
setting is `theme.decorations.border.width` below.
`general.mouse_follows_focus` (warp the pointer to match a keybinding-
driven focus change) and `general.auto_focus` (no clear distinct meaning
found beyond what plain click-to-focus already does) aren't implemented
either.

### Monitor Settings (`monitor.*`)
```lua
srd.set("monitor.primary_layout", "dynamic")           -- Default: "dynamic"
srd.set("monitor.secondary_layout", "tiling")          -- Default: "tiling"
```
These two keys set the layout for the primary monitor and for secondary
monitors. They apply only when `workspace.per_monitor` is `true`. Set
`workspace.per_monitor` to `false` (the default) and every monitor shares
one workspace. In that mode there is no separate primary/secondary
workspace to apply a different layout to, so these keys do nothing.

`monitor.auto_detect` is not implemented. Monitor detection runs
unconditionally; there is no toggle for it.

#### `srd.monitor.split(name, parts[, direction])`

Splits one physical monitor into `parts` equal logical monitors. Each
logical monitor gets its own name (`eDP-1-1`, `eDP-1-2`, and so on), its
own id, and its own placement rules. Windows tile and place within one
logical monitor the same way they do on a real one.

`direction` is `"columns"` (default) or `"rows"`. Columns place the
logical monitors side by side. Rows stack them.

```lua
srd.monitor.split("eDP-1", 2)              -- two side-by-side halves
srd.monitor.split("HDMI-A-1", 2, "rows")   -- two stacked halves
```

This does not create a second `wl_output`. A client that asks for
`wl_output.enter` or the output's scale still sees the one real output.
Fullscreen and maximize inside a logical monitor stay within that logical
monitor's own bounds; they do not spill into the other half.

`srd monitors` and the `monitors` event mark each split part with
`"split": true`. An ordinary, undivided output reports `"split": false`.
A display-arrangement UI should read this field and treat a split part
differently from a real output: do not offer to move it, resize it, or
extend a physical arrangement onto it, since there is no independent
output behind it.

#### `srd.monitor.scale(name, factor)`

Sets the output scale for one monitor by connector name.

srdwm sets scale automatically by default. It reads each monitor's
physical size and resolution from EDID, computes real pixel density
(PPI), and scales down monitors below roughly 109 PPI - a large monitor
at the same resolution as a smaller one, for example. It never scales a
monitor above `1.0` on its own.

Use `srd.monitor.scale` to override the automatic value for one
connector:

```lua
srd.monitor.scale("HDMI-A-1", 0.75)
```

Set `factor` to `0` or less to clear an override and return that
connector to the automatic value.

A scale change, automatic or explicit, takes effect the next time srdwm
brings that connector's output up: at startup, at hotplug, or after
`srd dispatch set output enabled <name> true`. It does not apply to an
already-running output on a plain config reload.

### Window Behavior (`window.*`)
Not implemented: this whole namespace duplicated `general.*`'s own focus
keys (see above for the two of those that are real) plus three genuinely
unbuilt per-app-window-state-persistence keys (`remember_position`/
`remember_size`/`remember_state` - no window remembers anything about a
previous run; every new window starts at a hardcoded size, see
`crates/wayland/src/xwayland.rs`/`state/lifecycle.rs`'s `new_managed_window`).
```

### Workspace Settings (`workspace.*`)
```lua
srd.set("workspace.count", 10)                         -- Default: 10
srd.set("workspace.names", {"1", "2", "3", "4", "5", "6", "7", "8", "9", "0"})
srd.set("workspace.auto_back_and_forth", false)        -- Default: false
```
(`workspace.auto_switch`/`workspace.persistent` are not implemented - not
listed here since setting either currently does nothing.)

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
    snap_threshold = 20,                                -- Default: 20
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
        width = 4,                                      -- Default: 4
        radius = 12,                                     -- Default: 12 (corner radius, logical px)
        active_color = "#88c0d0",                       -- Default: Nord blue
        inactive_color = "#2e3440",                     -- Accepted, not read: see inactive_dim below
        inactive_dim = 0.35,                            -- Default: 0.35 - unfocused border = active_color scaled by this factor (0 = black, 1 = same as focused). The real unfocused-border knob; inactive_color above is validated but never applied, since an absolute override would erase the dimming scheme instead of participating in it.
        focused_style = "solid",                        -- Default: "solid"
        unfocused_style = "solid"                       -- Default: "solid"
    },
    title_bar = {
        height = 32,                                    -- Not actually read - see the note below the table
        show = true,                                    -- Default: true
        font = "JetBrains Mono 10",                     -- Default: "JetBrains Mono 10"
        background = "#2e3440",                         -- Default: Nord dark
        foreground = "#eceff4",                         -- Accepted, not read: see foreground_focused/foreground_unfocused below
        foreground_focused = "#88c0d0",                 -- Default: Nord blue - titlebar text/icon colour on the focused window
        foreground_unfocused = "#4c566a"                -- Default: Nord dark gray - titlebar text/icon colour on every other window
    },
    - Which decoration mode a window gets before any per-window rule or
    - the window's own `xdg-decoration` request has a say. "server"
    - (default): srdwm draws its own titlebar for anything that doesn't
    - ask otherwise - consistent Windows/macOS-style chrome, and KDE/
    - KWin's own choice too. "client": srdwm steps back by default and
    - lets every window draw its own chrome (GNOME/Mutter's approach) --
    - a window with no titlebar opinion of its own then gets no titlebar
    - at all. A client that explicitly requests a mode is always honored
    - either way; this only decides what a client with *no* preference
    - ends up with. Also live-settable without a config reload/restart:
    - `srd set decoration_mode server` / `srd set decoration_mode client`
    - (affects windows created after the call, not already-open ones).
    default_mode = "server"                             -- Default: "server"
})
```

`title_bar.height` above is accepted but not read: the real titlebar band
is `srdwm_core::TITLEBAR_HEIGHT`, a compile-time constant (currently `32`)
shared between hit-testing and rendering so the two can never disagree.
Setting this key has no effect; it's kept in the table only so a preset
copied from here doesn't read as silently missing a value real desktops
all expose.

Four more `theme.decorations.title_bar.*` keys are flat `srd.set` values, not part of
the `set_decorations` table above:

```lua
srd.set("theme.decorations.title_bar.text_align", "left")     -- Default: "left"
srd.set("theme.decorations.title_bar.button_side", "right")   -- Default: "right"
srd.set("theme.decorations.title_bar.button_order", "")        -- Default: "" (unset)
srd.set("theme.decorations.title_bar.button_glyph", "hover")   -- Default: "hover"
srd.set("theme.decorations.title_bar.button_style", "traffic_lights")  -- Default: "traffic_lights"
```

`text_align` sets `"center"` for the macOS convention (title centered on
the whole titlebar width, ignoring the button cluster the way real macOS
does) or `"left"` (default) for the Windows/GTK convention. Any value
other than exactly `"center"` keeps `"left"`.

`button_side` sets `"left"` for the macOS convention (close, minimize,
maximize, left to right) or `"right"` for the Windows/GTK convention
(minimize, maximize, close). Any value other than exactly `"left"` keeps
`"right"`.

`button_order` overrides the relative order of the three buttons on
whichever side `button_side` selects. Set it to a comma-separated list
naming each button once: `"close,minimize,maximize"`. Unset (the
default) keeps `button_side`'s own built-in order. A value that does not
name each button exactly once is rejected with a warning in the log; the
built-in order stays in effect.

`button_side` and `button_order` are independent settings: `button_side`
alone chooses which convention's default order applies; `button_order`
replaces that order with your own, applied to whichever side `button_side`
selected. This matches KWin's `ButtonsOnLeft`/`ButtonsOnRight`, GNOME/
Adwaita's `decoration-layout`, and Openbox's `titlelayout` - each
independently uses a comma- or letter-separated button list for exactly
this purpose.

`button_glyph` sets `"always"` to keep each button's icon visible at all
times (modern GNOME/Adwaita), or `"hover"` (the default, classic macOS)
to hide the icon until the pointer is over that button.

`button_style` sets `"traffic_lights"` (the default) for filled, coloured
macOS-style dots with a glossy gradient and a "zoom" double-arrow maximize
icon, or `"traditional"` for plain glyphs (X / square / dash) drawn
straight on the titlebar background - no filled dot except a subtle,
neutral hover backdrop - and a plain square maximize icon, matching a
real Windows or GNOME titlebar's own convention instead. Any value other
than exactly `"traditional"` keeps `"traffic_lights"`. Independent of
`button_side` and `button_order`: either style can sit on either side in
either order, though `"traffic_lights"` paired with `button_side =
"left"` (macOS) and `"traditional"` paired with `button_side = "right"`
(Windows/GNOME) are the two ready-made combinations `config/srd/
themes.lua` ships (`macos` and `traditional` presets respectively).

### Getting a full macOS look

`apply(macos)` in `config/srd/themes.lua` covers everything srdwm itself
draws: centered titlebar text, left-aligned traffic-light buttons with a
glossy gradient, grey chrome matching a dark GTK theme. It does **not**
cover any other application's own window decoration - an undecorated
(client-side-decorated) window like Firefox or a GTK file manager draws
its *own* titlebar buttons, entirely outside srdwm's control. Getting
those to match too is three separate, srdwm-external steps, each a normal
part of *this desktop's* own configuration, not something a compositor
can apply on an app's behalf:

1. **Button side for every GTK/Firefox-on-Linux app**: Firefox and GTK3/4
   apps both read `org.gnome.desktop.wm.preferences button-layout`
   directly for which side to draw their own window buttons on --
   independent of any srdwm setting.
   ```sh
   gsettings set org.gnome.desktop.wm.preferences button-layout 'close,minimize,maximize:'
   ```
   (System-wide; revert with `'appmenu:minimize,maximize,close'`, GNOME's
   own default.)

2. **Firefox's own button colour/gradient**: Firefox draws its titlebar
   buttons itself and only reads a stylesheet you provide, in your own
   profile, never srdwm's config. Requires `toolkit.
   legacyUserProfileCustomizations.stylesheets = true` in `about:config`
   (one-time per profile) plus a full Firefox restart, and a
   `userChrome.css` in that profile's own `chrome/` directory targeting
   `.titlebar-close`/`.titlebar-min`/`.titlebar-max`/`.titlebar-restore`
   (Firefox's own class names for these buttons - confirmed directly
   against a real Firefox install's own `browser.xhtml`, inside its
   `omni.ja`, rather than assumed, since these have changed across
   versions before).

3. **Other GTK apps' own button colour/gradient** (Nemo, etc.): these
   follow the system GTK theme, so a user `~/.config/gtk-3.0/gtk.css` /
   `~/.config/gtk-4.0/gtk.css` overriding `headerbar button.titlebutton`
   (loaded after whatever theme is active in `gtk-theme-name`) gets the
   same gradient onto every such app at once, without touching the theme
   package itself.

None of this ships from srdwm or from this repo's own `config/srd/` --
it lives in each user's own dotfiles/profile, the same as any other
per-application customization on Linux.

## Key Binding Defaults

### Essential Bindings
```lua
- Layout switching
srd.bind("Mod4+1", function() srd.layout.set("tiling") end)      -- Default: Mod4+1
srd.bind("Mod4+2", function() srd.layout.set("dynamic") end)     -- Default: Mod4+2
srd.bind("Mod4+3", function() srd.layout.set("floating") end)    -- Default: Mod4+3

- Window management
srd.bind("Mod4+q", function() srd.window.close() end)            -- Default: Mod4+q
srd.bind("Mod4+m", function() srd.window.minimize() end)         -- Default: Mod4+m
srd.bind("Mod4+f", function() srd.window.maximize() end)         -- Default: Mod4+f

- Window movement (vim-style)
srd.bind("Mod4+h", function() srd.window.focus("left") end)      -- Default: Mod4+h
srd.bind("Mod4+j", function() srd.window.focus("down") end)      -- Default: Mod4+j
srd.bind("Mod4+k", function() srd.window.focus("up") end)        -- Default: Mod4+k
srd.bind("Mod4+l", function() srd.window.focus("right") end)     -- Default: Mod4+l

- Workspace management
srd.bind("Mod4+Tab", function() srd.workspace.next() end)        -- Default: Mod4+Tab
srd.bind("Mod4+Shift+Tab", function() srd.workspace.prev() end)  -- Default: Mod4+Shift+Tab

- Quick actions
srd.bind("Mod4+d", function() srd.spawn("rofi -show drun") end) - Default: Mod4+d
srd.bind("Mod4+Return", function() srd.spawn("alacritty") end)  -- Default: Mod4+Return
```

## Window Rules

Match windows by title/class and apply actions once, when they're first
created:

```lua
srd.rule(matcher, actions)
```

`matcher` fields (at least one required; an empty matcher matches nothing):
- `title` - case-insensitive substring match against the window title.
- `class` (alias `app_id`) - case-insensitive exact match against the
  window's `WM_CLASS` (X11) / `app_id` (Wayland).

`actions` fields (all optional):
- `floating` (bool), `maximized` (bool)
- `workspace` (number) - workspace id to place the window on
- `x`, `y`, `width`, `height` (number) - explicit geometry; all four must be
  given together to take effect
- `decorated` (bool)
- `border_color` (`{r, g, b}`), `border_width` (number), `corner_radius` (number)
- `pinned` (bool) - always-on-top
- `opacity` (number, `0.0`..=`1.0`) - content opacity; srdwm's own
  titlebar/border/shadow always stay fully opaque regardless
- `resize_margin` (number, logical pixels) - per-window override of
  `general.resize_margin` (Hyprland's per-window `extend_border_grab_area`).
  Also settable live via `srd.window.set_resize_margin(n)` on the focused
  window.

```lua
srd.rule({ class = "pavucontrol" }, { floating = true })
srd.rule({ title = "Picture-in-Picture" }, { floating = true, width = 480, height = 270 })
```

## Platform-Specific Defaults

### Linux (X11/Wayland)
```lua
- Auto-detect backend
srd.set("platform.backend", "auto")                    -- Default: "auto"

- X11 specific
srd.set("platform.x11.use_ewmh", true)                 -- Default: true
srd.set("platform.x11.use_netwm", true)                -- Default: true

- Wayland specific
srd.set("platform.wayland.use_xdg_shell", true)        -- Default: true
srd.set("platform.wayland.use_layer_shell", true)      -- Default: true
```

### Windows
```lua
- Windows specific
srd.set("platform.windows.use_dwm", true)              -- Default: true
srd.set("platform.windows.use_win32", true)            -- Default: true
srd.set("platform.windows.global_hooks", true)         -- Default: true
```

### macOS
```lua
- macOS specific
srd.set("platform.macos.use_cocoa", true)              -- Default: true
srd.set("platform.macos.use_core_graphics", true)      -- Default: true
srd.set("platform.macos.accessibility_enabled", true)  -- Default: true
```

## Configuration File Locations

### Linux
- **Config**: `~/.config/srd/`
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
- **Config**: `~/Library/Application Support/srdwm/`
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
- Reset specific setting
srd.reset("general.window_gap")

- Reset all settings
srd.reset_all()

- Reset specific category
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
- Check configuration status: returns { keys, bound_keys, log_entries, config_dir }
local status = srd.debug.config_status()

- Validate current configuration against docs/DEFAULTS.md's ranges/formats:
- returns ok (bool), errors (array of human-readable strings, empty when ok)
local ok, errors = srd.debug.validate_config()
- equivalently, at the top level:
local ok, errors = srd.validate_config()

- Show current settings: logs every key = value and returns them as a table
local settings = srd.debug.show_settings()

- Performance profiling: profile_stop() returns elapsed seconds (number)
srd.debug.profile_start()
local elapsed = srd.debug.profile_stop()
```

This documentation provides a comprehensive reference for all default values and configuration options in SRDWM.


