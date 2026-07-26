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
            // See `Window::size_is_provisional`'s own doc comment: only
            // when `add_window` actually used the guessed `800x600` above
            // (not a remembered size, a rule's own `geometry` action, or a
            // maximize/phone-mode fill) does the client get to pick its own
            // size instead - `sync_geometry`/`adopt_provisional_size` are
            // what actually act on membership here.
            if wm.window(id).is_some_and(|w| w.size_is_provisional) {
                self.provisional_size.insert(id);
            }
            // Nothing is drawn for this window until its client paints,
            // and the open-slide tween starts then rather than here - a
            // toplevel role exists well before a client's first buffer, so
            // starting the animation here ran it against an empty frame and
            // left the window simply appearing, already at rest.
            // `CompositorHandler::commit` does both. This is the only place
            // anything is ever put into `awaiting_first_buffer`; see that
            // field for why that matters.
            self.awaiting_first_buffer.insert(id);
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
        // Read fresh every call, not cached from creation - a client can
        // call `xdg_toplevel.set_parent` well after its own initial map
        // (a "Save As" dialog opened from an already-open main window,
        // say), and this function already re-runs on every relevant state
        // change. Written back onto the real `Window` (not just used
        // locally) so `ResizeEdge::hit_test`'s own `is_dialog` parameter
        // - read from `core`, which has no protocol concept to derive
        // this from itself - agrees with whatever got drawn here.
        //
        // Checks both real toplevel kinds a `DWindow` can wrap: a native
        // `xdg_toplevel`'s own `parent()`, or an XWayland `X11Surface`'s
        // `WM_TRANSIENT_FOR` via `is_transient_for()`. The X11 half used
        // to be unchecked entirely (`.toplevel()` alone, which is always
        // `None` for an X11-backed window - `X11Surface`'s own accessor
        // is `.x11_surface()`, a different method), so every XWayland
        // dialog - a GTK "Save As", an app's own "About" box, anything
        // that sets the ICCCM transient-for hint - always drew with the
        // full three-button titlebar and traffic-light colours, the
        // native-Wayland-only case this whole feature was built for.
        // Reported live: "dialog windows... should never have traffic
        // light, should just be x" - true for native Wayland dialogs
        // already, not for XWayland ones.
        let is_dialog = self.id_to_window.get(&id).is_some_and(|dw| {
            dw.toplevel().is_some_and(|t| t.parent().is_some()) || dw.x11_surface().is_some_and(|x| x.is_transient_for().is_some())
        });
        // Resolved the same way and at the same time as `is_dialog` above,
        // for the same reason: this is protocol state, which `core` cannot
        // read for itself. A client that pinned min == max on both axes is
        // telling us it cannot be resized - and therefore cannot be
        // maximized, so the Maximize button would do nothing. `0` means
        // "unconstrained" on that axis in both protocols, so a zero on
        // either side is never a match. See `Window::resizable`.
        let resizable = self.id_to_window.get(&id).is_none_or(|dw| {
            if let Some(toplevel) = dw.toplevel() {
                let (min, max) = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
                    let mut cached = states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
                    let current = cached.current();
                    (current.min_size, current.max_size)
                });
                return !(min.w > 0 && min.h > 0 && min == max);
            }
            if let Some(x11) = dw.x11_surface() {
                if let (Some(min), Some(max)) = (x11.min_size(), x11.max_size()) {
                    return min != max;
                }
            }
            true
        });
        // The client's own declared minimum, read from the same place
        // `resizable` above comes from. A rule's `min_size` still wins --
        // it is applied at map time and this must not undo it - so this
        // only ever raises the floor above the global default, never
        // overwrites a deliberate override.
        let declared_min = self.id_to_window.get(&id).and_then(|dw| {
            if let Some(toplevel) = dw.toplevel() {
                let min = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
                    let mut cached = states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
                    cached.current().min_size
                });
                return (min.w > 0 && min.h > 0).then_some((min.w as u32, min.h as u32));
            }
            dw.x11_surface().and_then(|x| x.min_size()).and_then(|m| (m.w > 0 && m.h > 0).then_some((m.w as u32, m.h as u32)))
        });
        if let Some(win) = self.wm.borrow_mut().window_mut(id) {
            win.is_dialog = is_dialog;
            win.resizable = resizable;
            if let Some(min) = declared_min {
                if !win.min_size_from_rule {
                    win.min_size = min;
                }
            }
        }
        let show_maximize = {
            let wm = self.wm.borrow();
            wm.window(id).is_none_or(|win| wm.show_maximize(win))
        };
        // Corrects `w.geometry`'s far edge to match what the client's
        // surface really committed, when that's known - see
        // `effective_frame`'s own doc comment. Every bitmap this method
        // builds (titlebar, top/bottom border, shadow) is sized from
        // `frame`, not `w.geometry` directly, so a client that settles on
        // a slightly different real size than requested (a terminal
        // snapping to a whole number of character cells, most commonly)
        // gets decoration that actually hugs its real edge instead of the
        // asked-for one.
        let frame = self.effective_frame(id, w.geometry);
        // Eased (ease-out-cubic, same curve `WindowAnim::current_rect`
        // already uses - see that doc comment) progress of the glyph-
        // reveal-on-hover animation, discretized to a `u8` alpha. `theme.
        // button_glyph_always` skips the timing/easing math entirely and
        // just asks for full opacity outright - see `render_titlebar`'s
        // own `glyph_always` parameter for where that's actually applied
        // (it overrides this per-button, not just here).
        let hovered_button = self.hovered_titlebar_button.and_then(|(hid, hit, start)| {
            (hid == id).then(|| {
                let t = (start.elapsed().as_secs_f32() / decoration::HOVER_GLYPH_DURATION.as_secs_f32()).min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                (hit, (eased * 255.0).round() as u8)
            })
        });
        // `main.rs`'s `sync()` calls `Platform::redraw_decoration` - which
        // always reaches here - for every visible window on every dirty
        // tick, not only the window that actually changed (see `Comp
        // State::decoration_signatures`'s own doc comment: a resize drag on
        // one window re-renders every *other* open window's title text and
        // border strips too, once per pointer-motion event, for pixels
        // identical to what's already cached). Skipping the rebuild when
        // nothing this function reads has actually changed since the last
        // call turns those redundant calls into a cheap signature
        // comparison instead of a full re-rasterization.
        // Whether this window is *actually* occupying a tiled slot right
        // now - see the shadow gate further down for the full reasoning.
        // Computed once, here, and reused there rather than recomputed:
        // it depends on the workspace's own layout name, not just this
        // window's own `floating` flag, so it has to be part of the
        // signature too - switching a workspace's layout (`Super+Shift+
        // t`/`s`) changes this for every window on it without touching
        // any of their own `floating` fields, and a signature that didn't
        // track it would keep serving whichever shadow state was cached
        // before the switch.
        let currently_tiled = self.wm.borrow().workspace(w.workspace).is_some_and(|ws| ws.layout == "tiling") && !w.floating;
        let signature = DecorationSignature {
            width: frame.width,
            height: frame.height,
            decorated: w.decorated,
            focused,
            title: w.title.clone(),
            border_color: w.border_color,
            border_width: w.border_width,
            corner_radius: w.corner_radius,
            maximized: w.maximized,
            fullscreen: w.fullscreen,
            shadows_enabled: self.wm.borrow().shadows_enabled,
            currently_tiled,
            hovered_button,
            title_centered: theme.title_centered,
            buttons_left: theme.buttons_left,
            button_glyph_always: theme.button_glyph_always,
            button_order: theme.button_order,
            traffic_light_buttons: theme.traffic_light_buttons,
            is_dialog,
        };
        if self.decoration_signatures.get(&id) == Some(&signature) {
            return;
        }
        self.decoration_signatures.insert(id, signature);
        if w.decorated {
            let fg = if focused { theme.titlebar_fg_focused } else { theme.titlebar_fg_unfocused };
            let width = frame.width.max(1);
            // Always rounded now, bordered or not - `render_border_top`
            // gives a bordered window's border strip the matching rounded
            // cut, so there's no more square-frame-around-a-round-titlebar
            // clash to avoid. See `render_titlebar`'s `round_corners` doc
            // comment.
            let data = decoration::render_titlebar(
                width,
                TITLEBAR_HEIGHT,
                &w.title,
                theme.titlebar_bg,
                fg,
                true,
                w.corner_radius,
                w.border_width,
                focused,
                hovered_button,
                theme.title_centered,
                theme.buttons_left,
                theme.button_glyph_always,
                theme.button_order,
                theme.traffic_light_buttons,
                is_dialog,
                show_maximize,
            );
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
            let color = effective_border_color(w.border_color, focused, theme.border_inactive_dim);
            let strips = decoration::border_strips(frame, w.border_width);
            // `render_border_top`/`render_border_bottom` both return a
            // buffer `border_width.max(corner_radius)` rows tall now, not
            // always exactly `border_width` - see their own doc comments
            // for why a strip thinner than the corner radius needs the
            // extra rows to let the curve actually resolve before handing
            // off to the (curve-blind) side strips. `render.rs`'s call
            // site positions this taller buffer to match: the top strip
            // grows downward from its existing anchor (unchanged), the
            // bottom strip grows upward, so its anchor shifts up by
            // exactly the extra height.
            let strip_h = w.border_width.max(w.corner_radius);
            if strips[0].width > 0 && strips[0].height > 0 {
                let data = decoration::render_border_top(strips[0].width, w.border_width, color, w.corner_radius, w.decorated);
                let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (strips[0].width as i32, strip_h as i32), 1, Transform::Normal, None);
                self.border_top_decorations.insert(id, buffer);
            } else {
                self.border_top_decorations.remove(&id);
            }
            if strips[1].width > 0 && strips[1].height > 0 {
                let data = decoration::render_border_bottom(strips[1].width, w.border_width, color, w.corner_radius);
                let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (strips[1].width as i32, strip_h as i32), 1, Transform::Normal, None);
                self.border_bottom_decorations.insert(id, buffer);
            } else {
                self.border_bottom_decorations.remove(&id);
            }
        } else {
            self.border_top_decorations.remove(&id);
            self.border_bottom_decorations.remove(&id);
        }
        // No shadow for a maximized/fullscreen window: it already reaches
        // (or, for fullscreen, exceeds) the monitor's own edge, so there is
        // nowhere for `SHADOW_SIZE` pixels of shadow to actually fall, and
        // a shadow drawn there would either be clipped to nothing useful or
        // - for a maximized window short of the true monitor edge - read
        // as a shadow the window doesn't visually need. Matches the
        // Hyprland/GNOME convention `MISSING.md` measures this compositor
        // against.
        //
        // No shadow for a window that is *actually being tiled right now*
        // - a real, reported bug, not a style choice made up front: a
        // drop shadow exists to separate a window from whatever is
        // visually *behind* it, but a tiled window is coplanar and
        // adjacent to its neighbours by construction, with nothing behind
        // it to separate from. `SHADOW_SIZE` pixels of shadow with only
        // `gap_inner` pixels of real gap to fall into (as little as 1px)
        // has nowhere to land except on the neighbouring tile, darkening
        // it by up to `SHADOW_MAX_ALPHA` - reported live as "some windows
        // are dark tinted."
        //
        // Gating this on `w.floating` alone (the first version of this
        // fix) was a real regression, caught live: `arrange_workspace`
        // only reads `floating` under the `"tiling"` layout, so every
        // window on this project's own default `"dynamic"` layout starts
        // - and stays - `floating: false` unless something explicitly
        // flips it. That first version read `floating: false` as "this
        // window is tiled, no shadow" regardless of which layout was
        // actually running, so *every* window under dynamic/floating mode
        // silently lost its shadow, recoverable only by toggling `Super+S`
        // (`srd.window.toggle_floating()`) - which then looked like that
        // key toggles a "tint", not floating, since floating itself does
        // nothing visible under a layout that never tiles anyone. `Window::
        // floating` only ever means "opted out of tiling" *within* a
        // workspace that tiles at all - checking the workspace's own
        // layout name first is what this needed instead: a window is only
        // "currently tiled" when both are true. `currently_tiled` itself
        // was already computed above, alongside the signature.
        let shadows_enabled = self.wm.borrow().shadows_enabled;
        if shadows_enabled && !currently_tiled && !w.maximized && !w.fullscreen {
            // A decorated window's corners are *always* rounded (the
            // titlebar/border strips round to `corner_radius` regardless of
            // this setting - see their own call sites); an undecorated
            // (CSD) window's own content only gets rounded when `general.
            // rounded_corners` is on (default off on this backend - see
            // `WindowManager::rounded_corners_enabled`'s doc comment). The
            // shadow has to match whichever is actually true for *this*
            // window, or it mismatches in the other direction: a rounded
            // shadow around a still-square undecorated window with content
            // rounding off.
            let rounded_corners_enabled = self.wm.borrow().rounded_corners_enabled.unwrap_or(false);
            let shadow_radius = if w.decorated || rounded_corners_enabled { w.corner_radius } else { 0 };
            // Dimmed the same way `effective_border_color` dims an
            // unfocused window's border - see `shadow_bitmap`'s own
            // `max_alpha` doc comment for the real-desktop convention this
            // matches (Hyprland's `color`/`color_inactive` shadow split).
            let max_alpha = if focused {
                decoration::SHADOW_MAX_ALPHA
            } else {
                (decoration::SHADOW_MAX_ALPHA as f32 * theme.border_inactive_dim).round().clamp(0.0, 255.0) as u8
            };
            let data = decoration::shadow_bitmap(frame.width, frame.height, shadow_radius, max_alpha);
            let rect = decoration::shadow_rect(frame);
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
        self.border_bottom_decorations.remove(&id);
        self.shadow_buffers.remove(&id);
        self.border_side_buffers.remove(&id);
        self.decoration_signatures.remove(&id);
        self.awaiting_first_buffer.remove(&id);
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
        // Same reasoning as the context menu above, for the Snap-Layouts
        // flyout.
        if self.snap_flyout.as_ref().is_some_and(|f| f.window == id) {
            self.close_snap_flyout();
        }
        self.wm.borrow_mut().remove_window(id);
        // Persists whatever `remove_window` just snapshotted into
        // `remembered_geometry` - see that function's own doc comment for
        // why a window closing, not just a manual drag/resize release,
        // needs to reach disk too.
        crate::window_memory::save_all(self.wm.borrow().all_remembered_geometry());
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
