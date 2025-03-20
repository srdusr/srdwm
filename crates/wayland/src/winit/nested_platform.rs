use super::*;

impl Platform for WaylandPlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::Wayland
    }

    /// **Self-paced, deliberately**: nothing else in this backend ever
    /// blocks. `pump_winit`'s underlying `dispatch_new_events` polls
    /// (returns immediately either way), and smithay's winit backend
    /// hardcodes `vsync: false` on the EGL surface it creates
    /// (`init_from_attributes_with_gl_attr` in smithay 0.7.0's own
    /// `backend/winit/mod.rs` - true of *every* entry point into that
    /// module, including the one this backend used before it needed custom
    /// `WindowAttributes`, so this was never introduced by that switch).
    /// `swap_buffers` therefore returns as soon as the GPU accepts the
    /// frame, with no wait for the next display refresh at all. Before this
    /// fix, that meant `poll_events` -> `render_frame` -> full render +
    /// `swap_buffers` ran back-to-back with nothing pacing the `while
    /// running.get()` loop in `main.rs` between iterations - confirmed
    /// live: an idle nested instance, zero windows, sat at a sustained
    /// ~52% of one core (`ps -o %cpu`), because it was rendering and
    /// presenting a full frame as fast as the CPU/GPU could physically
    /// cycle, forever, whether or not anything on screen had changed.
    /// Fixed by giving `idle_event_loop.dispatch` (already called every
    /// tick to service `ext_idle_notify_v1`'s timers, see its field doc
    /// comment) a real timeout instead of always `Duration::ZERO`: the
    /// remaining budget until `TARGET_FRAME_TIME` has elapsed since the
    /// last frame, clamped to zero once that budget is already spent. This
    /// reuses the one blocking wait this backend already has rather than
    /// adding a second, separate `thread::sleep`, and still services any
    /// idle-notify timer that comes due sooner than a full frame away.
    fn poll_events(&mut self) -> PlatformResult<Vec<CoreEvent>> {
        self.accept_clients()?;
        let closed = self.pump_winit()?;
        if closed {
            return Err(PlatformError::Other("compositor window closed".into()));
        }
        // Held bindings that repeat - see `CompState::tick_repeat`.
        self.state.tick_repeat();
        self.display.dispatch_clients(&mut self.state).map_err(err)?;
        self.display.flush_clients().map_err(err)?;
        if let Some(ipc) = self.ipc.as_mut() {
            if ipc.poll(&self.wm) {
                self.pending.borrow_mut().push(CoreEvent::WorkspaceChanged);
                // Same re-sync as `udev/platform.rs`'s matching block - see
                // its own comment. `handle_request` only ever touches core's
                // `WindowManager`, never `state.space`, so an IPC focus
                // change left rendering/hit-testing on the stale topmost
                // window until something else happened to raise it.
                //
                // `raise_in_space`, not `focus_window` - see that
                // function's doc comment: the full version re-runs the
                // workspace-follow side effect on the already-focused
                // window and silently reverts an `activate_workspace` IPC
                // dispatch from the same cycle.
                let focused = self.wm.borrow().focused_id();
                if let Some(id) = focused {
                    crate::input::raise_in_space(&mut self.state, id);
                }
            }
        }
        // Same lock-request draining as `udev/platform.rs`'s matching
        // block - see its own comment. Exercised here too (not just on
        // the real udev backend) specifically so a native lock can be
        // tested against this nested dev session without ever touching
        // the live tty1 one.
        if self.wm.borrow_mut().drain_lock_request() {
            self.state.begin_native_lock();
        }
        self.state.poll_native_lock_auth();
        let wait = TARGET_FRAME_TIME.saturating_sub(self.last_frame.elapsed());
        let _ = self.idle_event_loop.dispatch(Some(wait), &mut self.state);
        self.last_frame = Instant::now();
        self.render_frame()?;
        Ok(self.pending.borrow_mut().drain(..).collect())
    }

    fn monitors(&mut self) -> PlatformResult<Vec<srdwm_core::Monitor>> {
        // Shrunk by any layer-shell exclusive zone - see the matching
        // comment in `udev/platform.rs`'s `monitors()`. This backend is always a
        // single output at the global origin, so the output-local zone
        // rectangle already is the usable global-space rect.
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        Ok(vec![{
            let rect = srdwm_core::Rect::new(zone.loc.x, zone.loc.y, zone.size.w as u32, zone.size.h as u32);
            let mut m = srdwm_core::Monitor::new(0, "winit", rect);
            // Same fix as `udev/platform.rs`'s matching function: `Monitor::new`
            // defaults `full_geometry` to `geometry`, which is already
            // zone-shrunk here - without this, `toggle_fullscreen` had no
            // way to actually cover a bar/dock's reserved strip, since the
            // "true full rect" it targets was silently identical to the
            // "usable, shrunk rect" `toggle_maximize` targets.
            let full = self.backend.window_size();
            m.full_geometry = srdwm_core::Rect::new(0, 0, full.w as u32, full.h as u32);
            m.maximize_geometry = crate::input::maximize_geometry_for(&self.output, m.full_geometry);
            m.primary = true;
            m
        }])
    }

    fn apply_geometry(&mut self, window: WindowId, geometry: srdwm_core::Rect) -> PlatformResult<()> {
        let _ = geometry;
        self.state.sync_geometry(window);
        // See `udev/platform.rs`'s own `apply_geometry` doc comment - same
        // gap (the cached border/titlebar bitmap only rebuilds on a commit,
        // focus change, or a few specific paths, not this one), same fix.
        self.state.redraw_decoration_buffer(window);
        Ok(())
    }

    fn set_title(&mut self, _window: WindowId, _title: &str) -> PlatformResult<()> {
        Ok(())
    }

    /// See `udev/platform.rs`'s matching impl for why this has to go through
    /// `crate::input::focus_window` (the same path a real mouse click
    /// already uses) rather than only touching core state.
    fn focus(&mut self, window: WindowId) -> PlatformResult<()> {
        crate::input::focus_window(&mut self.state, window);
        Ok(())
    }

    fn minimize(&mut self, window: WindowId) -> PlatformResult<()> {
        if let Some(w) = self.state.id_to_window.get(&window) {
            self.state.space.unmap_elem(w);
        }
        Ok(())
    }

    fn restore(&mut self, window: WindowId) -> PlatformResult<()> {
        self.state.sync_geometry(window);
        self.state.redraw_decoration_buffer(window);
        Ok(())
    }

    fn close(&mut self, window: WindowId) -> PlatformResult<()> {
        let Some(w) = self.state.id_to_window.get(&window) else { return Ok(()) };
        if let Some(toplevel) = w.toplevel() {
            toplevel.send_close();
        } else if let Some(x11) = w.x11_surface() {
            // Same fix as `udev/platform.rs`'s matching function: `w.toplevel()`
            // is `None` for an XWayland window, so closing one silently did
            // nothing at all without this arm.
            let _ = x11.close();
        }
        Ok(())
    }

    fn set_decorated(&mut self, _window: WindowId, _decorated: bool) -> PlatformResult<()> {
        Ok(())
    }

    fn set_border_color(&mut self, _window: WindowId, _rgb: (u8, u8, u8)) -> PlatformResult<()> {
        Ok(())
    }

    fn set_border_width(&mut self, _window: WindowId, _width: u32) -> PlatformResult<()> {
        Ok(())
    }

    fn redraw_decoration(&mut self, window: WindowId, _win: &CoreWindow, _focused: bool) -> PlatformResult<()> {
        // Re-renders the title/focus-color band and re-syncs geometry;
        // `sync_geometry` re-renders the decoration too, but only if one
        // already exists, so this also covers first paint.
        self.state.redraw_decoration_buffer(window);
        self.state.sync_geometry(window);
        Ok(())
    }

    fn grab_keyboard(&mut self) -> PlatformResult<()> {
        Ok(())
    }

    fn ungrab_keyboard(&mut self) -> PlatformResult<()> {
        Ok(())
    }

    fn keyboard_layout(&mut self) -> PlatformResult<String> {
        let Some(keyboard) = self.state.seat.get_keyboard() else { return Ok(String::new()) };
        Ok(keyboard.with_xkb_state(&mut self.state, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            let layout = xkb.active_layout();
            xkb.layout_name(layout).to_string()
        }))
    }

    fn cycle_keyboard_layout(&mut self) -> PlatformResult<String> {
        let Some(keyboard) = self.state.seat.get_keyboard() else { return Ok(String::new()) };
        Ok(keyboard.with_xkb_state(&mut self.state, |mut ctx| {
            ctx.cycle_next_layout();
            let xkb = ctx.xkb().lock().unwrap();
            let layout = xkb.active_layout();
            xkb.layout_name(layout).to_string()
        }))
    }
}
