use super::*;

impl CompState {

    /// Re-raises always-on-top windows in the `Space`.
    ///
    /// `WindowManager` keeps pinned windows last in its own stacking order,
    /// but the `Space` has an order of its own that decides what actually
    /// draws on top - so pinning is only real once it is pushed here.
    /// Called after anything that raises a window.
    pub(crate) fn raise_pinned(&mut self) {
        let pinned: Vec<WindowId> = self.wm.borrow().stacking_order().filter(|w| w.always_on_top).map(|w| w.id).collect();
        for id in pinned {
            if let Some(w) = self.id_to_window.get(&id).cloned() {
                self.space.raise_element(&w, false);
            }
        }
    }

    pub(crate) fn sync_geometry(&mut self, id: WindowId) {
        // A pending `anim_from` (set by `toggle_maximize`/`toggle_fullscreen`,
        // or by `new_managed_window` for the open-slide) means the target
        // geometry below is where this window is *headed*, not where it
        // should appear right now - register (or replace) a tween and use
        // `WindowAnim::current_rect` in its place for this call and every
        // `tick_animations` call afterward, until it completes. `take()`
        // both reads and clears it, so a later, non-animated `sync_geometry`
        // call for the same window (an ordinary drag/resize frame) goes
        // straight back to applying `geometry` immediately, as before.
        let anim_from = self.wm.borrow_mut().window_mut(id).and_then(|w| w.anim_from.take());
        let Some((target, decorated, maximized, fullscreen)) = self.wm.borrow().window(id).map(|w| (w.geometry, w.decorated, w.maximized, w.fullscreen)) else { return };
        if let Some(from) = anim_from {
            let duration_ms = self.wm.borrow().animation_duration_ms;
            if from != target && duration_ms > 0 {
                self.window_anims
                    .insert(id, WindowAnim { from, to: target, start: Instant::now(), duration: Duration::from_millis(duration_ms as u64) });
            }
        }
        let geom = self.window_anims.get(&id).map(WindowAnim::current_rect).unwrap_or(target);
        // The titlebar band is only actually reserved when there is one --
        // an undecorated window (client-side decoration, see
        // `set_decorated_from_mode`) gets the whole of `geom` as content,
        // not `geom` minus a band that's no longer being drawn. Without
        // this, a window that negotiated client-side decoration kept the
        // same 30px gap at its top anyway: our titlebar wasn't drawn there
        // (correctly), but the content was still offset down and told it
        // was 30px shorter than the window actually is, leaving a blank
        // strip and the frame sitting visibly wrong relative to what's
        // inside it.
        let band = if decorated { TITLEBAR_HEIGHT as i32 } else { 0 };
        // Position always moves with the pointer; only a size change needs a
        // client configure or a titlebar re-render (see `last_synced_size`'s
        // doc comment).
        let size = (geom.width as i32, geom.height as i32 - band);
        let size_changed = self.last_synced_size.insert(id, size) != Some(size);
        let mut moved = false;
        if let Some(w) = self.id_to_window.get(&id) {
            // `w.geometry().loc` is the client's own `xdg_surface::
            // set_window_geometry` offset - a CSD client (GTK4/Firefox
            // concretely) declares its real visible content as a sub-rect
            // inset within a larger buffer that also reserves an invisible
            // shadow margin, even once the tiled-state hint below has told
            // it to skip drawing that shadow. `render_udev_frame`/
            // `winit/render.rs` both subtract this same offset from where
            // they draw the window's content, specifically so the client's
            // visible content lands at `geom.x, geom.y` instead of a
            // shadow-margin's width/height short of it - `space` has to
            // agree with that adjustment, not just rendering, or every
            // click computed via `win_relative = pos - space_loc` would
            // land `content_offset` short of whatever the user actually
            // clicked on: rendering moves the content, hit-testing keeps
            // routing against where the client's raw, unshifted buffer
            // origin used to be.
            let content_offset = w.geometry().loc;
            self.space.map_element(w.clone(), (geom.x - content_offset.x, geom.y + band - content_offset.y), false);
            moved = true;
            if let Some(top) = w.toplevel() {
                // xdg-shell position is a purely compositor-side concept --
                // the client is never told it - so only a size change
                // needs a configure here.
                if size_changed {
                    top.with_pending_state(|state| {
                        state.size = Some(size.into());
                        // No configure from this compositor, ever, set any
                        // `xdg_toplevel` state bit at all before this --
                        // confirmed by grepping the whole crate for
                        // `xdg_toplevel::State`, zero hits. GTK4 (Firefox
                        // concretely) reads the tiled bits to decide whether
                        // to reserve its own invisible client-side shadow
                        // margin around its actual content, independent of
                        // whether decoration is server- or client-side --
                        // with none ever sent, it always assumed "floating,
                        // might need a shadow" and kept reserving one. That
                        // margin sits inside the committed buffer but is
                        // functionally invisible, so this compositor's own
                        // border - drawn at the *full* geometry, margin
                        // included, since nothing here knew the margin
                        // existed - ended up visibly offset from where the
                        // client's real chrome began. Reported live as
                        // Firefox's border "not with the window," and more
                        // generally never feeling like part of it. Setting
                        // all four unconditionally (the same technique
                        // river/dwl use) tells every window it's flush
                        // against something and should skip its own shadow,
                        // regardless of whether it's actually in a tiled
                        // layout - which is the outcome actually wanted:
                        // this compositor draws the frame, so nothing else
                        // should also be reserving room for one.
                        state.states.set(xdg_toplevel::State::TiledLeft);
                        state.states.set(xdg_toplevel::State::TiledRight);
                        state.states.set(xdg_toplevel::State::TiledTop);
                        state.states.set(xdg_toplevel::State::TiledBottom);
                        // Same "no configure from this compositor ever set
                        // this" gap as the tiled bits above, confirmed the
                        // same way (grepped the whole crate for `State::
                        // Maximized`/`State::Fullscreen` outside foreign-
                        // toplevel-management, which is a *different*
                        // protocol read by external tools like a taskbar,
                        // not the client's own `xdg_toplevel` configure --
                        // zero hits there before this). The window was
                        // resized to the full monitor rect and told it was
                        // tiled on every side, but never actually told via
                        // the real protocol mechanism for it that it was
                        // maximized or fullscreen at all - indistinguishable
                        // from an ordinary tiled-to-the-edges floating
                        // window as far as the client could tell. Reported
                        // live as fullscreen leaving a persistent gap along
                        // one edge (Firefox keeping some of its own chrome
                        // logic that specifically keys off genuinely
                        // *knowing* it's fullscreen, not just being resized
                        // to fullscreen-sized). `unset` the other explicitly
                        // when only one applies - `WindowManager::
                        // toggle_fullscreen`/`toggle_maximize` are mutually
                        // exclusive, but nothing here should assume that
                        // holds forever just because it does today.
                        if maximized {
                            state.states.set(xdg_toplevel::State::Maximized);
                        } else {
                            state.states.unset(xdg_toplevel::State::Maximized);
                        }
                        if fullscreen {
                            state.states.set(xdg_toplevel::State::Fullscreen);
                        } else {
                            state.states.unset(xdg_toplevel::State::Fullscreen);
                        }
                    });
                    top.send_configure();
                }
            } else if let Some(x11) = w.x11_surface() {
                // Unlike xdg-shell, an X11 client's real on-screen position
                // is part of its own window state - it has to be told on
                // every move, not just every resize, the same way a real
                // X11 window manager sends continuous `ConfigureNotify`
                // during an interactive drag. Without this branch at all,
                // `sync_geometry` never reconfigured an XWayland window a
                // second time past its initial map: `space.map_element`
                // above still moved smithay's own tracked position (see
                // `resync_stacking_order`'s doc comment for the real
                // z-order side effect that has, since fixed below) and the
                // border/titlebar still redrew at the new `Window.geometry`
                // (both read it fresh every frame), but the real X11
                // client window was never told to move or resize - any
                // drag, resize, maximize, edge-snap, or tiling re-layout of
                // an XWayland-backed app left its actual content frozen at
                // its original position/size forever while srdwm's own
                // decoration moved freely around it.
                let _ = x11.configure(Rectangle::new((geom.x, geom.y + band).into(), size.into()));
            }
        }
        if size_changed && self.decorations.contains_key(&id) {
            self.redraw_decoration_buffer(id);
        }
        // See `resync_stacking_order`'s doc comment: `map_element` above
        // always re-stacks its target to the top of `Space`'s own order as
        // a side effect of updating position, `activate` or not - and
        // `sync_geometry` runs for reasons with nothing to do with raising
        // a window (a title changing, an ordinary resize frame), so left
        // uncorrected this silently, non-deterministically desynced
        // `Space`'s notion of "on top" from `WindowManager`'s.
        if moved {
            self.resync_stacking_order();
        }
    }
}
