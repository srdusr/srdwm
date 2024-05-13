//! Nested ("winit") backend: srdwm as a window on an existing
//! compositor/X server, the Wayland analogue of running the X11 backend
//! under Xephyr. Used for development and for the case where srdwm is
//! started from inside another session; the real bare-TTY path is
//! [`crate::udev`].
//!
//! Both backends share all protocol state ([`crate::state::CompState`]),
//! input routing ([`crate::input`]) and lock behaviour ([`crate::lock`]);
//! what differs is only how a frame reaches a screen.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{AbsolutePositionEvent, ButtonState as BackendButtonState, Event as InputEventTrait, InputEvent, PointerButtonEvent};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend};
use smithay::desktop::space::render_output;
use smithay::desktop::{layer_map_for_output, Space};
use smithay::input::SeatState;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::{Client, Display, ListeningSocket};
use smithay::reexports::winit::platform::pump_events::PumpStatus;
use smithay::utils::Transform;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;

use srdwm_core::{Event as CoreEvent, Window as CoreWindow, WindowId, WindowManager};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};

use crate::input::{handle_keyboard_key_event, handle_pointer_button, handle_pointer_position, last_pointer_pos};
use crate::lock::{lock_render_elements, send_lock_frame};
use crate::lock::SessionLock;
use crate::state::{ClientState, CompState, OutputEntry};
use crate::{err, screencopy};

pub struct WaylandPlatform {
    display: Display<CompState>,
    state: CompState,
    backend: WinitGraphicsBackend<GlesRenderer>,
    winit_events: WinitEventLoop,
    damage_tracker: OutputDamageTracker,
    output: Output,
    listener: ListeningSocket,
    clients: Vec<Client>,
    pending: Rc<RefCell<Vec<CoreEvent>>>,
    wm: Rc<RefCell<WindowManager>>,
}

impl WaylandPlatform {
    /// `bound_keys` are the config's `"Mod4+Shift+Return"`-style combo
    /// strings (see `srdwm_core::key_combo_string`) - the same set the X11
    /// backend grabs individually via `XGrabKey`. Only a keypress matching
    /// one of these is withheld from the focused client.
    pub fn connect(wm: Rc<RefCell<WindowManager>>, bound_keys: &[String]) -> PlatformResult<Self> {
        let display: Display<CompState> = Display::new().map_err(err)?;
        let dh = display.handle();

        let (backend, winit_events) = winit::init::<GlesRenderer>().map_err(err)?;
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
        seat.add_keyboard(Default::default(), 200, 25).map_err(err)?;
        seat.add_pointer();

        let mut space = Space::default();
        space.map_output(&output, (0, 0));

        let pending = Rc::new(RefCell::new(Vec::new()));
        let state = CompState {
            compositor_state,
            xdg_shell_state,
            _xdg_decoration_state: xdg_decoration_state,
            shm_state,
            seat_state,
            seat,
            space,
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
            lock: SessionLock::default(),
            cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
            cursor_buffer: crate::cursor::make_buffer(),
            wm: wm.clone(),
            surface_to_id: HashMap::new(),
            id_to_window: HashMap::new(),
            decorations: HashMap::new(),
            pending: pending.clone(),
            bound_keys: Rc::new(bound_keys.iter().cloned().collect()),
            start_time: Instant::now(),
            udev: None,
            xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState::new::<CompState>(&dh),
            xwm: None,
            xwayland_windows: HashMap::new(),
            xwayland_pending: Vec::new(),
        };

        let listener = ListeningSocket::bind_auto("wayland", 0..32).map_err(err)?;
        if let Some(name) = listener.socket_name() {
            std::env::set_var("WAYLAND_DISPLAY", name);
            log::info!("wayland socket: {}", name.to_string_lossy());
        }

        let damage_tracker = OutputDamageTracker::from_output(&output);

        Ok(Self { display, state, backend, winit_events, damage_tracker, output, listener, clients: Vec::new(), pending, wm })
    }

    fn accept_clients(&mut self) -> PlatformResult<()> {
        if let Some(stream) = self.listener.accept().map_err(err)? {
            let client = self.display.handle().insert_client(stream, std::sync::Arc::new(ClientState::default())).map_err(err)?;
            self.clients.push(client);
        }
        Ok(())
    }

    fn pump_winit(&mut self) -> PlatformResult<bool> {
        let mut closed = false;
        let state = &mut self.state;
        let output = &self.output;
        let pump = self.winit_events.dispatch_new_events(|event| {
            handle_winit_event(state, output, event, &mut closed);
        });
        if matches!(pump, PumpStatus::Exit(_)) {
            closed = true;
        }
        Ok(closed)
    }

    fn render_frame(&mut self) -> PlatformResult<()> {
        let size = self.backend.window_size();
        let resized = self.output.current_mode().map(|m| m.size) != Some(size);
        if resized {
            // Only push a new output mode - and thus emit `wl_output.mode`/
            // `done` - when the size actually changed. This used to run
            // unconditionally every frame; harmless with no Wayland-native
            // client connected (the only way this was ever exercised before
            // real layer-shell clients existed), but a real client bound to
            // `wl_output` would otherwise be flooded with duplicate
            // mode/done events at the render loop's full frame rate.
            self.output.change_current_state(Some(OutputMode { size, refresh: 60_000 }), None, None, None);
            layer_map_for_output(&self.output).arrange();
        }

        let age = self.backend.buffer_age().unwrap_or(0);
        let (renderer, mut framebuffer) = self.backend.bind().map_err(err)?;

        // Locked: the lock surface over an opaque black clear, and nothing
        // else - no windows, no decorations, no layer surfaces.
        if self.state.lock.locked {
            let lock_surface = self.state.lock_surface_for(&self.output).cloned();
            let elements = lock_render_elements(lock_surface.as_ref(), renderer);
            self.damage_tracker
                .render_output(renderer, &mut framebuffer, age, &elements, [0.0, 0.0, 0.0, 1.0])
                .map_err(err)?;
            drop(framebuffer);
            self.backend.submit(None).map_err(err)?;
            send_lock_frame(lock_surface.as_ref(), &self.output, self.state.start_time.elapsed());
            self.state.confirm_lock_if_presented(&self.output);
            screencopy::fail_pending(std::mem::take(&mut self.state.screencopy_pending));
            return Ok(());
        }

        let mut custom_elements: Vec<MemoryRenderBufferRenderElement<GlesRenderer>> = Vec::new();
        for (&id, deco) in self.state.decorations.iter() {
            let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) else { continue };
            match MemoryRenderBufferRenderElement::from_buffer(renderer, (geom.x as f64, geom.y as f64), deco, None, None, None, Kind::Unspecified) {
                Ok(elem) => custom_elements.push(elem),
                Err(e) => log::warn!("failed to import titlebar buffer for window {id}: {e}"),
            }
        }

        render_output(
            &self.output,
            renderer,
            &mut framebuffer,
            1.0,
            age,
            [&self.state.space],
            &custom_elements,
            &mut self.damage_tracker,
            [0.05, 0.05, 0.08, 1.0],
        )
        .map_err(err)?;
        drop(framebuffer);
        self.backend.submit(None).map_err(err)?;
        self.state.space.elements().for_each(|w| w.send_frame(&self.output, self.state.start_time.elapsed(), None, |_, _| Some(self.output.clone())));

        // Screencopy is serviced *after* the on-screen frame is submitted,
        // into its own offscreen buffer - never by reading back the window
        // surface. Reading the winit backend's EGL window surface (what an
        // earlier version did) reliably killed the GL context: the first
        // `grim` capture produced `eglSwapBuffers: BAD_SURFACE` followed by
        // `BAD_ALLOC` and "context has been lost", confirmed by A/B-ing the
        // same build with only the readback call removed. The offscreen
        // detour costs a second scene render, but only on frames where a
        // capture was actually requested.
        //
        // Deliberately placed after the locked-session early return above,
        // so a capture requested while the screen is locked can never see
        // client content.
        let captures = std::mem::take(&mut self.state.screencopy_pending);
        if !captures.is_empty() {
            if let Err(e) = self.capture_offscreen(captures) {
                log::warn!("screencopy: offscreen capture pass failed: {e}");
            }
        }
        Ok(())
    }

    /// Re-renders the current scene into an offscreen GLES renderbuffer and
    /// serves the queued screencopy captures from it. See the call site for
    /// why capture cannot read the window surface directly.
    fn capture_offscreen(&mut self, captures: Vec<screencopy::PendingCapture>) -> PlatformResult<()> {
        use smithay::backend::renderer::{Bind, Offscreen};

        let size = self.output.current_mode().map(|m| m.size).unwrap_or_default();
        if size.w <= 0 || size.h <= 0 {
            return Ok(());
        }
        let renderer = self.backend.renderer();
        let mut target: smithay::backend::renderer::gles::GlesRenderbuffer =
            renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into()).map_err(err)?;
        let mut framebuffer = renderer.bind(&mut target).map_err(err)?;

        let mut custom_elements: Vec<MemoryRenderBufferRenderElement<GlesRenderer>> = Vec::new();
        for (&id, deco) in self.state.decorations.iter() {
            let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) else { continue };
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(renderer, (geom.x as f64, geom.y as f64), deco, None, None, None, Kind::Unspecified) {
                custom_elements.push(elem);
            }
        }

        // A throwaway damage tracker, so this pass always draws the whole
        // scene (age 0) and never perturbs the on-screen tracker's history.
        let mut tracker = OutputDamageTracker::from_output(&self.output);
        render_output(
            &self.output,
            renderer,
            &mut framebuffer,
            1.0,
            0,
            [&self.state.space],
            &custom_elements,
            &mut tracker,
            [0.05, 0.05, 0.08, 1.0],
        )
        .map_err(err)?;

        screencopy::service_pending(captures, renderer, &framebuffer);
        Ok(())
    }
}

fn handle_winit_event(state: &mut CompState, output: &Output, event: WinitEvent, closed: &mut bool) {
    match event {
        WinitEvent::CloseRequested => *closed = true,
        WinitEvent::Input(InputEvent::Keyboard { event }) => handle_keyboard_key_event(state, &event),
        WinitEvent::Input(InputEvent::PointerMotionAbsolute { event }) => {
            let size = output.current_mode().map(|m| m.size).unwrap_or_default().to_logical(1);
            let pos = event.position_transformed(size);
            handle_pointer_position(state, pos, event.time_msec());
        }
        WinitEvent::Input(InputEvent::PointerButton { event }) => {
            let pos = last_pointer_pos(state);
            let button = event.button_code();
            let pressed = event.state() == BackendButtonState::Pressed;
            handle_pointer_button(state, pos, button, pressed, event.time_msec());
        }
        WinitEvent::Resized { .. } => {}
        _ => {}
    }
}

impl Platform for WaylandPlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::Wayland
    }

    fn poll_events(&mut self) -> PlatformResult<Vec<CoreEvent>> {
        self.accept_clients()?;
        let closed = self.pump_winit()?;
        if closed {
            return Err(PlatformError::Other("compositor window closed".into()));
        }
        self.display.dispatch_clients(&mut self.state).map_err(err)?;
        self.display.flush_clients().map_err(err)?;
        self.render_frame()?;
        Ok(self.pending.borrow_mut().drain(..).collect())
    }

    fn monitors(&mut self) -> PlatformResult<Vec<srdwm_core::Monitor>> {
        let size = self.backend.window_size();
        Ok(vec![{
            let mut m = srdwm_core::Monitor::new(0, "winit", srdwm_core::Rect::new(0, 0, size.w as u32, size.h as u32));
            m.primary = true;
            m
        }])
    }

    fn apply_geometry(&mut self, window: WindowId, geometry: srdwm_core::Rect) -> PlatformResult<()> {
        let _ = geometry;
        self.state.sync_geometry(window);
        Ok(())
    }

    fn set_title(&mut self, _window: WindowId, _title: &str) -> PlatformResult<()> {
        Ok(())
    }

    fn focus(&mut self, window: WindowId) -> PlatformResult<()> {
        self.state.wm.borrow_mut().focus_window(window);
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
        Ok(())
    }

    fn close(&mut self, window: WindowId) -> PlatformResult<()> {
        if let Some(w) = self.state.id_to_window.get(&window).and_then(|w| w.toplevel()) {
            w.send_close();
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
}
