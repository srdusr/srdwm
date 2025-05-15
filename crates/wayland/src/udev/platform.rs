use super::*;
use super::drm::{bring_up_head, pick_crtc, probe_connected};
use super::session::{register_drm_fd, register_gpu_drm_notifier, register_libinput, register_session_notifier, register_udev_monitor};

pub struct UdevPlatform {
    event_loop: EventLoop<'static, CompState>,
    display: Display<CompState>,
    state: CompState,
    listener: ListeningSocket,
    clients: Vec<Client>,
    pending: Rc<RefCell<Vec<CoreEvent>>>,
    ipc: Option<srdwm_platform::IpcServer>,
    /// Last time `ipc.poll()` actually ran - see its call site in
    /// `poll_events` for why this exists at all.
    last_ipc_poll: Instant,
    /// Last time the unconditional end-of-cycle `render_udev_frame()` call
    /// actually ran - see its own call site for why.
    last_render: Instant,
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
        // Opt-in only (`SRDWM_GPU=1`, unset by default) - see `gpu::probe`'s
        // own doc comment for exactly what this does and does not do yet.
        // A no-op unless that variable is set, so this line changes nothing
        // about any session that doesn't set it. `gpu_notifier` is
        // registered as its own calloop event source further down
        // (alongside `register_drm_fd`'s own registration for the
        // existing legacy heads); `gpu_context` is stored on `UdevState`
        // below and consulted by `render_udev_frame`.
        let (mut gpu_context, gpu_notifier) = match super::gpu::probe(&card) {
            Some((ctx, notifier)) => (Some(ctx), Some(notifier)),
            None => (None, None),
        };

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
        // Two accumulators - see `bring_up_head`'s own doc comment on its
        // `logical_x` parameter for why a second head's logical position
        // can't just be derived from the physical offset and its own
        // scale alone once an earlier head has a *different* scale.
        let mut x_offset = 0;
        let mut logical_x = 0;
        for probe in &connected {
            let Some(crtc) = pick_crtc(&card, probe, &used_crtcs) else {
                log::warn!("udev: no free CRTC left for connector {}; not driving it", probe.name);
                continue;
            };
            let scale = wm.borrow().monitor_scale(&probe.name);
            let (head, entry) = bring_up_head(&card, &display_handle, probe, crtc, x_offset, logical_x, scale)?;
            log::info!("udev: head {}: {} {}x{} at x={x_offset} (logical x={logical_x})", heads.len(), probe.name, head.size.0, head.size.1);
            used_crtcs.push(crtc);
            let resolved_scale = head.output.current_scale().fractional_scale();
            x_offset += head.size.0;
            logical_x += (head.size.0 as f64 / resolved_scale).round() as i32;
            // Every connected head gets a chance at the GPU path, not just
            // the first - `DrmOutputManager` already supports driving
            // several crtcs at once (`GpuContext::outputs`' own doc
            // comment), Phase 2 simply never called this more than once.
            // A no-op whenever `gpu_context` is `None` (every session that
            // doesn't set `SRDWM_GPU=1`, or where `gpu::probe` itself
            // failed). A head this fails for individually (logged inside
            // `initialize_output`) just falls back to the legacy Pixman
            // path below, same as before - this loop doesn't need to know
            // which outcome happened.
            if let Some(ctx) = gpu_context.as_mut() {
                ctx.initialize_output(head.crtc, head.mode, head.connector, &head.output);
            }
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
            disabled_connectors: std::collections::HashSet::new(),
            last_rendered_workspace: None,
            last_rendered_layout: None,
            gpu: gpu_context,
        };

        let mut state = CompState {
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
            _virtual_keyboard_state: smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState::new::<CompState, _>(&display_handle, |_client| true),
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
            pointer_button_grab: None,
            pointer_buttons_held: 0,
            window_anims: HashMap::new(),
            last_broadcast_flags: HashMap::new(),
            last_broadcast_workspace: None,
            lock: Default::default(),
            cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
            decoration_cursor_active: false,
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
            hovered_titlebar_button: None,
            shadow_buffers: HashMap::new(),
            rounded_corners_program: None,
            content_epoch: HashMap::new(),
            rounded_content_buffers: HashMap::new(),
            border_side_buffers: HashMap::new(),
            color_filter_buffers: HashMap::new(),
            last_synced_size: HashMap::new(),
            pending_size_configure: HashMap::new(),
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

        // Before the Wayland socket even binds, deliberately - see
        // `restore_monitor_layout`'s and `monitor_layout`'s own doc
        // comments for why this compositor restores its own remembered
        // layout itself rather than leaving it to whichever panel happens
        // to be running: no client can possibly connect and see the
        // default, un-restored arrangement, not even for one frame, since
        // the socket a client would need to connect to doesn't exist yet.
        state.restore_monitor_layout();

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
        // Only when `SRDWM_GPU=1` and `gpu::probe` succeeded - see
        // `register_gpu_drm_notifier`'s own doc comment. A failure here
        // (this specific registration, not the probe itself) is logged,
        // not fatal: the GPU head just never gets a `frame_submitted()`
        // call and its swapchain eventually stalls, no worse than the
        // probe never having succeeded at all.
        if let Some(gpu_notifier) = gpu_notifier {
            if let Err(e) = register_gpu_drm_notifier(&handle, gpu_notifier) {
                log::warn!("udev: SRDWM_GPU=1 but failed to register the GPU DRM notifier: {e}");
            }
        }
        let libinput_handle = register_libinput(&handle, &session, &seat_name)?;
        register_session_notifier(&handle, notifier, libinput_handle)?;
        if let Err(e) = register_udev_monitor(&handle, &seat_name) {
            log::warn!("udev: connector hotplug unavailable ({e}); monitors are fixed at startup");
        }
        if let Err(e) = crate::xwayland::spawn(&handle, &display_handle) {
            log::warn!("XWayland unavailable ({e}); X11-only clients will not run");
        }

        Ok(Self { event_loop, display: dh, state, listener, clients: Vec::new(), pending, ipc, last_ipc_poll: Instant::now(), last_render: Instant::now() })
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
        let dispatch_start = Instant::now();
        self.event_loop.dispatch(Some(Duration::from_millis(16)), &mut self.state).map_err(err)?;
        // `dispatch`'s `Duration::from_millis(16)` argument is a *maximum*
        // wait, not a guarantee - calloop returns the moment any
        // registered source looks ready, however long or short that takes.
        // A source stuck permanently "ready" (an fd calloop never removes
        // even though every read on it comes back EOF/HUP - confirmed live
        // via `strace`, traced to the libseat session notifier's internal
        // ping channel, and reproducible on a bare tty1 login within the
        // first second of every single srdwm start, independent of which
        // libseat backend - seatd or the logind fallback - is active)
        // makes `dispatch` return in microseconds forever, turning this
        // loop into an unthrottled spin that burns 70-90% of a core doing
        // nothing: `accept_clients`/`tick_repeat`/`dispatch_clients` all
        // still run their own (cheap) work on every single one of those
        // spurious wakeups, thousands of times a second, instead of the
        // ~60 times a second the 16ms figure was meant to cap it at.
        //
        // This doesn't fix *why* that source never goes away - that's
        // upstream, in calloop/libseat's own channel-notification internals
        // - but it puts a floor under the symptom regardless of which
        // source eventually turns out to cause it.
        //
        // Sleeping the full remainder of a 16ms cycle on *every* fast
        // return (an earlier version of this did exactly that) blocks this
        // thread against everything, not just the next spurious wakeup --
        // a genuine DRM page-flip completion or a client committing its
        // next video frame that becomes ready *during* the sleep sits
        // unprocessed until the sleep ends, instead of being picked up
        // immediately. Reported live as choppy/laggy video playback: up to
        // 16ms of pure, avoidable latency added to every frame's worth of
        // real work that happened to land in that window.
        //
        // A per-iteration streak counter was tried first, throttling only
        // once several fast returns in a row looked like true idle
        // spinning rather than one-off real work - but `dispatch`'s
        // return time can't actually distinguish the two here: the dead
        // pipe is *always* ready, so every call returns in microseconds
        // whether or not it also picked up something real, and a streak
        // built on that timing never resets during genuine activity
        // either. Telling real work apart from the spurious wakeup would
        // need a signal from *inside* dispatch (e.g. the render path
        // flagging "a frame actually went out this tick"), which is real
        // plumbing, not a one-line fix.
        //
        // Short of that: cap the sleep itself far below 16ms instead of
        // trying to skip it selectively. `MIN_CYCLE` (~3ms) still turns
        // the true spin (unbounded, thousands of empty iterations/sec)
        // into a bounded few hundred/sec - a real, if smaller, win over
        // no floor at all - while capping how long any genuinely-ready
        // event can ever sit blocked to something well under one frame at
        // 60Hz, rather than up to a full frame's worth of latency.
        const MIN_CYCLE: Duration = Duration::from_millis(3);
        let elapsed = dispatch_start.elapsed();
        if elapsed < MIN_CYCLE {
            std::thread::sleep(MIN_CYCLE - elapsed);
        }
        // Held bindings that repeat - see `CompState::tick_repeat`.
        self.state.tick_repeat();
        self.display.dispatch_clients(&mut self.state).map_err(err)?;
        self.display.flush_clients().map_err(err)?;
        self.state.apply_registrar_events();
        self.state.poll_global_menu_properties();
        // Throttled to ~60Hz, not run on every single `poll_events` cycle --
        // `IpcServer::poll` unconditionally rebuilds and diffs a full
        // `client_snapshot`/`workspace_snapshot` on every call (cloning each
        // window's title, app_id, global-menu data, ...) even when nothing
        // has changed and nobody is subscribed, purely so a real change is
        // never missed. Cheap at a sane call rate; not cheap at the rate
        // this loop actually runs at - see `MIN_CYCLE`'s own doc comment
        // just above: the dead libseat pipe that makes `dispatch` return in
        // microseconds forever means this whole function's "rest of the
        // cycle" work already runs at whatever `dispatch` gets bounced to
        // (a few hundred times a second, floor-capped by `MIN_CYCLE`, not
        // the ~60 times a second one `Duration::from_millis(16)` above was
        // meant to imply), and that snapshot/diff cost was riding along at
        // that same needlessly high rate - measured live as a continuous,
        // unwavering ~20% of a core even at complete idle, unaffected by
        // toggling shadows/rounded_corners/animations (all purely per-
        // render-frame costs, not per-cycle ones, so none of them could
        // have explained a cost that never budged with the screen doing
        // nothing). A real `srd dispatch`/`srd set` command still lands
        // within one throttled window (well under a human's own reaction
        // time), not delayed by anything close to what would read as
        // input lag.
        const IPC_POLL_INTERVAL: Duration = Duration::from_millis(16);
        let ipc_due = self.last_ipc_poll.elapsed() >= IPC_POLL_INTERVAL;
        if ipc_due {
            self.last_ipc_poll = Instant::now();
        }
        if let Some(ipc) = self.ipc.as_mut().filter(|_| ipc_due) {
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
                //
                // `raise_in_space`, not the full `focus_window` - that one
                // also re-runs `WindowManager::focus_window`'s workspace-
                // follow side effect on the already-focused window, which
                // silently reverted any `activate_workspace` IPC dispatch
                // within this same cycle (see `raise_in_space`'s own doc
                // comment for the full story).
                let focused = self.state.wm.borrow().focused_id();
                if let Some(id) = focused {
                    crate::input::raise_in_space(&mut self.state, id);
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
                // `(x, y)` is whatever `srd dispatch set output position`
                // sent, unconverted - that command's own contract is to
                // match `srd monitors`' `full_x`/`full_y` (physical),
                // which is exactly what `apply_output_position` wants.
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
        // Applies any `srd set_output_enabled` IPC requests queued since
        // the last poll - `disable_connector_by_name`/`enable_connector_
        // by_name` already push their own `MonitorRemoved`/`MonitorAdded`
        // event, so nothing further is needed here beyond calling them.
        let enable_requests = self.state.wm.borrow_mut().drain_output_enable_requests();
        for (name, enabled) in enable_requests {
            if enabled {
                self.state.enable_connector_by_name(&name);
            } else {
                self.state.disable_connector_by_name(&name);
            }
        }
        // Throttled the same way and for the same underlying reason as the
        // `ipc.poll()` call above - this is the *other*, larger half of
        // this cycle's needless work at the dead-pipe-driven spin rate.
        // `render_udev_frame` isn't only called from here: a real DRM
        // page-flip completion (`session.rs`), a VT-switch resume, and an
        // output hotplug each call it directly, immediately, completely
        // unthrottled by this - those are genuine, comparatively rare
        // events that should redraw the instant they happen. This one
        // specific call site is different: it's the unconditional catch-
        // all that used to run at the end of *every* cycle regardless of
        // whether `dispatch` actually picked up anything real, which at
        // this loop's dead-pipe-driven rate meant re-walking every visible
        // window, rebuilding the whole `custom_elements` list, and running
        // Pixman's own damage tracking against it a few hundred times a
        // second, forever - `has_damage` already meant an idle desktop's
        // *page flip* was skipped, but computing "no, still nothing to
        // flip" this often is itself most of the cost this whole function
        // was found burning at idle. `RENDER_INTERVAL` (~8ms, ~120Hz) is
        // comfortably above any real display's refresh rate - a head can
        // never actually present faster than its own vblank allows
        // regardless (`flip_pending` already gates that) - so this cannot
        // cap real, on-screen frame rate on any hardware this backend
        // targets; it only stops the redundant "check again" calls in
        // between.
        const RENDER_INTERVAL: Duration = Duration::from_millis(8);
        if self.last_render.elapsed() >= RENDER_INTERVAL {
            self.last_render = Instant::now();
            self.state.render_udev_frame();
        }
        Ok(self.pending.borrow_mut().drain(..).collect())
    }

    /// One `srdwm_core::Monitor` per head, positioned in the global space
    /// - or several, when `srd.monitor.split` has requested that head be
    /// divided into logical sub-monitors ("monitors inside monitors"; see
    /// `srdwm_core::monitor::MonitorSplit`'s own doc comment). This is
    /// what makes core's layout engine multi-monitor-aware in practice:
    /// `arrange_workspace` groups windows by `monitor` and lays each group
    /// out inside that monitor's rectangle - a split just means more,
    /// smaller rectangles feeding the same grouping, no other core-side
    /// change needed.
    fn monitors(&mut self) -> PlatformResult<Vec<srdwm_core::Monitor>> {
        let Some(udev) = self.state.udev.as_ref() else { return Ok(Vec::new()) };
        let wm = self.state.wm.clone();
        let wm = wm.borrow();
        let mut out = Vec::new();
        let mut next_id: u32 = 0;
        for head in udev.heads.iter() {
            // Shrunk by whatever a layer-shell surface (bar, dock) has
            // reserved via `set_exclusive_zone` - reporting the full
            // head size here otherwise means core's placement/tiling
            // treats that strip as ordinary free space, so a new
            // window's titlebar lands right where the bar renders on
            // top of it, unreachable to drag. `non_exclusive_zone()` is
            // output-local, so it's translated into this head's
            // position in the shared global space the same way
            // `head.location` already is.
            //
            // `non_exclusive_zone()` is in *logical* (scale-divided)
            // units - a bar reports its own reserved strip the way every
            // layer-shell client does, in logical points - while `head.
            // location`/`head.size` are raw physical pixels straight from
            // the DRM mode, never touched by `srd.monitor.scale`. Left
            // unconverted, `usable` silently mixed the two units on any
            // output with a scale other than exactly `1.0`: at scale
            // `0.712`, a 1920-physical-pixel-wide head's own `zone.size.w`
            // came back as ~2697 (logical), reported as this monitor's
            // *usable* width - larger than its own *full* width, and
            // large enough to overlap whichever real monitor sat next to
            // it in the shared global space. Reported live as "Firefox
            // maximized on one monitor also shows partially on the
            // other" and general visual glitching on the scaled output --
            // both are this: placement math trusting an oversized rect
            // that reached into a neighboring monitor's real screen.
            // Scaling `zone` back into physical pixels here keeps `usable`
            // in the same unit as `full`/`maximize`/`head.location`
            // everywhere else in this compositor.
            let zone = layer_map_for_output(&head.output).non_exclusive_zone();
            let scale = head.output.current_scale().fractional_scale();
            let zone_physical = |v: i32| (v as f64 * scale).round() as i32;
            let usable = srdwm_core::Rect::new(
                head.location.x + zone_physical(zone.loc.x),
                head.location.y + zone_physical(zone.loc.y),
                zone_physical(zone.size.w).max(0) as u32,
                zone_physical(zone.size.h).max(0) as u32,
            );
            // The head's true full rect, ignoring any exclusive zone --
            // deliberately *not* defaulted from `usable` the way `Monitor::
            // new` alone would (see the fullscreen note below).
            let full = srdwm_core::Rect::new(head.location.x, head.location.y, head.size.0 as u32, head.size.1 as u32);
            let maximize = crate::input::maximize_geometry_for(&head.output, full);
            let name = head.output.name();
            let split = wm.monitor_split(&name);
            let parts = split.map(|s| s.parts).unwrap_or(1).max(1);
            let rows = split.map(|s| s.rows).unwrap_or(false);
            for part in 0..parts {
                let sub_name = if parts <= 1 { name.clone() } else { format!("{name}-{}", part + 1) };
                let mut m = srdwm_core::Monitor::new(next_id, sub_name, srdwm_core::monitor::split_rect(usable, part, parts, rows));
                // `Monitor::new` defaults `full_geometry`/`maximize_
                // geometry` to whatever `geometry` was constructed with --
                // correct for a monitor with no layer-shell client and no
                // split at all, wrong the moment either exists, since the
                // rect above may already be zone-shrunk and/or a sub-
                // region. Without this, `full_geometry` was silently
                // identical to `geometry` for every real monitor this
                // backend ever reported, which made `toggle_fullscreen`'s
                // whole "ignore the reserved zone" design a no-op in
                // practice: fullscreen still stopped at the bar/dock
                // exactly like maximize does. Reported live as "fullscreen
                // isn't actually going fullscreen" - confirmed by
                // triggering it and reading the resulting geometry back
                // over IPC, not just from reading this code. Each split
                // part gets its *own* full/maximize rect too - without
                // this, fullscreening a window in either half of a split
                // head would cover the *entire* physical panel, silently
                // erasing the split it was placed to respect.
                m.full_geometry = srdwm_core::monitor::split_rect(full, part, parts, rows);
                m.maximize_geometry = srdwm_core::monitor::split_rect(maximize, part, parts, rows);
                m.primary = next_id == 0;
                m.split = parts > 1;
                m.scale = scale;
                out.push(m);
                next_id += 1;
            }
        }
        Ok(out)
    }

    fn apply_geometry(&mut self, window: srdwm_core::WindowId, _geometry: srdwm_core::Rect) -> PlatformResult<()> {
        self.state.sync_geometry(window);
        // `redraw_decoration_buffer` sizes the cached border-strip/titlebar
        // bitmaps from `effective_frame`, which (see that function's own
        // doc comment) can differ from `w.geometry` alone once a CSD
        // client's own invisible shadow margin enters the picture. Without
        // this, the bitmap stays sized from whatever it was last built at
        // - correct right up until this specific call changes `w.geometry`
        // (`toggle_maximize`/`apply_snap_zone`, the two core-side callers of
        // this callback) - and the *next* rebuild only happens whenever
        // this window's own client next commits (`protocols/compositor.rs`'s
        // per-commit call) or something else unrelated triggers one, not
        // reliably right away. Confirmed live: maximizing then restoring a
        // Chrome window left its border strips sized for the *maximized*
        // frame while its real content had already settled back to the
        // smaller restored size, immediately and permanently until some
        // later unrelated trigger (a fresh commit) happened to catch it up
        // - a real, visible gap between content and border on the far
        // edges, not the half-pixel seam `blend_corner_pixel`'s own fix
        // addressed.
        self.state.redraw_decoration_buffer(window);
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
        // See `apply_geometry`'s own doc comment - same gap, same fix.
        self.state.redraw_decoration_buffer(window);
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
