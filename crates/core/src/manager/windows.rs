//! Window registration/removal, lookup, and stacking-order operations (raise, lower, pin).
//! Split out of the original single `manager.rs` - see `super` (`mod.rs`) for
//! `WindowManager`'s field definitions; everything here is plain `impl WindowManager`
//! methods, unchanged from before the split.

use super::*;

impl WindowManager {
    // ---- Windows -------------------------------------------------------

    pub fn alloc_window_id(&mut self) -> WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    /// Registers a window that a backend has already created. If the current
    /// workspace's layout doesn't auto-tile ("dynamic"/"floating"), the
    /// window's initial geometry is chosen via [`SmartPlacement`]; otherwise
    /// it's left for the next `arrange_workspace` call to place.
    pub fn add_window(&mut self, mut window: Window) -> WindowId {
        let id = window.id;
        // Applied before rule matching below, which still wins when a rule
        // sets its own `border_color`/`border_width` - this only replaces
        // whatever a backend's `Window::new` happened to hardcode.
        window.border_color = self.theme.default_border_color;
        window.border_width = self.theme.default_border_width;
        window.corner_radius = self.theme.default_corner_radius;
        window.decorated = self.theme.default_decorated && !likely_draws_own_titlebar(&window.app_id);
        let actions = self.rules.iter().find(|r| r.matcher.matches(&window)).map(|r| r.actions.clone());
        log::warn!(
            "DECO-DIAG add_window id={id} app_id={:?} title={:?} rules_count={} actions_found={} decorated_before_actions={}",
            window.app_id,
            window.title,
            self.rules.len(),
            actions.is_some(),
            window.decorated
        );
        // See `Window::rules_applied`'s doc comment: a native Wayland window
        // still has empty title/app_id at this point, so a real (if
        // inconclusive) match attempt needs to wait for `reapply_rules_if_pending`.
        window.rules_applied = actions.is_some() || !(window.title.is_empty() && window.app_id.is_empty());

        let workspace = actions.as_ref().and_then(|a| a.workspace).unwrap_or(self.current_workspace);
        window.workspace = workspace;
        if let Some(a) = &actions {
            if let Some(floating) = a.floating {
                window.floating = floating;
            }
            if let Some(decorated) = a.decorated {
                window.decorated = decorated;
            }
            if let Some(color) = a.border_color {
                window.border_color = color;
            }
            if let Some(width) = a.border_width {
                window.border_width = width;
            }
            if let Some(radius) = a.corner_radius {
                window.corner_radius = radius;
            }
            if let Some(pinned) = a.pinned {
                window.always_on_top = pinned;
            }
            if let Some(opacity) = a.opacity {
                window.opacity = opacity.clamp(0.0, 1.0);
            }
            if let Some(margin) = a.resize_margin {
                window.resize_margin = Some(margin);
            }
            if let Some(ratio) = a.aspect_ratio {
                window.aspect_ratio = Some(ratio);
            }
        }

        // A remembered size (`remembered_geometry`'s own doc comment) wins
        // over whatever fixed default a backend hardcoded into `window.
        // geometry` before calling this - but a rule's explicit `geometry`
        // action below still wins over *this*, since that's a deliberate
        // per-app override, more specific than "whatever I last left this
        // app at". Clamped to the same minimums a live resize itself can
        // never go below, so a corrupted/stale entry can't hand a new
        // window a degenerate size. Position (see `remembered_position`
        // below, computed after `target_monitor` exists) is a separate
        // question from size: a size is always safe to reapply verbatim,
        // but a *position* needs checking against the monitors that
        // actually exist right now before it's safe to reuse.
        let mut remembered_position: Option<(i32, i32)> = None;
        if !window.app_id.is_empty() {
            if let Some((x, y, w, h)) = self.remembered_geometry.get(&window.app_id).copied() {
                window.geometry.width = w.max(MIN_WINDOW_WIDTH);
                window.geometry.height = h.max(MIN_WINDOW_HEIGHT);
                remembered_position = Some((x, y));
            }
        }
        // Every new window used to land on the *primary* monitor
        // unconditionally, regardless of which monitor the user was
        // actually working on - reported live as "why do all windows only
        // open on the first monitor" once a second, non-primary monitor
        // was actually in use.
        //
        // This used to check the *focused* window's monitor first and the
        // pointer only as a fallback, on the reasoning that focus is the
        // stronger "where is the user working" signal. Reported live as
        // still wrong the same way: launching an app while the pointer sat
        // on a second monitor's bare desktop (nothing focused *there* --
        // the panel/launcher that started it isn't a core-tracked window,
        // and the last *focused* window was still whatever had been open on
        // the first monitor) put the new window back on the first monitor
        // regardless. `self.focused` only changes when a real window is
        // focused, so it goes stale the moment the user's attention moves
        // to empty desktop, a panel, or a dock - exactly the case that
        // matters here. The pointer's own current monitor has no such
        // staleness: `set_pointer_monitor` is updated on every motion
        // event, so it always reflects where the user physically is right
        // now. Checked first for that reason, matching this compositor's
        // own mouse-first design (see `docs/DEFAULTS.md`) and the same
        // "active output follows the cursor" default every comparable
        // dynamic/floating compositor (Hyprland, Mutter/GNOME, sway's
        // `focus_follows_mouse`) ships. Falling back to the focused
        // window's monitor when the pointer's own is unknown, then all the
        // way back to primary as the last resort for the one case neither
        // signal can answer - a fresh session's very first window, before
        // any pointer motion has been reported at all.
        let target_monitor = self
            .pointer_monitor
            .and_then(|id| self.monitors.iter().find(|m| m.id == id))
            .or_else(|| self.focused.and_then(|id| self.windows.get(&id)).and_then(|w| self.monitors.iter().find(|m| m.id == w.monitor)))
            .or_else(|| self.primary_monitor());
        // A remembered position (see just above) wins over both the
        // pointer/focus-based `target_monitor` heuristic and smart
        // placement - real desktop convention (Windows, macOS) is "reopen
        // exactly where I left this app", not "wherever the mouse happens
        // to be right now", *provided* that position still lands on a
        // monitor that actually exists this run - a laptop undocked since
        // the position was saved, say, must not place a window off in
        // space on a monitor that's no longer there. Checked against every
        // current monitor's own *full* geometry (not the exclusive-zone-
        // shrunk usable one): a remembered position under where a bar now
        // sits is still "a real monitor, just partly covered", not invalid.
        let remembered_monitor = remembered_position.and_then(|(x, y)| self.monitors.iter().find(|m| m.full_geometry.contains_point(x, y)));
        if let (Some((x, y)), Some(monitor)) = (remembered_position, remembered_monitor) {
            window.monitor = monitor.id;
            window.geometry.x = x;
            window.geometry.y = y;
        } else if let Some(monitor) = target_monitor {
            window.monitor = monitor.id;
            let layout_name = self.workspace(workspace).map(|w| w.layout.clone()).unwrap_or_default();
            if layout_name != "tiling" {
                let existing: Vec<Rect> = self.windows_on_workspace(workspace).map(|w| w.geometry).collect();
                let size = (window.geometry.width, window.geometry.height);
                window.geometry = SmartPlacement::place(monitor, &existing, size, &self.placement);
            }
        }
        if let Some(geometry) = actions.as_ref().and_then(|a| a.geometry) {
            window.geometry = geometry;
        }
        // `general.phone_mode`'s own real default (see its doc comment on
        // `WindowManager` for the full "optional phone mode" reasoning):
        // every ordinary new window opens maximized, since a phone-shaped
        // screen has no real room for multiple windows side by side. A
        // rule's own explicit `maximized` action (`Some(true)` or
        // `Some(false)`) always wins regardless - this only supplies the
        // *default* `None` would otherwise fall back to. `window.floating`
        // already reflects any rule's own `floating` action by this point
        // (applied above) - a floating window (a picture-in-picture
        // popup, a dialog) is floating *because* it's meant to stay small,
        // so phone mode leaves it alone rather than maximizing it anyway.
        let maximize = actions.as_ref().and_then(|a| a.maximized).unwrap_or(self.phone_mode && !window.floating);

        self.windows.insert(id, window);
        self.order.push(id);
        self.focused = Some(id);
        // A new window goes on top, but must not cover a pinned one.
        self.restack_pinned();
        if maximize {
            self.toggle_maximize(id);
        }
        id
    }

    /// Retries rule matching for a window `add_window` couldn't conclusively
    /// match yet (see `Window::rules_applied`'s doc comment) - a backend
    /// calls this once a native Wayland window's real `title`/`app_id`
    /// become known, typically on its first real commit. A no-op once
    /// `rules_applied` is already `true`, so this is safe to call on every
    /// subsequent metadata change without rules re-applying repeatedly.
    ///
    /// Returns whether a rule actually matched and was applied - distinct
    /// from simply "ran" (this is a no-op past the first call regardless).
    /// A backend uses this to decide whether a follow-up geometry/decoration
    /// sync is warranted: `sync_geometry` re-stacks the window to the top
    /// via smithay's `Space::map_element` as a side effect of updating its
    /// tracked position (`map_element` always does this, `activate` or
    /// not - there is no "move without restacking" in this smithay
    /// version), so calling it on *every* title/app_id change - which
    /// happens constantly for perfectly ordinary reasons (a browser tab
    /// finishing a page load) long after the window's own creation - would
    /// silently yank an unfocused, unrelated window back to the front any
    /// time its title happened to update. Reported live as exactly that:
    /// an older window jumping in front of a newer, focused one with no
    /// user action to explain it.
    pub fn reapply_rules_if_pending(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else { return false };
        if window.rules_applied || (window.title.is_empty() && window.app_id.is_empty()) {
            log::warn!(
                "DECO-DIAG reapply_rules_if_pending id={id} SKIPPED rules_applied={} app_id={:?} title={:?}",
                window.rules_applied,
                window.app_id,
                window.title
            );
            return false;
        }
        let actions = self.rules.iter().find(|r| r.matcher.matches(window)).map(|r| r.actions.clone());
        log::warn!(
            "DECO-DIAG reapply_rules_if_pending id={id} app_id={:?} actions_found={} actions_decorated={:?}",
            window.app_id,
            actions.is_some(),
            actions.as_ref().and_then(|a| a.decorated)
        );
        let Some(window) = self.windows.get_mut(&id) else { return false };
        window.rules_applied = true;
        // `add_window`'s matching fallback only ever sees this once
        // `app_id` is actually known - for a native Wayland window that's
        // usually after creation (`set_app_id` lands later), which is
        // exactly why this needs its own check here too, not just there.
        // A rule's own `decorated` action, if any, still wins below.
        let mut heuristic_changed = false;
        if actions.as_ref().and_then(|a| a.decorated).is_none() && window.decorated && likely_draws_own_titlebar(&window.app_id) {
            window.decorated = false;
            heuristic_changed = true;
        }
        let Some(actions) = actions else { return heuristic_changed };
        if let Some(floating) = actions.floating {
            window.floating = floating;
        }
        if let Some(decorated) = actions.decorated {
            window.decorated = decorated;
        }
        log::warn!("DECO-DIAG reapply_rules_if_pending id={id} FINAL window.decorated={}", window.decorated);
        if let Some(color) = actions.border_color {
            window.border_color = color;
        }
        if let Some(width) = actions.border_width {
            window.border_width = width;
        }
        if let Some(pinned) = actions.pinned {
            window.always_on_top = pinned;
        }
        if let Some(opacity) = actions.opacity {
            window.opacity = opacity.clamp(0.0, 1.0);
        }
        if let Some(margin) = actions.resize_margin {
            window.resize_margin = Some(margin);
        }
        if let Some(ratio) = actions.aspect_ratio {
            window.aspect_ratio = Some(ratio);
        }
        if let Some(geometry) = actions.geometry {
            window.geometry = geometry;
        }
        if let Some(workspace) = actions.workspace {
            self.move_window_to_workspace(id, workspace);
        }
        if actions.maximized.unwrap_or(false) {
            self.toggle_maximize(id);
        }
        true
    }

    pub fn remove_window(&mut self, id: WindowId) -> Option<Window> {
        self.order.retain(|&w| w != id);
        if self.focused == Some(id) {
            self.focused = self.order.last().copied();
        }
        self.windows.remove(&id)
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values()
    }

    /// Windows in stacking order, topmost (most recently raised) last.
    pub fn stacking_order(&self) -> impl Iterator<Item = &Window> {
        self.order.iter().filter_map(|id| self.windows.get(id))
    }

    pub(super) fn windows_on_workspace(&self, workspace: WorkspaceId) -> impl Iterator<Item = &Window> {
        self.windows.values().filter(move |w| w.workspace == workspace)
    }

    pub fn raise_window(&mut self, id: WindowId) {
        if let Some(pos) = self.order.iter().position(|&w| w == id) {
            let id = self.order.remove(pos);
            self.order.push(id);
        }
        self.restack_pinned();
    }

    /// Sends a window to the back of the stack - the middle-click-titlebar
    /// convention most X11 WMs (twm, fvwm, IceWM) have always had and this
    /// one never did. Doesn't touch focus: lowering the window you're
    /// currently looking at out from under the pointer without also moving
    /// keyboard focus elsewhere would leave input going to a window that's
    /// no longer visible under the cursor, which is more surprising than
    /// useful. `restack_pinned` still runs afterward so a pinned window
    /// can't accidentally end up buried by this either.
    pub fn lower_window(&mut self, id: WindowId) {
        if let Some(pos) = self.order.iter().position(|&w| w == id) {
            let id = self.order.remove(pos);
            self.order.insert(0, id);
        }
        self.restack_pinned();
    }

    /// Toggles "always on top" for a window (Hyprland's `pin`), used for
    /// picture-in-picture and small HUD overlays that must stay visible
    /// while you work in something else.
    pub fn toggle_always_on_top(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.always_on_top = !w.always_on_top;
        }
        self.restack_pinned();
    }

    pub fn is_always_on_top(&self, id: WindowId) -> bool {
        self.windows.get(&id).map(|w| w.always_on_top).unwrap_or(false)
    }

    /// Moves every always-on-top window to the top of the stack, keeping
    /// their relative order.
    ///
    /// `order` is the stacking order (last = topmost), so pinning is not a
    /// property the renderer checks - it is maintained here, which means
    /// every existing consumer of `stacking_order` gets it for free and
    /// cannot forget to honour it.
    fn restack_pinned(&mut self) {
        if !self.windows.values().any(|w| w.always_on_top) {
            return;
        }
        let (pinned, rest): (Vec<_>, Vec<_>) = self
            .order
            .iter()
            .partition(|id| self.windows.get(id).is_some_and(|w| w.always_on_top));
        self.order = rest.into_iter().chain(pinned).collect();
    }

}
