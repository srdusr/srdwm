use super::*;
impl WaylandPlatform {
    /// `bound_keys` are the config's `"Mod4+Shift+Return"`-style combo
    /// strings (see `srdwm_core::key_combo_string`) - the same set the X11
    /// backend grabs individually via `XGrabKey`. Only a keypress matching
    /// one of these is withheld from the focused client.
    pub fn connect(wm: Rc<RefCell<WindowManager>>, bound_keys: &[String], repeat_keys: &[String]) -> PlatformResult<Self> {
        let display: Display<CompState> = Display::new().map_err(err)?;
        let dh = display.handle();
        // See `idle_event_loop`'s own doc comment on `WaylandPlatform`.
        let idle_event_loop: CalloopEventLoop<'static, CompState> = CalloopEventLoop::try_new().map_err(err)?;

        let (mut backend, winit_events) = winit::init_from_attributes::<GlesRenderer>(
            WinitWindow::default_attributes()
                .with_inner_size(WinitLogicalSize::new(1280.0, 800.0))
                .with_title("srdwm")
                .with_visible(true),
        )
        .map_err(err)?;
        let size = backend.window_size();

        let output = Output::new(
            "srdwm-wayland".to_string(),
            PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "srdwm".into(), model: "winit".into() },
        );
        output.change_current_state(
            Some(OutputMode { size, refresh: 60_000 }),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output.create_global::<CompState>(&dh);
        // xdg-output (zxdg_output_manager_v1): several real layer-shell
        // clients (confirmed live: wofi 1.5.3) unconditionally call
        // `zxdg_output_manager_v1.get_xdg_output` while setting up a layer
        // surface and don't null-check the manager proxy if the global was
        // never advertised - so without this, those clients don't just
        // fail gracefully, they segfault. Real wlroots compositors (e.g.
        // Hyprland) always advertise it, which is why this only surfaced
        // once a real Wayland-native client could connect at all (see
        // `docs/IMPLEMENTATION_STATUS.md`).
        smithay::wayland::output::OutputManagerState::new_with_xdg_output::<CompState>(&dh);

        let compositor_state = CompositorState::new::<CompState>(&dh);
        let xdg_shell_state = XdgShellState::new::<CompState>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<CompState>(&dh);
        let shm_state = ShmState::new::<CompState>(&dh, Vec::new());
        // Order matters: data-control piggybacks on primary-selection's
        // client filter, so it needs the already-built state by reference.
        let primary_selection_state = PrimarySelectionState::new::<CompState>(&dh);
        let data_control_state = DataControlState::new::<CompState, _>(&dh, Some(&primary_selection_state), |_| true);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
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

        let mut space = Space::default();
        space.map_output(&output, (0, 0));

        // `zwp_linux_dmabuf_v1` - see `protocols.rs`'s `DmabufHandler` for
        // the udev-vs-winit split on eager import validation, and udev.rs's
        // matching global for why v3 (`create_global`) rather than v4's
        // feedback variant.
        let mut dmabuf_state = DmabufState::new();
        dmabuf_state.create_global::<CompState>(&dh, backend.renderer().dmabuf_formats());

        // See `state/mod.rs`'s `rounded_corners_program` doc comment: `None` on
        // any failure (an old/software GL driver missing something the
        // shader needs) rather than refusing to start over a cosmetic
        // feature - content just renders unrounded in that case.
        let rounded_corners_program = match crate::rounded_corners::compile(backend.renderer()) {
            Ok(program) => Some(program),
            Err(e) => {
                log::warn!("winit: rounded-corner shader failed to compile, content will render unrounded: {e}");
                None
            }
        };

        let pending = Rc::new(RefCell::new(Vec::new()));
        let state = CompState {
            compositor_state,
            xdg_shell_state,
            _xdg_decoration_state: xdg_decoration_state,
            shm_state,
            dmabuf_state,
            xdg_activation_state: XdgActivationState::new::<CompState>(&dh),
            _text_input_manager_state: smithay::wayland::text_input::TextInputManagerState::new::<CompState>(&dh),
            _input_method_manager_state: smithay::wayland::input_method::InputMethodManagerState::new::<CompState, _>(&dh, |_client| true),
            _gtk_shell_state: crate::gtk_shell::GtkShellState::new::<CompState>(&dh),
            seat_state,
            seat,
            space,
            popups: PopupManager::default(),
            // The nested backend is inherently one output: a single window on
            // the host. Multi-output is a udev/DRM concern.
            outputs: vec![OutputEntry { output: output.clone(), location: (0, 0).into() }],
            layer_shell_state: WlrLayerShellState::new::<CompState>(&dh),
            dh: dh.clone(),
            data_device_state: DataDeviceState::new::<CompState>(&dh),
            primary_selection_state,
            data_control_state,
            session_lock_state: SessionLockManagerState::new::<CompState, _>(&dh, |_| true),
            _screencopy_state: screencopy::ScreencopyState::new::<CompState>(&dh),
            screencopy_pending: Vec::new(),
            _foreign_toplevel_state: crate::foreign_toplevel::ForeignToplevelState::new::<CompState>(&dh),
            foreign_toplevel_managers: Vec::new(),
            foreign_toplevel_handles: HashMap::new(),
            _workspace_state: crate::workspace::WorkspaceManagerState::new::<CompState>(&dh),
            _output_power_state: None,
            _gamma_control_state: None,
            _output_management_state: crate::output_management::OutputManagementState::new::<CompState>(&dh),
            output_managers: Vec::new(),
            output_heads: HashMap::new(),
            output_modes: HashMap::new(),
            output_serial: 0,
            last_broadcast_outputs: Vec::new(),
            workspace_managers: Vec::new(),
            workspace_groups: Vec::new(),
            workspace_handles: HashMap::new(),
            _viewporter_state: smithay::wayland::viewporter::ViewporterState::new::<CompState>(&dh),
            _fractional_scale_state: smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<CompState>(&dh),
            _cursor_shape_state: smithay::wayland::cursor_shape::CursorShapeManagerState::new::<CompState>(&dh),
            idle_notifier_state: smithay::wayland::idle_notify::IdleNotifierState::new(&dh, idle_event_loop.handle()),
            _idle_inhibit_manager_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<CompState>(&dh),
            idle_inhibiting_surfaces: Vec::new(),
            last_idle_notify: None,
            window_anims: HashMap::new(),
            last_broadcast_flags: HashMap::new(),
            last_broadcast_workspace: None,
            lock: SessionLock::default(),
            cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
            cursor_buffers: crate::cursor::make_buffers(),
            last_titlebar_click: None,
            context_menu: None,
            context_menu_buffer: None,
            wm: wm.clone(),
            surface_to_id: HashMap::new(),
            id_to_window: HashMap::new(),
            dead_layer_surfaces: HashSet::new(),
            decorations: HashMap::new(),
            border_top_decorations: HashMap::new(),
            shadow_buffers: HashMap::new(),
            rounded_corners_program,
            content_epoch: HashMap::new(),
            rounded_content_buffers: HashMap::new(),
            border_side_buffers: HashMap::new(),
            last_synced_size: HashMap::new(),
            pending: pending.clone(),
            bound_keys: Rc::new(bound_keys.iter().cloned().collect()),
            repeat_keys: Rc::new(repeat_keys.iter().cloned().collect()),
            repeat: None,
            start_time: Instant::now(),
            udev: None,
            xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState::new::<CompState>(&dh),
            xwm: None,
            xwayland_windows: HashMap::new(),
            xwayland_pending: Vec::new(),
            ewmh: None,
        };

        let listener = ListeningSocket::bind_auto("wayland", 0..32).map_err(err)?;
        if let Some(name) = listener.socket_name() {
            std::env::set_var("WAYLAND_DISPLAY", name);
            log::info!("wayland socket: {}", name.to_string_lossy());
        }

        let damage_tracker = OutputDamageTracker::from_output(&output);

        let ipc = match listener.socket_name().map(|n| n.to_string_lossy().into_owned()) {
            Some(name) => match IpcServer::bind(&name) {
                Ok(ipc) => Some(ipc),
                Err(e) => {
                    log::warn!("control socket unavailable ({e}); srd and scripts that use it won't work");
                    None
                }
            },
            None => None,
        };

        Ok(Self {
            display,
            state,
            backend,
            winit_events,
            damage_tracker,
            output,
            listener,
            clients: Vec::new(),
            pending,
            wm,
            ipc,
            idle_event_loop,
            last_frame: Instant::now(),
        })
    }
}
