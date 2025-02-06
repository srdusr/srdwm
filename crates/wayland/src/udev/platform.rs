use super::*;
use super::drm::{bring_up_head, pick_crtc, probe_connected};
use super::session::{register_drm_fd, register_libinput, register_session_notifier, register_udev_monitor};

pub struct UdevPlatform {
    event_loop: EventLoop<'static, CompState>,
    display: Display<CompState>,
    state: CompState,
    listener: ListeningSocket,
    clients: Vec<Client>,
    pending: Rc<RefCell<Vec<CoreEvent>>>,
    ipc: Option<srdwm_platform::IpcServer>,
}

impl UdevPlatform {
    pub fn connect(wm: Rc<RefCell<WindowManager>>, bound_keys: &[String], repeat_keys: &[String]) -> PlatformResult<Self> {
        let event_loop: EventLoop<'static, CompState> = EventLoop::try_new().map_err(err)?;

        let (session, notifier) = LibSeatSession::new().map_err(err)?;
        let seat_name = session.seat();

        let gpu_path = udev::primary_gpu(&seat_name)
            .ok()
            .flatten()
            .unwrap_or_else(|| std::path::PathBuf::from("/dev/dri/card0"));
        log::info!("udev: using {} as primary GPU", gpu_path.display());

        let mut session_for_open = session.clone();
        let fd = session_for_open
            .open(&gpu_path, rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC)
            .map_err(err)?;
        let card = Rc::new(Card(fd));

        // Every connected connector becomes a head, laid out left-to-right.
        let connected = probe_connected(&card)?;
        log::info!("udev: {} connected output(s)", connected.len());

        let renderer = PixmanRenderer::new().map_err(err)?;
        let dh = Display::<CompState>::new().map_err(err)?;
        let display_handle = dh.handle();
        // `zwp_linux_dmabuf_v1` - see `protocols.rs`'s `DmabufHandler` for
        // why `PixmanRenderer`, a pure software renderer, can still import
        // these (mmap, not GPU). `create_global` (v3) rather than the v4
        // `..._with_default_feedback` variant: the latter needs a
        // `main_device` `dev_t` to steer multi-GPU clients toward the
        // right render node, which is a real gap worth closing later but
        // not required for a single-GPU client to allocate and hand over a
        // Linear-modifier buffer, which is all this backend can use anyway.
        let mut dmabuf_state = DmabufState::new();
        dmabuf_state.create_global::<CompState>(&display_handle, renderer.dmabuf_formats());

        let mut heads: Vec<UdevHead> = Vec::new();
        let mut output_entries: Vec<crate::state::OutputEntry> = Vec::new();
        let mut used_crtcs: Vec<crtc::Handle> = Vec::new();
        let mut x_offset = 0;
        for probe in &connected {
            let Some(crtc) = pick_crtc(&card, probe, &used_crtcs) else {
                log::warn!("udev: no free CRTC left for connector {}; not driving it", probe.name);
                continue;
            };
            let (head, entry) = bring_up_head(&card, &display_handle, probe, crtc, x_offset)?;
            log::info!("udev: head {}: {} {}x{} at x={x_offset}", heads.len(), probe.name, head.size.0, head.size.1);
            used_crtcs.push(crtc);
            x_offset += head.size.0;
            heads.push(head);
            output_entries.push(entry);
        }

        let Some(first) = heads.first() else {
            return Err(PlatformError::Other("udev: no usable outputs".into()));
        };
        // Pointer starts centred on the first head.
        let (width, height) = first.size;
        // xdg-output - see the matching comment in `lib.rs`'s
        // `WaylandPlatform::connect` for why this isn't optional.
        smithay::wayland::output::OutputManagerState::new_with_xdg_output::<CompState>(&display_handle);

        let compositor_state = CompositorState::new::<CompState>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<CompState>(&display_handle);
        let xdg_decoration_state = XdgDecorationState::new::<CompState>(&display_handle);
        let shm_state = ShmState::new::<CompState>(&display_handle, Vec::new());
        // Selection (clipboard) protocols - see the matching block in
        // `lib.rs`'s `WaylandPlatform::connect` for the ordering constraint.
        let primary_selection_state = PrimarySelectionState::new::<CompState>(&display_handle);
        let data_control_state =
            DataControlState::new::<CompState, _>(&display_handle, Some(&primary_selection_state), |_| true);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "seat0");
        let system_xkb = crate::xkb_config::read();
        let xkb_config = smithay::input::keyboard::XkbConfig {
            rules: "",
            model: system_xkb.model.as_deref().unwrap_or(""),
            layout: system_xkb.layout.as_deref().unwrap_or(""),
            variant: system_xkb.variant.as_deref().unwrap_or(""),
            options: system_xkb.options.clone(),
        };
        // 600ms delay, not 200 - see `state/mod.rs`'s `REPEAT_DELAY` doc
        // comment for why.
        seat.add_keyboard(xkb_config, 600, 25).map_err(err)?;
        seat.add_pointer();

        // Each output occupies its own slice of the global space, so a
        // window's coordinates say which monitor it is on.
        let mut space = Space::default();
        for entry in &output_entries {
            space.map_output(&entry.output, (entry.location.x, entry.location.y));
        }

        let pending = Rc::new(RefCell::new(Vec::new()));
        let udev_state = UdevState {
            card: card.clone(),
            renderer,
            heads,
            active: true,
            pointer_pos: (width as f64 / 2.0, height as f64 / 2.0).into(),
            session: session.clone(),
        };

        let state = CompState {
            compositor_state,
            xdg_shell_state,
            _xdg_decoration_state: xdg_decoration_state,
            shm_state,
            dmabuf_state,
            xdg_activation_state: XdgActivationState::new::<CompState>(&display_handle),
            _text_input_manager_state: smithay::wayland::text_input::TextInputManagerState::new::<CompState>(&display_handle),
            _input_method_manager_state: smithay::wayland::input_method::InputMethodManagerState::new::<CompState, _>(&display_handle, |_client| true),
            _gtk_shell_state: crate::gtk_shell::GtkShellState::new::<CompState>(&display_handle),
            seat_state,
            seat,
            space,
            popups: PopupManager::default(),
            outputs: output_entries,
            layer_shell_state: smithay::wayland::shell::wlr_layer::WlrLayerShellState::new::<CompState>(&display_handle),
            dh: display_handle.clone(),
            data_device_state: DataDeviceState::new::<CompState>(&display_handle),
            primary_selection_state,
            data_control_state,
            session_lock_state: smithay::wayland::session_lock::SessionLockManagerState::new::<CompState, _>(
                &display_handle,
                |_| true,
            ),
            _screencopy_state: crate::screencopy::ScreencopyState::new::<CompState>(&display_handle),
            screencopy_pending: Vec::new(),
            _appmenu_state: crate::appmenu::AppmenuManagerState::new::<CompState>(&display_handle),
            _foreign_toplevel_state: crate::foreign_toplevel::ForeignToplevelState::new::<CompState>(&display_handle),
            foreign_toplevel_managers: Vec::new(),
            foreign_toplevel_handles: HashMap::new(),
            _workspace_state: crate::workspace::WorkspaceManagerState::new::<CompState>(&display_handle),
            _output_power_state: Some(crate::output_power::OutputPowerManagerState::new::<CompState>(&display_handle)),
            _gamma_control_state: Some(crate::gamma_control::GammaControlManagerState::new::<CompState>(&display_handle)),
            _output_management_state: crate::output_management::OutputManagementState::new::<CompState>(&display_handle),
            output_managers: Vec::new(),
            output_heads: HashMap::new(),
            output_modes: HashMap::new(),
            output_serial: 0,
            last_broadcast_outputs: Vec::new(),
            workspace_managers: Vec::new(),
            workspace_groups: Vec::new(),
            workspace_handles: HashMap::new(),
            _viewporter_state: smithay::wayland::viewporter::ViewporterState::new::<CompState>(&display_handle),
            _fractional_scale_state: smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<CompState>(&display_handle),
            _cursor_shape_state: smithay::wayland::cursor_shape::CursorShapeManagerState::new::<CompState>(&display_handle),
            idle_notifier_state: smithay::wayland::idle_notify::IdleNotifierState::new(&display_handle, event_loop.handle()),
            _idle_inhibit_manager_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<CompState>(&display_handle),
            idle_inhibiting_surfaces: Vec::new(),
            last_idle_notify: None,
            window_anims: HashMap::new(),
            last_broadcast_flags: HashMap::new(),
            last_broadcast_workspace: None,
            lock: Default::default(),
            cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
            cursor_buffers: crate::cursor::make_buffers(),
            last_titlebar_click: None,
            gesture_swipe: None,
            context_menu: None,
            context_menu_buffer: None,
            snap_flyout: None,
            snap_flyout_buffer: None,
            wm: wm.clone(),
            surface_to_id: HashMap::new(),
            id_to_window: HashMap::new(),
            dead_layer_surfaces: HashSet::new(),
            hidden_layer_surfaces: HashMap::new(),
            layer_surfaces_shown_once: HashSet::new(),
            decorations: HashMap::new(),
            border_top_decorations: HashMap::new(),
            border_bottom_decorations: HashMap::new(),
            decoration_signatures: HashMap::new(),
            shadow_buffers: HashMap::new(),
            rounded_corners_program: None,
            content_epoch: HashMap::new(),
            rounded_content_buffers: HashMap::new(),
            border_side_buffers: HashMap::new(),
            last_synced_size: HashMap::new(),
            pending: pending.clone(),
            bound_keys: Rc::new(bound_keys.iter().cloned().collect::<HashSet<_>>()),
            repeat_keys: Rc::new(repeat_keys.iter().cloned().collect::<HashSet<_>>()),
            repeat: None,
            start_time: Instant::now(),
            udev: Some(udev_state),
            xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState::new::<CompState>(&display_handle),
            xwm: None,
            xwayland_windows: HashMap::new(),
            xwayland_pending: Vec::new(),
            ewmh: None,
            appmenu_registrar: None,
        };

        let listener = ListeningSocket::bind_auto("wayland", 0..32).map_err(err)?;
        if let Some(name) = listener.socket_name() {
            std::env::set_var("WAYLAND_DISPLAY", name);
            log::info!("wayland socket: {}", name.to_string_lossy());
        }
        // Otherwise this is whatever the session inherited - typically
        // stale from a *previous* login's compositor (a shell's exported
        // `XDG_CURRENT_DESKTOP=Hyprland` surviving into this one), since
        // nothing else ever sets it. `xdg-desktop-portal` and any client
        // that sniffs this value to pick a desktop-specific integration
        // (screenshot/file-picker backends, etc.) get actively misrouted by
        // the stale value rather than just seeing "unknown". Only affects
        // processes spawned from here on (autostart, `srd.spawn`) - an
        // env var set mid-process doesn't retroactively reach anything
        // already running.
        std::env::set_var("XDG_CURRENT_DESKTOP", "srdwm");

        let ipc = match listener.socket_name().map(|n| n.to_string_lossy().into_owned()) {
            Some(name) => match srdwm_platform::IpcServer::bind(&name) {
                Ok(ipc) => Some(ipc),
                Err(e) => {
                    log::warn!("control socket unavailable ({e}); srd and scripts that use it won't work");
                    None
                }
            },
            None => None,
        };

        let handle = event_loop.handle();
        register_drm_fd(&handle, &card)?;
        register_libinput(&handle, &session, &seat_name)?;
        register_session_notifier(&handle, notifier)?;
        if let Err(e) = register_udev_monitor(&handle, &seat_name) {
            log::warn!("udev: connector hotplug unavailable ({e}); monitors are fixed at startup");
        }
        if let Err(e) = crate::xwayland::spawn(&handle, &display_handle) {
            log::warn!("XWayland unavailable ({e}); X11-only clients will not run");
        }

        Ok(Self { event_loop, display: dh, state, listener, clients: Vec::new(), pending, ipc })
    }

    fn accept_clients(&mut self) -> PlatformResult<()> {
        if let Some(stream) = self.listener.accept().map_err(err)? {
            let client = self.display.handle().insert_client(stream, std::sync::Arc::new(ClientState::default())).map_err(err)?;
            self.clients.push(client);
        }
        Ok(())
    }
}


impl Platform for UdevPlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::Wayland
    }

    fn poll_events(&mut self) -> PlatformResult<Vec<CoreEvent>> {
        self.accept_clients()?;
        self.event_loop.dispatch(Some(Duration::from_millis(16)), &mut self.state).map_err(err)?;
        // Held bindings that repeat - see `CompState::tick_repeat`.
        self.state.tick_repeat();
        self.display.dispatch_clients(&mut self.state).map_err(err)?;
        self.display.flush_clients().map_err(err)?;
        self.state.apply_registrar_events();
        if let Some(ipc) = self.ipc.as_mut() {
            if ipc.poll(&self.state.wm) {
                self.pending.borrow_mut().push(CoreEvent::WorkspaceChanged);
                // `ipc.rs`'s `handle_request` (`"focus"`, `"toggle
                // visibility"`, ...) only ever touches core's `WindowManager`
                // - it has no handle to `state.space`, which is what
                // actually renders on top *and* what `space.element_under`
                // hit-tests against (see `input::focus_window`'s own doc
                // comment, which fixed every *other* focus path this same
                // way). Left alone, a dock/AGS "focus" click over IPC moved
                // core's idea of focus while the window kept rendering, and
                // hit-testing, underneath whatever was already topmost --
                // reproduced live: `srd dispatch focus` on a covered Firefox
                // window raised it in the taskbar/keyboard sense but a
                // click at its own visible location still landed on the
                // window still actually on top. Re-syncing here rather than
                // in `ipc.rs` itself since core is platform-agnostic and
                // cannot see `state.space`; cheap and safe to call
                // unconditionally on any IPC mutation, not just ones that
                // are definitely focus changes - raising an already-topmost
                // element is a no-op reinsertion.
                let focused = self.state.wm.borrow().focused_id();
                if let Some(id) = focused {
                    crate::input::focus_window(&mut self.state, id);
                }
            }
        }
        // Starts srdwm's own lock UI if `srd dispatch lock` queued a
        // request since the last poll - see `WindowManager::request_lock`'s
        // own doc comment for why this crosses the core/backend boundary
        // as a drained request rather than a direct call. A no-op if
        // already locked (native or external), same guard `begin_native_
        // lock` applies itself.
        if self.state.wm.borrow_mut().drain_lock_request() {
            self.state.begin_native_lock();
        }
        // Same drained-request pattern as the lock check just above, for
        // `srd capture workspace` - see `WindowManager::request_capture_
        // workspace`'s own doc comment for why this needs the backend at
        // all rather than being answerable from core state.
        let capture_requests = self.state.wm.borrow_mut().drain_capture_requests();
        if !capture_requests.is_empty() {
            self.state.service_capture_requests(capture_requests);
        }
        // Checks whether a background PAM authentication spawned by a
        // native lock's own `Return` handling finished since the last
        // poll - see `native_lock.rs`'s module doc comment for why this
        // runs on a background thread rather than blocking here.
        self.state.poll_native_lock_auth();
        // Applies any `srd set_output_position` IPC requests queued since
        // the last poll - see `WindowManager::request_output_position`'s
        // own doc comment for why this indirection exists at all (core has
        // no real output handle to move itself). `id` is this head's index
        // into `udev.heads` *as of the platform's last `monitors()` query*
        // (see that function's own construction of `Monitor::new(i as u32,
        // ...)`) - stale if a hotplug reordered heads in between, same
        // trade-off `wlr-output-management-v1`'s own `apply_or_test`
        // guards against with a serial check. Not guarded the same way
        // here: this is a first pass at the primitive a display-settings
        // panel needs to build real monitor mirroring on top of, not yet
        // hardened against a hotplug racing an in-flight request - worth
        // adding if that turns out to matter in practice.
        let output_requests = self.state.wm.borrow_mut().drain_output_position_requests();
        if !output_requests.is_empty() {
            let mut any_applied = false;
            for (id, x, y) in output_requests {
                let Some(output) = self.state.udev.as_ref().and_then(|u| u.heads.get(id as usize)).map(|h| h.output.clone()) else {
                    log::warn!("udev: set_output_position: no head at index {id}");
                    continue;
                };
                crate::output_management::apply_output_position(&mut self.state, &output, (x, y).into());
                any_applied = true;
            }
            if any_applied {
                crate::output_management::broadcast_dirty_outputs(&mut self.state);
                // Core's own `Monitor` list is a passive mirror of whatever
                // the backend last reported (see `monitors()` above) --
                // without re-triggering a query, `Window.geometry`/
                // placement would keep using the pre-move rect until some
                // unrelated event happened to refresh it. `MonitorAdded`'s
                // payload is discarded unread on this path (`main.rs`
                // re-queries the full list rather than trusting it), same
                // as every other "just go recompute" use of this event
                // elsewhere in this codebase.
                self.pending.borrow_mut().push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
            }
        }
        self.state.render_udev_frame();
        Ok(self.pending.borrow_mut().drain(..).collect())
    }

    /// One `srdwm_core::Monitor` per head, positioned in the global space.
    /// This is what makes core's layout engine multi-monitor-aware in
    /// practice: `arrange_workspace` groups windows by `monitor` and lays
    /// each group out inside that monitor's rectangle.
    fn monitors(&mut self) -> PlatformResult<Vec<srdwm_core::Monitor>> {
        let Some(udev) = self.state.udev.as_ref() else { return Ok(Vec::new()) };
        Ok(udev
            .heads
            .iter()
            .enumerate()
            .map(|(i, head)| {
                // Shrunk by whatever a layer-shell surface (bar, dock) has
                // reserved via `set_exclusive_zone` - reporting the full
                // head size here otherwise means core's placement/tiling
                // treats that strip as ordinary free space, so a new
                // window's titlebar lands right where the bar renders on
                // top of it, unreachable to drag. `non_exclusive_zone()` is
                // output-local, so it's translated into this head's
                // position in the shared global space the same way
                // `head.location` already is.
                let zone = layer_map_for_output(&head.output).non_exclusive_zone();
                let rect = srdwm_core::Rect::new(
                    head.location.x + zone.loc.x,
                    head.location.y + zone.loc.y,
                    zone.size.w as u32,
                    zone.size.h as u32,
                );
                let mut m = srdwm_core::Monitor::new(i as u32, head.output.name(), rect);
                // `Monitor::new` defaults `full_geometry` to whatever
                // `geometry` was constructed with - correct for a monitor
                // with no layer-shell client at all, wrong the moment one
                // exists, since `rect` above is already zone-shrunk. Without
                // this, `full_geometry` was silently identical to `geometry`
                // for every real monitor this backend ever reported, which
                // made `toggle_fullscreen`'s whole "ignore the reserved
                // zone" design a no-op in practice: fullscreen still
                // stopped at the bar/dock exactly like maximize does.
                // Reported live as "fullscreen isn't actually going
                // fullscreen" - confirmed by triggering it and reading
                // the resulting geometry back over IPC, not just from
                // reading this code.
                m.full_geometry = srdwm_core::Rect::new(head.location.x, head.location.y, head.size.0 as u32, head.size.1 as u32);
                m.maximize_geometry = crate::input::maximize_geometry_for(&head.output, m.full_geometry);
                m.primary = i == 0;
                m
            })
            .collect())
    }

    fn apply_geometry(&mut self, window: srdwm_core::WindowId, _geometry: srdwm_core::Rect) -> PlatformResult<()> {
        self.state.sync_geometry(window);
        Ok(())
    }

    fn set_title(&mut self, _window: srdwm_core::WindowId, _title: &str) -> PlatformResult<()> {
        Ok(())
    }

    /// Was `wm.focus_window(window)` alone - core-only, so a caller that
    /// only has `Platform` to go through (`crates/platform`'s `IpcServer`,
    /// which can't reach `CompState`/real Wayland focus at all) could make
    /// a window *look* focused (rendering already reads live core state
    /// for the highlighted-border/titlebar-text colour) without it ever
    /// actually receiving a keystroke - confirmed live: `srd dispatch
    /// focus <xwayland-window-id>` changed core's own focused-window
    /// bookkeeping but left `_NET_ACTIVE_WINDOW` at `0x0` and real
    /// keyboard input going nowhere. `crate::input::focus_window` is the
    /// same full path a real mouse click already goes through.
    fn focus(&mut self, window: srdwm_core::WindowId) -> PlatformResult<()> {
        crate::input::focus_window(&mut self.state, window);
        Ok(())
    }

    fn minimize(&mut self, window: srdwm_core::WindowId) -> PlatformResult<()> {
        if let Some(w) = self.state.id_to_window.get(&window) {
            self.state.space.unmap_elem(w);
        }
        Ok(())
    }

    fn restore(&mut self, window: srdwm_core::WindowId) -> PlatformResult<()> {
        self.state.sync_geometry(window);
        Ok(())
    }

    fn close(&mut self, window: srdwm_core::WindowId) -> PlatformResult<()> {
        let Some(w) = self.state.id_to_window.get(&window) else { return Ok(()) };
        if let Some(toplevel) = w.toplevel() {
            toplevel.send_close();
        } else if let Some(x11) = w.x11_surface() {
            // `w.toplevel()` is `None` for an XWayland window - without
            // this arm, closing one (the WM's own close binding, or `srd
            // dispatch close`) silently did nothing at all. `close()` itself
            // handles both cases: a polite WM_DELETE_WINDOW for a
            // cooperating client, outright `destroy_window` for one that
            // doesn't support it.
            let _ = x11.close();
        }
        Ok(())
    }

    fn set_decorated(&mut self, _window: srdwm_core::WindowId, _decorated: bool) -> PlatformResult<()> {
        Ok(())
    }

    fn set_border_color(&mut self, _window: srdwm_core::WindowId, _rgb: (u8, u8, u8)) -> PlatformResult<()> {
        Ok(())
    }

    fn set_border_width(&mut self, _window: srdwm_core::WindowId, _width: u32) -> PlatformResult<()> {
        Ok(())
    }

    fn redraw_decoration(&mut self, window: srdwm_core::WindowId, _win: &srdwm_core::Window, _focused: bool) -> PlatformResult<()> {
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
