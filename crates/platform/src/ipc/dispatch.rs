//! Request dispatch: `handle_request` (every query/dispatch `cmd`) and
//! `handle_set` (the `"set"` cmd's own key/value sub-dispatch). Split out
//! of the original single `ipc.rs` purely by concern, no behavior change.

use srdwm_core::{WindowId, WindowManager};

use super::types::*;

/// Parses and applies one request line, returning the response body (no
/// trailing newline) and whether it changed window state.
pub(crate) fn handle_request(line: &[u8], wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> (Vec<u8>, bool) {
    let Ok(req) = serde_json::from_slice::<serde_json::Value>(line) else {
        return (err("invalid request"), false);
    };
    let cmd = req.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    let id = req.get("id").and_then(|v| v.as_u64()).map(|v| v as WindowId);

    match cmd {
        "clients" => (serde_json::to_vec(&ClientsResponse { clients: client_snapshot(wm) }).unwrap_or_default(), false),
        "workspaces" => (serde_json::to_vec(&WorkspacesResponse { workspaces: workspace_snapshot(wm) }).unwrap_or_default(), false),
        "settings" => {
            let wm = wm.borrow();
            let settings = SettingsResponse {
                shadows: wm.shadows_enabled,
                rounded_corners: wm.rounded_corners_enabled,
                animations: wm.animations_enabled,
                night_light: wm.color_filter == srdwm_core::ColorFilter::NightLight,
                reading_mode: wm.color_filter == srdwm_core::ColorFilter::ReadingMode,
                phone_mode: wm.phone_mode,
                multi_cursor: wm.multi_cursor_enabled,
            };
            (serde_json::to_vec(&settings).unwrap_or_default(), false)
        }
        "monitors" => (serde_json::to_vec(&MonitorsResponse { monitors: monitor_snapshot(wm) }).unwrap_or_default(), false),
        // The connection is handed off to `IpcServer::subscribers` by the
        // caller (`poll`, which is the only place that can see the raw
        // `cmd` string this deep call already consumed) right after this
        // reply is written - this arm only has to produce that reply, in
        // the same `ClientsEvent` shape every later push uses.
        "subscribe" => {
            // Four JSON objects, not one: `poll` writes this response plus
            // one trailing `\n` verbatim, so an embedded `\n` between each
            // here is all it takes to hand a fresh subscriber every initial
            // snapshot as its own line - exactly the shape every later
            // push already uses, so there's nothing for a consumer to
            // special-case about the first few lines it reads.
            let clients = client_snapshot(wm);
            let workspaces = workspace_snapshot(wm);
            let layout = wm.borrow().keyboard_layout.clone();
            let monitors = monitor_snapshot(wm);
            let mut out = serde_json::to_vec(&ClientsEvent { event: "clients", clients: &clients }).unwrap_or_default();
            out.push(b'\n');
            out.extend(serde_json::to_vec(&WorkspacesEvent { event: "workspaces", workspaces: &workspaces }).unwrap_or_default());
            out.push(b'\n');
            out.extend(serde_json::to_vec(&KeyboardLayoutEvent { event: "keyboard_layout", layout: &layout }).unwrap_or_default());
            out.push(b'\n');
            out.extend(serde_json::to_vec(&MonitorsEvent { event: "monitors", monitors: &monitors }).unwrap_or_default());
            (out, false)
        }
        "keyboard_layout" => {
            (serde_json::to_vec(&KeyboardLayoutResponse { layout: wm.borrow().keyboard_layout.clone() }).unwrap_or_default(), false)
        }
        "cycle_keyboard_layout" => {
            wm.borrow_mut().request_keyboard_layout_cycle();
            (ok(), true)
        }
        "toggle_visibility" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let mut wm = wm.borrow_mut();
            let current = wm.current_workspace();
            let Some(w) = wm.windows().find(|w| w.id == id) else {
                return (err("no such window"), false);
            };
            let now_hidden = w.minimized || w.workspace != current;
            if now_hidden {
                // Follows the caller to whichever workspace is current --
                // matches Hyprland's `special:scratchpad`/Sway's `scratchpad
                // show`, which is the behaviour the `scratchpad` script and
                // its keybindings are written against.
                wm.move_window_to_workspace(id, current);
                wm.restore_window(id);
                wm.focus_window(id);
            } else {
                wm.minimize_window(id);
            }
            (ok(), true)
        }
        "focus" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().focus_window(id);
            (ok(), true)
        }
        "close" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().close_window(id);
            (ok(), true)
        }
        // `srd dispatch lock` - no id, there's only ever one session to
        // lock. Core cannot lock the screen itself (real rendering/input-
        // routing, backend-owned); this just queues the request the same
        // way `set_output_position` queues one for whichever backend owns
        // real output hardware - see `WindowManager::request_lock`'s own
        // doc comment.
        "lock" => {
            wm.borrow_mut().request_lock();
            (ok(), true)
        }
        // `{"cmd":"pin_input","pid":<client pid>,"id":<window id>}` pins
        // every `zwlr_virtual_pointer_unstable_v1` object that client owns
        // to window `id` - Phase 2 of the multi-cursor plan (`docs/
        // TODO.md`), the primitive an agent-controlling tool needs to
        // operate one specific window without moving the human's own
        // cursor or stealing focus. Omitting `id` unpins instead (`{"cmd":
        // "pin_input","pid":<pid>}`) - one command for both directions,
        // since "pin" and "unpin" are really just "set the pin to Some or
        // None", the same shape `set_output_enabled`'s own boolean already
        // uses for two related actions on one dispatch. Keyed by pid, not
        // an opaque per-object id nothing outside the Wayland backend
        // could ever learn - a controlling tool already knows its own
        // pid (`std::process::id()`) for free. Queued via `request_pin_
        // input` and applied by the Wayland backend on its own next poll,
        // same one-poll-tick latency `set_output_position` already has.
        "pin_input" => {
            let Some(pid) = req.get("pid").and_then(|v| v.as_i64()) else {
                return (err("missing pid"), false);
            };
            wm.borrow_mut().request_pin_input(pid as i32, id);
            (ok(), true)
        }
        // `{"cmd":"create_fake_monitor","name":<string>,"width":<u32>,
        // "height":<u32>}` - a fully virtual `wl_output` with no real
        // hardware behind it, applied by whichever backend owns real
        // output hardware (only the udev backend can; the winit/nested
        // backend has no headless render path to draw one with). See
        // `crates/wayland/src/udev/virtual_heads.rs`'s own module doc
        // comment for the full design and scope.
        "create_fake_monitor" => {
            let Some(name) = req.get("name").and_then(|v| v.as_str()) else { return (err("missing name"), false) };
            let (Some(width), Some(height)) = (req.get("width").and_then(|v| v.as_u64()), req.get("height").and_then(|v| v.as_u64())) else {
                return (err("missing width/height"), false);
            };
            wm.borrow_mut().request_create_fake_monitor(name.to_string(), width as u32, height as u32);
            (ok(), true)
        }
        // `{"cmd":"remove_fake_monitor","name":<string>}`.
        "remove_fake_monitor" => {
            let Some(name) = req.get("name").and_then(|v| v.as_str()) else { return (err("missing name"), false) };
            wm.borrow_mut().request_remove_fake_monitor(name.to_string());
            (ok(), true)
        }
        // `srd.window.maximize()`/`.fullscreen()`'s exact IPC-side
        // equivalents - lets an external script (or a live diagnostic
        // check, same as `toggle_visibility`/`focus`/`close` already allow)
        // drive either without needing a keybinding to already exist.
        "toggle_maximize" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_maximize(id);
            (ok(), true)
        }
        "toggle_fullscreen" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_fullscreen(id);
            (ok(), true)
        }
        // The four general compositor operations that have no standard
        // Wayland protocol to fall back on - confirmed with the AGS peer
        // session that `zwlr_foreign_toplevel_manager_v1` already covers
        // activate/close/maximize/minimize/fullscreen (so those stay
        // protocol-only, no bespoke verb here), but nothing in that
        // protocol or `ext-workspace-v1` can toggle floating, pin a
        // window, move one within the tiling order, or move one to a
        // specific workspace. Designed as plain general operations any
        // client can use (a panel, a script, a keybinding daemon), not
        // shaped around one particular shell's own IPC habits.
        "toggle_floating" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_floating(id);
            (ok(), true)
        }
        "toggle_pinned" => {
            let Some(id) = id else { return (err("missing id"), false) };
            wm.borrow_mut().toggle_always_on_top(id);
            (ok(), true)
        }
        // `{"cmd":"move_window","id":<window id>,"direction":"left"|"right"|"up"|"down"}`
        // - `WindowManager::move_window_direction` swaps the *focused*
        // window with its neighbour in that direction, so a caller asking
        // to move a window that isn't currently focused needs it focused
        // first; matches `movewindow` needing the target window active in
        // every tiling WM this gesture is modeled on.
        "move_window" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let Some(dir) = req.get("direction").and_then(|v| v.as_str()).and_then(parse_direction) else {
                return (err("direction must be one of: left, right, up, down"), false);
            };
            let mut wm = wm.borrow_mut();
            if wm.focused_id() != Some(id) {
                wm.focus_window(id);
            }
            wm.move_window_direction(dir);
            (ok(), true)
        }
        // `{"cmd":"move_to_workspace","id":<window id>,"workspace":<workspace id>}`
        // - the operation the AGS peer's Overview needs for drag-a-window-
        // onto-another-workspace, which `ext-workspace-v1` (activation
        // only, no toplevel-to-workspace verb) and `zwlr-foreign-toplevel`
        // (no workspace concept at all) both lack entirely.
        "move_to_workspace" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let Some(workspace) = req.get("workspace").and_then(|v| v.as_u64()) else {
                return (err("missing workspace"), false);
            };
            wm.borrow_mut().move_window_to_workspace(id, workspace as srdwm_core::WorkspaceId);
            (ok(), true)
        }
        // The workspace-side equivalent of `focus`: `id` here is a
        // `WorkspaceId`, not a `WindowId` - both are plain `usize`/`u64`
        // on the wire, so the same generic `id` field this whole match
        // already reads serves both, same as every other dispatch arm.
        "activate_workspace" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let before = wm.borrow().current_workspace();
            // `switch_workspace_on_monitor` falls straight through to the
            // ordinary shared-mode `switch_workspace` when `workspace.
            // per_monitor` is off, so this is the one call site that works
            // correctly either way - no branching on the config flag
            // needed here. The monitor it applies to in per-monitor mode:
            // the focused window's own monitor, falling back to the
            // primary monitor if nothing is focused (an empty desktop) --
            // the same "whichever output a keybinding should apply to"
            // choice real per-output-aware WMs (Hyprland, niri) make.
            {
                let mut wm = wm.borrow_mut();
                let monitor = wm
                    .focused_id()
                    .and_then(|f| wm.window(f))
                    .map(|w| w.monitor)
                    .or_else(|| wm.primary_monitor().map(|m| m.id))
                    .unwrap_or(0);
                wm.switch_workspace_on_monitor(id as srdwm_core::WorkspaceId, monitor);
            }
            let after = wm.borrow().current_workspace();
            let known: Vec<_> = wm.borrow().workspaces().iter().map(|w| w.id).collect();
            log::warn!("WS-IPC-DIAG requested_id={id} before={before} after={after} known_ids={known:?}");
            (ok(), true)
        }
        // `{"cmd":"set_output_position","id":<monitor id>,"x":<i32>,"y":<i32>}`
        // - the primitive an output-configuration UI (a display-settings
        // panel, concretely the monitor-mirroring toggle this was built
        // for) needs and had no way to reach before: `wlr-output-
        // management-v1` already supports repositioning an output
        // (`crates/wayland/src/output_management.rs`), but only to a
        // client willing to implement that whole protocol itself just to
        // move one output. This exposes the same capability over the
        // plain IPC socket every other `srd dispatch` action already
        // uses. Deliberately just "move this output" with no separate
        // "mirror" concept anywhere: positioning two outputs at the same
        // coordinates already shows the same desktop region on both (every
        // window/render decision downstream works in shared global space,
        // not per-output), so mirroring is something a caller *achieves*
        // with this primitive, not something srdwm needs to know about as
        // its own state.
        //
        // Not applied here, and deliberately not a `WindowId` on the wire
        // despite reusing the same `id` field every other dispatch already
        // reads (both are plain integers on the wire; only the Rust-side
        // type differs) - this crate has no real output handle to move,
        // only `WindowManager`'s passive mirror of whatever the backend
        // last reported. Queued via `request_output_position` and applied
        // by whichever backend actually owns the hardware on its own next
        // poll, the same one-poll-tick latency every other backend-owned
        // effect in this IPC layer already has (a redraw, a geometry
        // change) - `changed = true` still makes sense to return since
        // this genuinely will change what's on screen once the backend
        // catches up, just not synchronously within this call.
        "set_output_position" => {
            // Accepts a monitor `name` as well as the plain `id` every
            // other dispatch already reads - `srd monitors`/`wlr-output-
            // management-v1` both key on name first (`eDP-1`,
            // not an arbitrary index), and a display-arrangement UI
            // reasonably lists outputs by that name rather than making a
            // caller look its own id up first just to turn around and send
            // it straight back. `id` still wins if both are somehow given.
            let Some(monitor_id) = resolve_monitor_id(wm, id, req.get("name").and_then(|v| v.as_str())) else {
                return (err("missing id or a name matching a connected monitor"), false);
            };
            let (Some(x), Some(y)) = (req.get("x").and_then(|v| v.as_i64()), req.get("y").and_then(|v| v.as_i64())) else {
                return (err("missing x/y"), false);
            };
            wm.borrow_mut().request_output_position(monitor_id, x as i32, y as i32);
            (ok(), true)
        }
        // `{"cmd":"set_output_enabled","id"|"name":...,"enabled":<bool>}`
        // - "primary only"/a per-display toggle, the two AGS monitor-
        // layout panel rows gated pending this. Disabling and re-enabling
        // reuse this backend's own existing hotplug-removal/bring-up code
        // paths rather than a new mechanism (see the udev platform's own
        // drain site) - the same real, already-tested steps a genuine
        // unplug/replug already goes through, just triggered
        // administratively instead of by a real DRM event.
        //
        // Resolved to a *name* here, unlike `set_output_position` (which
        // stays on `resolve_monitor_id`/plain `MonitorId`) - see
        // `WindowManager::request_output_enabled`'s own doc comment for
        // why: disabling removes the output from `monitors()` entirely, so
        // its id has nothing left to mean by the time a caller wants to
        // *re-enable* it. `id` is still accepted, resolved against the
        // live list the same way `resolve_monitor_id` does, but that only
        // ever works for the disable direction (the output is still live
        // when you ask to turn it off) - re-enabling a currently-disabled
        // output needs its `name` given directly, since no live entry
        // exists to resolve an `id` against at that point.
        "set_output_enabled" => {
            let name = match req.get("name").and_then(|v| v.as_str()) {
                Some(name) => Some(name.to_string()),
                None => id.and_then(|id| wm.borrow().monitors().iter().find(|m| m.id == id as srdwm_core::MonitorId).map(|m| m.name.clone())),
            };
            let Some(name) = name else { return (err("missing name, or an id matching a currently-connected monitor"), false) };
            let Some(enabled) = req.get("enabled").and_then(|v| v.as_bool()) else {
                return (err("missing enabled"), false);
            };
            wm.borrow_mut().request_output_enabled(name, enabled);
            (ok(), true)
        }
        // `{"cmd":"capture_workspace","id":<workspace id>,"path":<string>,
        // "width":<u32>,"height":<u32>}` - `width`/`height` are optional,
        // both or neither. Exists for a workspace switcher's thumbnail
        // previews (AGS's Overview): `wlr-screencopy` - what `grim` and
        // this compositor's own `screencopy.rs` both use - can only ever
        // capture what an output is currently *presenting*, so a workspace
        // that isn't the active one is structurally invisible to it. This
        // is the one thing screencopy can't do, queued the same
        // cross-boundary way `set_output_position` is (core has no
        // renderer of its own) and drained by whichever backend is
        // actually running on its own next poll. Same one-poll-tick
        // latency as every other backend-owned effect this IPC layer
        // already has - the file exists shortly after this call returns,
        // not necessarily before it.
        "capture_workspace" => {
            let Some(id) = id else { return (err("missing id"), false) };
            let Some(path) = req.get("path").and_then(|v| v.as_str()) else {
                return (err("missing path"), false);
            };
            let size = match (req.get("width").and_then(|v| v.as_u64()), req.get("height").and_then(|v| v.as_u64())) {
                (Some(w), Some(h)) => Some((w as u32, h as u32)),
                (None, None) => None,
                _ => return (err("width and height must both be given, or neither"), false),
            };
            wm.borrow_mut().request_capture_workspace(id as srdwm_core::WorkspaceId, path.to_string(), size);
            (ok(), true)
        }
        // Live theme values - an AGS peer session's equivalent of
        // Hyprland's `hyprctl keyword general:col.active_border ...`, the
        // mechanism their shell already uses to repaint window borders the
        // instant an accent palette/radius/etc changes in Settings. Was a
        // real, invisible gap before this: every one of these already had
        // a real, mutable `WindowManager` field (`theme.default_border_*`,
        // `tiling.gap_*`, `shadows_enabled`, `rounded_corners_enabled`),
        // set once from Lua config at startup and never touched again --
        // so a running session had no way to change any of it without a
        // full restart, unlike everything else `srd dispatch` already
        // covers live.
        //
        // No extra redraw call needed here: returning `changed = true`
        // (same as every other mutating command) is exactly what makes
        // `main.rs`'s `sync()` run its next tick, which already calls
        // `redraw_decoration`/`apply_geometry` for every visible window
        // unconditionally - this only has to mutate the right field and
        // let that existing machinery do the rest.
        "set" => handle_set(&req, wm),
        _ => (err("unknown command"), false),
    }
}

/// `{"cmd":"set","key":"border_width","value":3}` and the rest of `"set"`'s
/// keys - pulled out of `handle_request`'s match arm purely to keep that
/// match's per-arm bodies roughly the same size; no reuse motive.
///
/// A window's `border_color`/`border_width` are copied from `theme.
/// default_border_color`/`default_border_width` once, at creation
/// (`WindowManager::add_window`), and a rule's explicit `border_color`/
/// `border_width` action can overwrite that afterward - so a window
/// carrying the *old* default is, in practice, exactly the set of windows
/// that never had a rule override it (a rule-set colour coincidentally
/// equal to today's default is the only false positive, and updating it
/// to the new default too is a reasonable outcome, not a real bug). That
/// predicate is what the two colour/width arms below walk existing
/// windows with, rather than touching every window unconditionally.
fn handle_set(req: &serde_json::Value, wm: &std::rc::Rc<std::cell::RefCell<WindowManager>>) -> (Vec<u8>, bool) {
    let key = req.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = req.get("value");
    match key {
        "border_width" => {
            let Some(width) = value.and_then(|v| v.as_u64()) else { return (err("border_width needs a numeric value"), false) };
            let width = width as u32;
            let mut wm = wm.borrow_mut();
            let old = wm.theme.default_border_width;
            wm.theme.default_border_width = width;
            let matching: Vec<_> = wm.windows().filter(|w| w.border_width == old).map(|w| w.id).collect();
            for id in matching {
                if let Some(w) = wm.window_mut(id) {
                    w.border_width = width;
                }
            }
            (ok(), true)
        }
        "border_color" => {
            let Some(hex) = value.and_then(|v| v.as_str()) else { return (err("border_color needs a hex string value"), false) };
            let Some(rgb) = srdwm_core::parse_hex_color(hex) else { return (err("border_color must be a hex string like #cba6f7"), false) };
            let mut wm = wm.borrow_mut();
            let old = wm.theme.default_border_color;
            wm.theme.default_border_color = rgb;
            let matching: Vec<_> = wm.windows().filter(|w| w.border_color == old).map(|w| w.id).collect();
            for id in matching {
                if let Some(w) = wm.window_mut(id) {
                    w.border_color = rgb;
                }
            }
            (ok(), true)
        }
        // `border_width`'s exact twin, for the titlebar/border-strip corner
        // radius - same "only touch windows still carrying the old
        // default" predicate, so a window a rule already gave its own
        // explicit `corner_radius` isn't silently overwritten by a later
        // live-set.
        "corner_radius" => {
            let Some(radius) = value.and_then(|v| v.as_u64()) else { return (err("corner_radius needs a numeric value"), false) };
            let radius = radius as u32;
            let mut wm = wm.borrow_mut();
            let old = wm.theme.default_corner_radius;
            wm.theme.default_corner_radius = radius;
            let matching: Vec<_> = wm.windows().filter(|w| w.corner_radius == old).map(|w| w.id).collect();
            for id in matching {
                if let Some(w) = wm.window_mut(id) {
                    w.corner_radius = radius;
                }
            }
            (ok(), true)
        }
        // Live A/B-testing knob for `srdwm_core::ThemeConfig::
        // default_decorated` - see its own doc comment for the "which
        // desktop environment does what" reasoning behind making this
        // configurable at all. Deliberately only affects windows created
        // *after* this call, not existing ones - retroactively flipping
        // an already-mapped window's decoration needs the same redraw-buffer
        // + geometry-resync `set_decorated_from_mode` does on the Wayland
        // side (backend-specific, unreachable from this backend-agnostic
        // `crates/platform` code), and the actual use case here is testing
        // which default a freshly opened app gets, not live-migrating
        // windows already on screen.
        "decoration_mode" => {
            let Some(mode) = value.and_then(|v| v.as_str().map(str::to_string)) else {
                return (err("decoration_mode needs \"server\" or \"client\""), false);
            };
            if mode != "server" && mode != "client" {
                return (err("decoration_mode needs \"server\" or \"client\""), false);
            }
            wm.borrow_mut().theme.default_decorated = mode != "client";
            (ok(), true)
        }
        // Tiling-only: `arrange_workspace` skips floating/fullscreen
        // windows regardless, and under `"dynamic"` (the no-op default
        // layout) nothing reads `tiling.gap_*` at all - so setting these
        // is a correct no-op, visually, until a workspace actually runs
        // the `"tiling"` layout, exactly matching what Hyprland's own
        // `general:gaps_*` do under its own non-tiling/floating windows.
        "gap_inner" => {
            let Some(v) = value.and_then(|v| v.as_u64()) else { return (err("gap_inner needs a numeric value"), false) };
            wm.borrow_mut().tiling.gap_inner = v as u32;
            (ok(), true)
        }
        "gap_outer" => {
            let Some(v) = value.and_then(|v| v.as_u64()) else { return (err("gap_outer needs a numeric value"), false) };
            wm.borrow_mut().tiling.gap_outer = v as u32;
            (ok(), true)
        }
        "shadows" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("shadows needs a boolean value"), false) };
            wm.borrow_mut().shadows_enabled = v;
            (ok(), true)
        }
        // A bool, not a radius: the actual corner radius is a fixed
        // constant (`crates/wayland/src/decoration.rs::CORNER_RADIUS`),
        // not a per-session config value anywhere in the compositor yet --
        // this can only turn rounding on/off, matching `WindowManager::
        // rounded_corners_enabled`'s existing `Option<bool>` shape (also
        // config-settable at startup via `general.rounded_corners`, never
        // live until now). A live-settable numeric radius is real, separate
        // future work, not something to fake here with a value that's
        // silently ignored.
        "rounded_corners" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("rounded_corners needs a boolean value"), false) };
            wm.borrow_mut().rounded_corners_enabled = Some(v);
            (ok(), true)
        }
        // Same shape as `shadows` - `WindowManager::animations_enabled`
        // already existed (config-settable at startup via `general.
        // animations`) but had no live IPC toggle, unlike shadows/rounded
        // corners which did. Added specifically so a performance-profile
        // script (ported from a Hyprland one that used `hyprctl keyword
        // animations:enabled`) has something real to call instead of
        // silently no-op-ing under srdwm.
        "animations" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("animations needs a boolean value"), false) };
            wm.borrow_mut().animations_enabled = v;
            (ok(), true)
        }
        // `srd set phone_mode <bool>` - live equivalent of `general.
        // phone_mode`, same "config-settable at startup, also live via
        // `srd set`" shape as `animations`/`shadows`/`rounded_corners`
        // just above. Only ever changes how the *next* new window opens
        // (`WindowManager::add_window`'s own use of this) - `changed`
        // is still `true` since a subscriber (a shell panel adapting its
        // own chrome to this same signal) genuinely has something new to
        // read from `srd settings`, even though no *window* moves as a
        // direct result of this call alone.
        "phone_mode" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("phone_mode needs a boolean value"), false) };
            wm.borrow_mut().phone_mode = v;
            (ok(), true)
        }
        // `srd set multi_cursor <bool>` - live equivalent of `general.
        // multi_cursor`. See `WindowManager::multi_cursor_enabled`'s own
        // doc comment for why this is opt-in rather than always-on.
        "multi_cursor" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("multi_cursor needs a boolean value"), false) };
            wm.borrow_mut().multi_cursor_enabled = v;
            (ok(), true)
        }
        "blur" => (err("blur is not supported - no GPU shader path on this compositor's software renderer yet"), false),
        // The two ported Hyprland `decoration:screen_shader` scripts --
        // mutually exclusive by construction (`srdwm_core::ColorFilter` is
        // one enum, not two bools), matching the original scripts' own
        // "both point at the same single shader slot" behaviour: setting
        // either key `true` clears the other, and `false` always clears to
        // `None` regardless of which one (if any) was actually active.
        "night_light" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("night_light needs a boolean value"), false) };
            wm.borrow_mut().color_filter = if v { srdwm_core::ColorFilter::NightLight } else { srdwm_core::ColorFilter::None };
            (ok(), true)
        }
        "reading_mode" => {
            let Some(v) = value.and_then(|v| v.as_bool()) else { return (err("reading_mode needs a boolean value"), false) };
            wm.borrow_mut().color_filter = if v { srdwm_core::ColorFilter::ReadingMode } else { srdwm_core::ColorFilter::None };
            (ok(), true)
        }
        _ => (err("unknown set key"), false),
    }
}

