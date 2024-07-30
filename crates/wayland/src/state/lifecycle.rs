use super::*;
impl CompState {
    pub(crate) fn new_managed_window(&mut self, toplevel: ToplevelSurface) {
        let surface = toplevel.wl_surface().clone();
        let id = {
            let mut wm = self.wm.borrow_mut();
            let id = wm.alloc_window_id();
            let title = with_toplevel_title(toplevel.wl_surface()).unwrap_or_default();
            let mut w = CoreWindow::new(id, title);
            w.app_id = with_toplevel_app_id(toplevel.wl_surface()).unwrap_or_default();
            w.geometry = srdwm_core::Rect::new(0, 0, 800, 600 + TITLEBAR_HEIGHT as i32 as u32);
            wm.add_window(w);
            // Starts the open-slide tween (see `WindowAnim`'s doc comment):
            // the window's first `sync_geometry` call below will see this,
            // register the tween, and place it here - a few pixels below
            // its resting position - rather than jumping straight to
            // `geometry`. Same size throughout, so no extra client configure
            // is needed for the tween itself.
            if wm.animations_enabled {
                if let Some(win) = wm.window_mut(id) {
                    let g = win.geometry;
                    win.anim_from = Some(srdwm_core::Rect { y: g.y + OPEN_SLIDE_OFFSET, ..g });
                }
            }
            id
        };

        let dwindow = DWindow::new_wayland_window(toplevel.clone());
        self.surface_to_id.insert(surface.clone(), id);
        self.id_to_window.insert(id, dwindow);
        // `sync_geometry` handles the initial placement itself (map_element
        // + the first configure, since `last_synced_size` has no entry yet
        // for this id) as well as starting the open-slide tween registered
        // above - see its own doc comment.
        self.sync_geometry(id);
        self.redraw_decoration_buffer(id);
        // `WindowManager::add_window` already made this the focused window in
        // srdwm's own state, but that alone is purely internal bookkeeping --
        // without this, a freshly-opened window receives no keystrokes and
        // can't copy/paste until it's clicked, because nothing ever gave it
        // real Wayland keyboard/selection focus. (Same class of bug as the
        // click-to-focus one fixed earlier; this is the creation path.)
        self.set_keyboard_focus(Some(surface));
        // A newly-mapped window goes on top, but not over a pinned one.
        self.raise_pinned();
        self.pending.borrow_mut().push(CoreEvent::WindowCreated(id));
        foreign_toplevel::window_created(self, id);
    }

    /// Applies a negotiated `zxdg_toplevel_decoration_v1` mode to our own
    /// `Window.decorated` flag and refreshes (or drops) its titlebar buffer
    /// to match - see `XdgDecorationHandler::request_mode`'s doc comment
    /// for why. A no-op if the surface has no window yet (decoration
    /// negotiation racing ahead of `new_toplevel`, which shouldn't happen
    /// in practice but costs nothing to guard against).
    pub(crate) fn set_decorated_from_mode(&mut self, surface: &WlSurface, decorated: bool) {
        let Some(&id) = self.surface_to_id.get(surface) else { return };
        if let Some(w) = self.wm.borrow_mut().window_mut(id) {
            w.decorated = decorated;
        }
        self.redraw_decoration_buffer(id);
        // Re-applies content size/position for the now-changed titlebar
        // reservation - see `sync_geometry`'s own doc comment on why this
        // can't be skipped: redrawing the titlebar buffer alone doesn't
        // touch the content area's size or offset at all.
        self.sync_geometry(id);
    }

    /// (Re)renders the titlebar band for `id` - background plus title text
    /// via `decoration::render_titlebar` - and replaces the buffer in
    /// `self.decorations`. Called on creation, geometry change (width
    /// affects layout), and focus change (text color).
    pub(crate) fn redraw_decoration_buffer(&mut self, id: WindowId) {
        let Some(w) = self.wm.borrow().window(id).cloned() else { return };
        let focused = self.wm.borrow().focused_id() == Some(id);
        let theme = self.wm.borrow().theme;
        if w.decorated {
            let fg = if focused { theme.titlebar_fg_focused } else { theme.titlebar_fg_unfocused };
            let width = w.geometry.width.max(1);
            // Always rounded now, bordered or not - `render_border_top`
            // gives a bordered window's border strip the matching rounded
            // cut, so there's no more square-frame-around-a-round-titlebar
            // clash to avoid. See `render_titlebar`'s `round_corners` doc
            // comment.
            let data = decoration::render_titlebar(width, TITLEBAR_HEIGHT, &w.title, theme.titlebar_bg, fg, true);
            let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (width as i32, TITLEBAR_HEIGHT as i32), 1, Transform::Normal, None);
            self.decorations.insert(id, buffer);
        } else {
            self.decorations.remove(&id);
        }
        // The border-top bitmap is independent of `decorated` - an
        // undecorated (CSD) window can still have `border_width > 0` - so
        // it's rebuilt here unconditionally rather than falling under the
        // early return above. Cached the same way `decorations` is, at the
        // same trigger points (creation, a size change, a rule re-applying,
        // and - since this call is now also reached from focus changes --
        // `w.border_color`'s focused/unfocused dimming): see `elements::
        // border_side_render_element`'s doc comment for why re-rasterizing
        // this every render frame (an earlier version of this method did)
        // was a real, continuous cost, not just a redundant one.
        if w.border_width > 0 {
            let color = effective_border_color(w.border_color, focused);
            let strips = decoration::border_strips(w.geometry, w.border_width);
            if strips[0].width > 0 && strips[0].height > 0 {
                let data = decoration::render_border_top(strips[0].width, w.border_width, color);
                let buffer =
                    MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (strips[0].width as i32, w.border_width as i32), 1, Transform::Normal, None);
                self.border_top_decorations.insert(id, buffer);
            } else {
                self.border_top_decorations.remove(&id);
            }
        } else {
            self.border_top_decorations.remove(&id);
        }
        // No shadow for a maximized/fullscreen window: it already reaches
        // (or, for fullscreen, exceeds) the monitor's own edge, so there is
        // nowhere for `SHADOW_SIZE` pixels of shadow to actually fall, and
        // a shadow drawn there would either be clipped to nothing useful or
        // - for a maximized window short of the true monitor edge - read
        // as a shadow the window doesn't visually need. Matches the
        // Hyprland/GNOME convention `MISSING.md` measures this compositor
        // against.
        let shadows_enabled = self.wm.borrow().shadows_enabled;
        if shadows_enabled && !w.maximized && !w.fullscreen {
            let data = decoration::shadow_bitmap(w.geometry.width, w.geometry.height);
            let rect = decoration::shadow_rect(w.geometry);
            let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (rect.width as i32, rect.height as i32), 1, Transform::Normal, None);
            self.shadow_buffers.insert(id, buffer);
        } else {
            self.shadow_buffers.remove(&id);
        }
    }

    pub(crate) fn remove_window(&mut self, surface: &WlSurface) {
        let Some(id) = self.surface_to_id.remove(surface) else { return };
        if let Some(w) = self.id_to_window.remove(&id) {
            self.space.unmap_elem(&w);
        }
        self.decorations.remove(&id);
        self.border_top_decorations.remove(&id);
        self.shadow_buffers.remove(&id);
        self.border_side_buffers.remove(&id);
        self.last_synced_size.remove(&id);
        self.content_epoch.remove(&id);
        self.rounded_content_buffers.remove(&id);
        // A window closing (crash, kill, or its own menu's "Close" action
        // racing ahead of this) while its context menu is still open would
        // otherwise leave the menu pointing at a dead id - selecting any
        // row on it would then silently no-op against a window that no
        // longer exists, with no indication anything went wrong.
        if self.context_menu.as_ref().is_some_and(|m| m.window == id) {
            self.close_context_menu();
        }
        self.wm.borrow_mut().remove_window(id);
        self.pending.borrow_mut().push(CoreEvent::WindowDestroyed(id));
        foreign_toplevel::window_closed(self, id);
        // `remove_window` may have picked a new focused window on its own
        // (falls back to whatever's now on top) - see `sync_keyboard_focus`'s
        // doc comment for why the Wayland/X11 side needs a separate nudge to
        // actually catch up to that.
        crate::input::sync_keyboard_focus(self);
        // Safety net for `zwp_idle_inhibit_manager_v1`: smithay's own
        // `IdleInhibitorState` only calls `uninhibit` on an explicit
        // `destroy` request, never on `Dispatch::destroyed` - so a video
        // player that crashes or gets killed instead of exiting cleanly
        // would leave its inhibitor permanently stuck, holding the whole
        // system awake forever with no client left to ever release it.
        // Its window closing is the one thing guaranteed to happen either
        // way, so this is what actually catches that case.
        if self.idle_inhibiting_surfaces.contains(surface) {
            self.idle_inhibiting_surfaces.retain(|s| s != surface);
            self.idle_notifier_state.set_is_inhibited(!self.idle_inhibiting_surfaces.is_empty());
        }
    }
}
