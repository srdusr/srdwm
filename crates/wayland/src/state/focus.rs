use super::*;

impl CompState {

    /// Sets keyboard focus *and* selection (clipboard/primary) focus to the
    /// same surface's client. These have to move together: the data-device
    /// protocols only ever offer the current selection to the client that
    /// holds selection focus, and only accept `set_selection` from it, so a
    /// window that has keyboard focus but not data-device focus can neither
    /// paste nor copy.
    pub(crate) fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        // While the session is locked, only the lock surface may hold focus.
        // This is the single chokepoint that enforces it: without the guard,
        // any path that focuses a window - notably `new_managed_window`,
        // i.e. *a client simply opening a window* - would hand keyboard
        // focus to a normal client at a locked screen. (Caught by an A/B
        // test that counted `wl_keyboard.enter` events delivered to a client
        // launched while locked; it was 1 before this guard, 0 after.)
        if self.lock.locked {
            // With multiple outputs there is a lock surface per output, and
            // any of them is a legitimate focus target.
            let is_lock_surface = surface
                .as_ref()
                .is_some_and(|s| self.lock.surfaces.values().any(|lock| lock.wl_surface() == s));
            if surface.is_some() && !is_lock_surface {
                return;
            }
        }
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let old_focus = keyboard.current_focus();
        if old_focus == surface {
            return;
        }
        let client = surface.as_ref().and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh.clone(), &self.seat.clone(), client.clone());
        set_primary_focus(&self.dh.clone(), &self.seat.clone(), client);
        self.update_net_active_window(surface.as_ref());
        foreign_toplevel::update_activated(self, old_focus.clone(), surface.as_ref());
        // `xdg_toplevel`'s own `Activated` state - never sent anywhere
        // before this. `keyboard.set_focus` below only delivers
        // `wl_keyboard.enter`/`leave`; it says nothing about `xdg_toplevel`
        // state, which is the signal GTK4/libadwaita's `:backdrop` CSS
        // pseudo-class (and equivalents elsewhere) actually key off to
        // decide whether to paint their *own* titlebar as focused. Without
        // this, no window - including the only one open, with nothing else
        // it could be losing focus to - ever received it, so any client
        // that draws its own focus indicator this way looked permanently
        // unfocused no matter how many other windows existed, even though
        // real keyboard input (`wl_keyboard.enter`/`keyboard.set_focus`
        // below) was unaffected and reached the right window regardless.
        self.set_window_activated(old_focus.as_ref(), false);
        self.set_window_activated(surface.as_ref(), true);
        let serial = SERIAL_COUNTER.next_serial();
        keyboard.set_focus(self, surface, serial);
    }

    /// Sets `xdg_toplevel`'s `Activated` state (or the X11 equivalent) for
    /// whichever window owns `surface`, flushing a configure for the native
    /// case - `DWindow::set_activated` alone only queues the pending state
    /// for an xdg-shell window; nothing sends it to the client without an
    /// explicit `send_configure` (the X11 case has no such split, its own
    /// `set_activated` talks to the X connection directly). A no-op if
    /// `surface` has no window (e.g. `None`, or a layer surface/popup,
    /// neither of which are `xdg_toplevel`s) or the state didn't actually
    /// change.
    fn set_window_activated(&mut self, surface: Option<&WlSurface>, active: bool) {
        let Some(id) = surface.and_then(|s| self.surface_to_id.get(s)).copied() else { return };
        let changed = match self.id_to_window.get(&id) {
            Some(w) if w.set_activated(active) => {
                if let Some(toplevel) = w.toplevel() {
                    toplevel.send_configure();
                }
                true
            }
            _ => false,
        };
        // Titlebar text and border colour both dim/brighten on focus (see
        // `redraw_decoration_buffer`'s `fg`/`effective_border_color`), but
        // neither was ever actually re-rasterized here before - this call
        // only ever ran for other reasons (creation, a resize, a rule
        // re-applying) that happened to also be roughly when focus
        // changed, in practice, close enough that the gap went unnoticed.
        // Gated on `changed` so a redundant `set_window_activated(_, false)`
        // for a surface that was never activated (the common `old_focus ==
        // None` case) doesn't force a pointless redraw.
        if changed {
            self.redraw_decoration_buffer(id);
        }
    }
}
