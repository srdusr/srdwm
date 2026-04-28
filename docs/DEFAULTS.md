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
srd.set("general.gpu", false)                          -- Default: false - udev backend only, see "GPU rendering" below
srd.set("general.desktop_icons", true)                 -- Default: true - see "Desktop icons" below
srd.set("general.desktop_icons_all_monitors", true)    -- Default: true - mirror icons onto every monitor, not just primary
srd.set("general.config_reload_on_write", true)         -- Default: true - re-read init.lua when it changes on disk
srd.set("general.reserve_top", 0)                      -- Default: 0 - static space reserved before any bar/dock connects
srd.set("general.reserve_bottom", 0)                   -- Default: 0 - see "Startup space reservation" below
srd.set("general.reserve_left", 0)                     -- Default: 0
srd.set("general.reserve_right", 0)                    -- Default: 0
srd.set("general.file_manager", "")                    -- Default: "" - empty means dispatch via `xdg-open`
srd.set("general.desktop_icon_single_click", false)    -- Default: false - double-click opens an icon
srd.set("general.terminal", "")                        -- Default: "" - empty tries a common terminal on $PATH
srd.set("general.phone_mode", false)                   -- Default: false - see "Phone mode" below
srd.set("general.close_focus_follows_workspace", false) - Default: false - see below
```

`close_focus_follows_workspace` decides what happens when your currently
focused window closes and no other window is left on the workspace you're
looking at. `false` (the default) leaves you with nothing focused on your
own workspace - it never switches you elsewhere, matching Windows/GNOME/
macOS, none of which change your active workspace just because a window
closed. `true` restores the alternate behaviour: falling back to whichever
window was focused most recently anywhere, switching your active workspace
to follow it there (matching Hyprland's own `focuswindow`-driven
convention). Live-settable: `srd set close_focus_follows_workspace <bool>`.
`general.smart_placement`/`general.border_width` are not listed: neither
is implemented - new-window placement always uses smart placement
unconditionally (no toggle exists), and the real, working border-width
setting is `theme.decorations.border.width` below.
`general.mouse_follows_focus` (warp the pointer to match a keybinding-
driven focus change) and `general.auto_focus` (no clear distinct meaning
found beyond what plain click-to-focus already does) aren't implemented
either.

#### GPU rendering (`general.gpu`)

Real GBM+EGL+`DrmCompositor` GPU rendering for the udev (real-hardware)
backend, instead of its default, always-available software (Pixman/
dumb-buffer) path. Off by default on every platform - unlike
`general.rounded_corners`, this has one unambiguous default regardless
of which backend connects, since GPU rendering is udev-only and still
experimental. Setting it `true` only ever *attempts* GPU rendering: it
falls back to the software path with no user action needed on any
failure at any step (no GBM device, no atomic-modesetting support, a
software-only EGL renderer), so it is safe to enable on a machine or
VM that turns out not to support it - one harmless log line is the
only cost.

Read once at startup, not live-settable via `srd set` - the render
backend is wired into the DRM pipeline when the compositor connects to
its GPU, not something that can be swapped while running.

Still missing decorations (border, titlebar) as of this writing: a
GPU-driven head renders its own clear color, the real cursor, and real
window content (plain, square-cornered, no border/titlebar) - see
`crates/wayland/src/udev/gpu.rs`'s own module doc comment for the
current state. `SRDWM_GPU=1` (an environment variable) remains a
separate, lower-level override for testing without touching config --
either it or `general.gpu` being set is enough to
attempt GPU rendering.

#### Phone mode (`general.phone_mode`)

Off by default. When on, a new window opens maximized instead of
floating/tiled small - the one placement default a phone-shaped screen
actually needs (no real room for more than one window at a time), applied
the same way every other rule-overridable default already is: a rule's
own explicit `floating = true` (a genuinely small popup) or `maximized =
false` still wins over this. Live-settable via `srd set phone_mode
<true|false>` (`WindowManager::phone_mode`'s own doc comment) - changes
only how the *next* new window opens, not a re-layout of windows already
open, same as `general.animations`/`general.shadows`.

Read-only via `srd settings`'s own `phone_mode` field too, specifically so
a shell panel (AGS) can adapt its own chrome (a phone-shaped bar/dock
layout, concretely) to the same signal without a second, separate way to
ask "is this a phone-shaped session" - that adaptation is real work in
*that* project, not this one; this is the one thing srdwm itself needed
to add so a panel has something real to read. Not touchscreen input --
see `docs/TODO.md`'s own touchscreen entry for why that's separately
skipped (no hardware to verify against).

#### Desktop icons (`general.desktop_icons`)

Real, individually-draggable desktop icons rendered above the wallpaper
and below every window: fixed **Home** (`$HOME`), **Computer** (`/`), and
**Trash** (`~/.local/share/Trash/files`, or `$XDG_DATA_HOME/Trash/files`
if set) icons, plus one per real, non-hidden entry of `~/Desktop` (created
if it doesn't exist yet). On by default - unlike `general.gpu`, this is a
purely visual, directly requested feature with no hardware-support
question to hedge against.

`general.desktop_icons_all_monitors` (default `true`) mirrors the same
icon set onto every enabled monitor's own corner, matching real macOS
convention (each display gets its own Desktop icons view) rather than the
older single-monitor-only convention. Set `false` for the original
primary-monitor-only behaviour. One shared set of icons/cells underneath
either way - dragging a mirrored copy on any monitor moves the one real
icon, which then shows in its new cell everywhere it's mirrored.

Real freedesktop icon-theme artwork (`resvg`-rendered SVG, real theme
lookup honoring whatever `gtk-icon-theme-name` is configured - WhiteSur on
this machine) when a theme has the icon; the original hand-drawn flat
shapes remain as the fallback for whatever a theme doesn't ship (see
`icon_theme.rs`'s own module doc comment).

Icons sort into one alphabetical list by label, case-insensitive - the
three fixed shortcuts interleave with real filenames rather than always
coming first, e.g. "Computer" and "Documents" and "Home" and "Trash" sort
exactly where their names put them.

Double-click (or a single click when `general.desktop_icon_single_click`
is `true`) opens an icon: `general.file_manager <path>` if that key is
set, otherwise `xdg-open <path>`. Dragging an icon snaps it to the nearest
free grid cell on release and persists that cell to
`$XDG_STATE_HOME/srd/desktop-icons.json` (else `~/.local/state/srd/...`) --
only icons the user has actually moved get an entry there; everything
else keeps recomputing its default slot on every rescan. The grid's own
origin is re-derived from the primary monitor's current usable geometry
every frame, so it always sits clear of a bar/dock's reserved strip on
whichever edge it's anchored to, even if that reservation only appears
after srdwm's first render (a real startup race with panels like AGS
that connect and register their own exclusive zone after the compositor
is already up).

Right-click an icon: a real file or folder gets **Open**, **Rename**
(inline, Enter to commit/Escape to cancel), and **Delete** (moves it to
`~/.local/share/Trash` per the freedesktop.org Trash spec, same-filesystem
case only - no confirmation prompt, since this is the reversible move-to-
trash, not a permanent delete, the same convention every mainstream file
manager uses). **Home**/**Computer** get **Open** only - they're
shortcuts, not real files, so rename/delete don't apply. **Trash** gets
**Open** and **Empty Trash** (also no prompt, same reversibility
framing - this is the intentional final step, not a slip). Right-click
bare desktop: **New Folder** (creates `~/Desktop/New Folder`, de-
duplicated as `New Folder (2)`, `(3)`, ...), **Open Terminal Here**
(`general.terminal`, or the first of alacritty/kitty/wezterm/foot/gnome-
terminal/konsole/xterm found on `$PATH`, with `~/Desktop` as its working
directory), **Open in File Manager** (opens `~/Desktop` itself in
`general.file_manager`/`xdg-open` - the concrete path to a real file
manager's own richer menu: cut/copy/paste, properties, set-as-wallpaper,
deliberately not reimplemented here), and **Refresh** (re-scans
`~/Desktop`).

Not implemented, deliberately: Cut/Copy/Paste (real interop with a file
manager needs the Wayland `wl_data_device`/`text/uri-list` clipboard
protocol, a separate substantial feature - an srdwm-only internal
clipboard wouldn't achieve real interop anyway), filesystem watching (a
file added to `~/Desktop` by another program needs "Refresh" or a restart
to appear), multi-select, and View/Sort submenus (no nested-menu UI
exists).

#### Startup space reservation (`general.reserve_top`/`_bottom`/`_left`/`_right`)

`0` (no reservation) on every edge by default. A bar or dock only actually
reserves its own strip (`set_exclusive_zone`) once it has connected and
committed a real surface - which happens *after* this compositor's own
first render pass and first-window-placement decisions, since autostart
spawns those clients rather than waiting for them. Desktop icons re-derive
their position every frame so they self-correct once the real zone lands,
but a window placed in that gap gets a one-time placement decision and can
end up spawned under where the bar will render, with nothing to nudge it
out afterward. Setting these to the bar/dock's own known height/width
closes that gap: every usable-area computation already accounts for it
from the first call, before any real client has connected. Only ever
shrinks the usable area *further* than a real, already-connected client's
own zone - a real bar registering a bigger reservation always wins, this
is a floor under it, not a competing claim.

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
Not implemented - every key below is seeded and range-validated
(`window_cache_size`/`max_fps`, per Validation Rules below) but never
read anywhere past that seed call, confirmed the same way as the
Layout-Specific section above. Real frame pacing on this compositor
comes from the display's own hardware vsync (the udev backend's real
page-flip cadence, or the winit backend's own frame-timing logic --
see `crates/wayland/src/winit/nested_platform.rs`'s own doc comment),
not a config knob; `RUST_LOG` (Environment Variables, above) is the
real logging control.
```lua
srd.set("performance.vsync", true)                     -- Stored/validated only, no effect
srd.set("performance.max_fps", 60)                     -- Stored/validated only, no effect
srd.set("performance.window_cache_size", 100)          -- Stored/validated only, no effect
srd.set("performance.event_queue_size", 1000)          -- Stored only, no effect
srd.set("performance.layout_timeout", 16)              -- Stored only, no effect
srd.set("performance.enable_caching", true)            -- Stored only, no effect
```

### Debug Settings (`debug.*`)
Not implemented - every key below is seeded but never read anywhere
past that seed call. `RUST_LOG` (Environment Variables, above) is the
real, working logging control.
```lua
srd.set("debug.logging", true)                         -- Stored only, no effect
srd.set("debug.log_level", "info")                     -- Stored only, no effect
srd.set("debug.profile", false)                        -- Stored only, no effect
srd.set("debug.trace_events", false)                   -- Stored only, no effect
srd.set("debug.show_layout_bounds", false)             -- Stored only, no effect
srd.set("debug.show_window_geometry", false)           -- Stored only, no effect
```

## Layout-Specific Defaults

`srd.layout.configure(name, table)` (`crates/config/src/engine/layout.rs`)
stores every key given under `layout.<name>.*` - `srd.get("layout.tiling.
split_ratio")` genuinely echoes back whatever was last set, and each numeric
`gaps.inner`/`gaps.outer` is range-validated by `srd.validate_config()` --
but of everything below, **only `master_ratio` is actually read back and
applied** to real window geometry (`WindowManager::tiling.master_ratio`,
which the tiling layout's own split math uses). Every other key in this
section - `split_ratio`, `auto_swap`, every `behavior.*` table, `layout.
tiling`/`layout.dynamic`/`layout.floating`'s own `gaps.*`, `snap_threshold`,
`grid_size`, `cascade_offset`, `smart_placement`, `default_position`,
`remember_position`, `always_on_top` - is accepted, stored, and
retrievable, but has no effect on window behavior. Confirmed by grepping
the whole compositor for a second reader of each key beyond the one seed
call in `crates/config/src/engine/support.rs`'s `default_config` --
`master_ratio` is the only one with any other hit.

The real, working gap setting for every layout is `general.window_gap`
(above) - it drives `WindowManager::tiling.gap_inner`/`gap_outer`
directly, unconditionally, regardless of what a layout's own `gaps.inner`/
`gaps.outer` say.

### Tiling Layout (`layout.tiling.*`)
```lua
srd.layout.configure("tiling", {
    split_ratio = 0.5,                                 -- Stored/validated only, no effect
    master_ratio = 0.6,                                -- Default: 0.6 - the one real key here
    auto_swap = true,                                  -- Stored/validated only, no effect
    gaps = {
        inner = 8,                                      -- Stored/validated only - see general.window_gap above
        outer = 16                                      -- Stored/validated only - see general.window_gap above
    },
    behavior = {
        new_window_master = false,                      -- Stored only, no effect
        auto_balance = true,                            -- Stored only, no effect
        preserve_ratio = true                           -- Stored only, no effect
    }
})
```

### Dynamic Layout (`layout.dynamic.*`)
```lua
srd.layout.configure("dynamic", {
    snap_threshold = 20,                                -- Stored only, no effect
    grid_size = 6,                                      -- Stored only, no effect
    cascade_offset = 30,                                -- Stored only, no effect
    smart_placement = true,                             -- Stored only, no effect - placement is unconditionally "smart" already, see general.smart_placement above
    gaps = {
        inner = 8,                                      -- Stored/validated only - see general.window_gap above
        outer = 16                                      -- Stored/validated only - see general.window_gap above
    },
    behavior = {
        remember_positions = true,                      -- Stored only, no effect
        auto_arrange = true,                            -- Stored only, no effect
        overlap_prevention = true                       -- Stored only, no effect
    }
})
```

### Floating Layout (`layout.floating.*`)
```lua
srd.layout.configure("floating", {
    default_position = "center",                        -- Stored only, no effect
    remember_position = true,                           -- Stored only, no effect
    always_on_top = false,                             -- Stored only, no effect - see the real per-window `pinned` rule action instead
    gaps = {
        inner = 0,                                      -- Stored/validated only - see general.window_gap above
        outer = 16                                      -- Stored/validated only - see general.window_gap above
    },
    behavior = {
        allow_resize = true,                            -- Stored only, no effect
        allow_move = true,                              -- Stored only, no effect
        snap_to_edges = true                            -- Stored only, no effect
    }
})
```

## Theme Defaults

### Colors (`theme.colors.*`)
Not implemented - every key below is seeded and hex-color-validated
(Validation Rules, below) but never read anywhere past that seed call:
confirmed by grepping the whole compositor for a second reference to
each `theme.colors.*` key beyond `crates/config/src/engine/support.rs`'s
own seed/validate calls, none found. `theme.decorations.*` below (border
colors, titlebar colors) is the real, working theme surface - every
color a window actually renders comes from there, not here.
```lua
srd.theme.set_colors({
    background = "#2e3440",                             -- Stored/validated only, no effect
    foreground = "#eceff4",                             -- Stored/validated only, no effect
    primary = "#88c0d0",                                -- Stored/validated only, no effect
    secondary = "#81a1c1",                              -- Stored/validated only, no effect
    accent = "#5e81ac",                                 -- Stored/validated only, no effect
    error = "#bf616a",                                  -- Stored/validated only, no effect
    warning = "#ebcb8b",                                -- Stored/validated only, no effect
    success = "#a3be8c"                                 -- Stored/validated only, no effect
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
        focused_style = "solid",                        -- Stored only, no effect - solid is the only style this compositor ever draws
        unfocused_style = "solid"                       -- Stored only, no effect - solid is the only style this compositor ever draws
    },
    title_bar = {
        height = 32,                                    -- Not actually read - see the note below the table
        show = true,                                    -- Stored only, no effect - see `theme.decorations.default_mode`/per-window `decorated` rule action instead
        font = "JetBrains Mono 10",                     -- Stored only, no effect - the titlebar always uses whatever system font `find_system_font` picks
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
srd.set("theme.decorations.title_bar.button_mode", "dynamic")  -- Default: "dynamic"
```

`button_mode` sets `"dynamic"` (the default) to show only the buttons a
window can actually use, or `"fixed"` to always show the full set. Today
dynamic mode has one rule: a window whose client pinned its minimum and
maximum size to the same value gets no Maximize button, because pressing it
can do nothing. GNOME, KDE and Windows all hide or disable maximize in the
same case. A dialog's Close-only titlebar is a separate rule and applies in
both modes. Live-settable with `srd set button_mode <dynamic|fixed>`, and
readable back from `srd settings`.

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

### Lock Screen (`theme.lock.*`)

srdwm's own built-in session-lock UI (`srd dispatch lock`), not a config
table - each key is a flat `srd.set` value, read once when the lock
engages:

```lua
srd.set("theme.lock.box_bg", "#2e3440")             -- Default: Nord dark
srd.set("theme.lock.box_border", "#88c0d0")         -- Default: Nord blue
srd.set("theme.lock.text_color", "#eceff4")         -- Default: Nord light
srd.set("theme.lock.error_color", "#bf616a")        -- Default: Nord red
srd.set("theme.lock.corner_radius", 10)             -- Default: 10
srd.set("theme.lock.blur_radius", 20)               -- Default: 20 (0 disables blur)
srd.set("theme.lock.dot_char", "\u{25cf}")          -- Default: "\u{25cf}" (a filled circle)
srd.set("theme.lock.show_caps_lock", true)          -- Default: true
srd.set("theme.lock.show_failed_attempts", true)    -- Default: true
srd.set("theme.lock.fail_message", "Wrong password") - Default: "Wrong password"
srd.set("theme.lock.show_clock", true)              -- Default: true
srd.set("theme.lock.show_keyboard", true)           -- Default: true
srd.set("theme.lock.avatar_bg", "#88c0d0")          -- Default: Nord blue, matches box_border
```

`show_clock` adds a large time/date readout above the password box, plus a
circular avatar (the username's first letter) and the username itself --
the set of things a mainstream lock screen (GNOME, macOS, Windows) shows.
Setting it to `false` reduces the lock screen to just the password box.

`show_keyboard` adds an on-screen keyboard below the password box, for a
session with no physical keyboard reachable (a touchscreen device). A real
physical keyboard still works identically either way; this only adds a
second input method, it never replaces the first. Tapping `Shift` toggles
between the lowercase and uppercase/symbol rows.

`avatar_bg` sets the avatar circle's fill colour. It defaults to the same
value as `box_border` rather than a third independent colour to track.

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
- `aspect_ratio` (string, `"W:H"`, positive integers) - holds this ratio
  while the window is floating and being interactively resized, deriving
  whichever dimension the user isn't actively dragging from the one they
  are. No effect on a tiled window. The "phone monitor" primitive: tag any
  VM/emulator/`scrcpy` window by `class` and it keeps a phone-shaped frame
  through a resize - this is a plain window-rule action, not anything
  Android- or VM-specific.

```lua
srd.rule({ class = "pavucontrol" }, { floating = true })
srd.rule({ title = "Picture-in-Picture" }, { floating = true, width = 480, height = 270 })
srd.rule({ class = "scrcpy" }, { floating = true, aspect_ratio = "9:16" })
```

## Platform-Specific Defaults

Only `platform.backend`/`platform.os` are real here - both are
read-only, informational values `crates/srdwm/src/main.rs` overwrites
with the actually-detected backend/OS at startup (`platform.backend`:
`"wayland"`/`"x11"`/`"windows"`/`"macos"`; not the string `"auto"` a
config never actually sets it to), so `init.lua` can branch on which
platform it's running under (`if srd.get("platform.backend") ==
"wayland" then ... end`). Every `use_*`/`global_hooks`/
`accessibility_enabled` flag below is seeded but never read anywhere
past that seed call - EWMH/NetWM/xdg-shell/layer-shell/DWM/Win32/
Cocoa/Core Graphics support is simply always compiled in and used
unconditionally on the relevant platform; none of it is behind a
toggle.

### Linux (X11/Wayland)
```lua
srd.get("platform.backend")                             -- Read-only: "wayland" or "x11", whichever actually connected

- X11 specific
srd.set("platform.x11.use_ewmh", true)                 -- Stored only, no effect
srd.set("platform.x11.use_netwm", true)                -- Stored only, no effect

- Wayland specific
srd.set("platform.wayland.use_xdg_shell", true)        -- Stored only, no effect
srd.set("platform.wayland.use_layer_shell", true)      -- Stored only, no effect
```

### Windows
```lua
- Windows specific
srd.set("platform.windows.use_dwm", true)              -- Stored only, no effect
srd.set("platform.windows.use_win32", true)            -- Stored only, no effect
srd.set("platform.windows.global_hooks", true)         -- Stored only, no effect
```

### macOS
```lua
- macOS specific
srd.set("platform.macos.use_cocoa", true)              -- Stored only, no effect
srd.set("platform.macos.use_core_graphics", true)      -- Stored only, no effect
srd.set("platform.macos.accessibility_enabled", true)  -- Stored only, no effect
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

The real, currently-implemented set (checked directly against every
`std::env::var` call in the compositor's own source, not aspirational --
`SRDWM_THEME`/`SRDWM_DEBUG_LEVEL`/`SRDWM_PLATFORM`/`SRDWM_MAX_FPS`/
`SRDWM_VSYNC`, previously listed here, do not exist in the codebase at
all):

```bash
# Configuration directory override - falls back to
# $XDG_CONFIG_HOME/srd, then ~/.config/srd
export SRDWM_CONFIG_PATH="/path/to/config"

# Monitor-layout state file override - falls back to
# $XDG_STATE_HOME/srd/monitor-layout.json, then
# ~/.local/state/srd/monitor-layout.json
export SRDWM_STATE_PATH="/path/to/monitor-layout.json"

# Opt into the udev backend's real GBM+EGL GPU render path - the
# lower-level override alongside general.gpu in config (see that
# key's own section above); either being set is enough
export SRDWM_GPU=1

# Log level/filter - standard env_logger-style directive syntax
# (e.g. "srdwm=debug,warn"), read by srdwm's own logger setup
export RUST_LOG="info"
```

Two more are read, but are standard desktop-environment conventions
rather than srdwm-specific settings, so setting them affects any
Wayland/X11 app, not just this compositor: `XCURSOR_THEME`/
`XCURSOR_SIZE` (the built-in cursor's theme/size), and `DISPLAY`/
`WAYLAND_DISPLAY` (which existing session `srd` itself connects to).

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
Enforced by `srd.validate_config()` against a Lua config's own declared
values (`init.lua`, `srd.set(...)`) - the live `srd set <key> <value>`
CLI/IPC command does **not** currently run these same checks, only that
the value parses as the right type (a non-negative integer for
`border_width`/`corner_radius`, say). A value outside the ranges below
set live still applies exactly as given, unvalidated.
- **Gaps** (`general.window_gap`, every layout's `gaps.inner`/`gaps.outer`): 0-100 pixels
- **Border width** (`theme.decorations.border.width`): 0-20 pixels
- **Corner radius** (`theme.decorations.border.radius`): 0-100 pixels
- **Resize margin** (`general.resize_margin`): 1-50 pixels
- **Animation duration**: 0-1000ms
- **FPS**: 30-240
- **Cache size**: 10-10000
- **Border inactive-window dim** (`theme.decorations.border.inactive_dim`): 0.0-1.0

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



## Config reloading and what happens when a config breaks

A Lua config is a program, so breaking it is an ordinary event rather than
an exceptional one. Three things make that safe.

**A failed reload changes nothing.** Reloading clears the keybinding, event
handler and repeat-key tables before re-running `init.lua`, so that a
binding deleted from the file really disappears. If the new file fails to
parse or errors while running, all three tables are put back exactly as they
were. The last working config keeps running. Whatever the broken run managed
to register before it failed is discarded rather than merged, because half a
config is not a config.

**The error is shown, not just logged.** Config failures go to
`notify-send` as well as the log. Without that the failure is close to
silent from the user's side: the compositor keeps running and the edit simply
does nothing.

**Edits apply on save.** `general.config_reload_on_write` (default `true`)
checks the config directory's `.lua` modification times once a second and
reloads when one changes. Set it to `false` for a config that does expensive
work at load time. `Mod4+Ctrl+r` still reloads on demand in either case.

**A hand-made change is not undone by a reload.** A reload rebuilds the
theme and general settings from the config file, which is right for a file
edit but would also wipe every live `srd set` - and the titlebar
right-click menu's "Customize" rows are all live `srd set`s. Every setting
changed live is recorded and re-applied after each reload, so changing a
button style from that menu and then saving `init.lua` for an unrelated
reason keeps the change. A live value stays until it is changed again or the
session ends; it is a session override, not a persisted setting, so write it
into the config to keep it across restarts.

One limit to know: a reload does not re-register key *grabs* with the
backend, so a brand new key combination needs a restart before the
compositor sees that key at all. An existing combination picks up its new
action immediately.
