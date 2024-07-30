use super::*;


pub(crate) fn with_toplevel_title(surface: &WlSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(surface, |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap().title.clone().unwrap_or_default())
    })
}

/// Same pattern as `with_toplevel_title`, for `app_id` - the xdg-shell
/// equivalent of `WM_CLASS`, and what `srd.rule({ class = ... })` matches
/// against (`crates/core/src/rules.rs`).
///
/// Nothing read this before: `new_managed_window` populated `Window.title`
/// from `with_toplevel_title` but never touched `Window.app_id` at all, so
/// every native Wayland window had an *empty* app_id the entire time rules
/// are evaluated (`WindowManager::add_window` matches rules once, at
/// creation). Every `class`-based rule - including the Firefox
/// `decorated = false` one meant to stop srdwm drawing a second titlebar
/// on top of Firefox's own - could therefore never match a native Wayland
/// client, only an XWayland one (`map_window_request` in xwayland.rs does
/// set `app_id` correctly, from `X11Surface::class()`). Reported live as
/// Firefox showing two titlebars, "same for other applications" - exactly
/// what this predicts, since it silently breaks every class-matched rule
/// for every native Wayland app, not just Firefox's.
pub(crate) fn with_toplevel_app_id(surface: &WlSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(surface, |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap().app_id.clone().unwrap_or_default())
    })
}

/// Re-reads title/app_id from the surface's own xdg-shell state and updates
/// `Window`/foreign-toplevel listeners if either changed since last read.
///
/// Needed because a fresh toplevel's `new_toplevel` fires at
/// `xdg_surface.get_toplevel()` - role assignment - which for essentially
/// every real client happens *before* the `set_title`/`set_app_id`/first
/// `commit()` sequence that actually supplies them. `new_managed_window`
/// reading those fields at that moment (see its own comment) reliably got
/// nothing: not a race, a fixed ordering every client hits, confirmed live
/// by a peer session capturing the raw `zwlr_foreign_toplevel_handle_v1`
/// wire output and finding `app_id`/`title` empty on every window. Unlike
/// `Window.geometry`/state (double-buffered per xdg-shell semantics),
/// title and app_id are plain immediate-apply requests in smithay's own
/// `XdgToplevelSurfaceData`, so re-reading them on every commit - cheap,
/// and this is already a per-commit hook - keeps `Window.title`/`app_id`
/// (and anything, like `srd.rule`, that reads them) correct from the first
/// real commit onward instead of frozen at an empty initial snapshot.
pub(crate) fn sync_toplevel_metadata(state: &mut CompState, id: WindowId, surface: &WlSurface) {
    let title = with_toplevel_title(surface).unwrap_or_default();
    let app_id = with_toplevel_app_id(surface).unwrap_or_default();
    let changed = {
        let mut wm = state.wm.borrow_mut();
        let Some(w) = wm.window_mut(id) else { return };
        let changed = w.title != title || w.app_id != app_id;
        w.title = title;
        w.app_id = app_id;
        changed
    };
    if changed {
        // Now that `title`/`app_id` are real, give `add_window`'s rule
        // match (which ran before either was set - see `Window::
        // rules_applied`'s doc comment) a real chance. Only the *first*
        // successful evaluation (rule actually matched, `Some` returned)
        // warrants `redraw_decoration_buffer`/`sync_geometry` - unlike
        // `set_decorated_from_mode`, this fires on every subsequent title
        // change too (a page finishing loading, long after the window's
        // own creation), and `sync_geometry` re-stacks the window to the
        // top of smithay's `Space` as a side effect of `map_element`
        // (true regardless of its `activate` argument - there's no
        // "move without restacking" in this smithay version). Calling it
        // unconditionally here silently yanked an unrelated, unfocused
        // window back to the front any time its title happened to
        // update - reported live as an older window jumping in front of
        // a newer, focused one with no user action to explain it.
        if state.wm.borrow_mut().reapply_rules_if_pending(id) {
            state.redraw_decoration_buffer(id);
            state.sync_geometry(id);
        }
        crate::foreign_toplevel::send_state(state, id);
    }
}
