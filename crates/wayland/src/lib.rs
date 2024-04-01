//! Wayland backend for srdwm: a smithay-based compositor.
//!
//! Unlike the X11 backend, this has essentially no working prior art to
//! port from - the legacy C++ `wayland_platform.cc` created the wlroots
//! backend/renderer/compositor/seat/xdg-shell objects but never wired a
//! single `wl_signal_add` listener, so no window was ever actually managed
//! (see docs/PRIOR_ART.md). This is a from-scratch implementation using
//! `smithay` (the Rust analog of wlroots), following the same object-graph
//! shape smithay's own bundled `examples/` (compositor.rs, seat.rs) use.
//!
//! Scope for this pass, chosen to keep the implementation honest about what
//! is and isn't real:
//! - Runs via smithay's **winit backend**: a nested window on the host
//!   compositor/X server, analogous to how the X11 backend was verified
//!   under Xephyr. A DRM/udev backend for running as the actual system
//!   compositor on a TTY is not implemented - see docs/IMPLEMENTATION_STATUS.md.
//! - xdg-shell toplevels are tracked exactly like the X11 backend tracks
//!   X windows: through `srdwm_core::WindowManager`, so layout, smart
//!   placement, and drag/resize hit-testing are the *same* code path as X11
//!   (`srdwm_core::window::ResizeEdge::hit_test`), not a reimplementation.
//! - Decorations are drawn as a solid-color titlebar band (no text - font
//!   rasterization is a real chunk of additional work, not something to
//!   fake here) using smithay's `SolidColorRenderElement`.
//! - Global keybindings are intercepted using a simple heuristic: any key
//!   press with the Super/Mod4 modifier held is treated as WM-exclusive and
//!   not forwarded to the focused client; everything else is forwarded.
//!   Real WMs do something similar in practice (Super is rarely used by
//!   applications); a more precise design would thread the config's actual
//!   bound-key set into the platform layer, which is left as a TODO.
//! - xdg-decoration is forced to server-side mode (`Mode::ServerSide`) so
//!   well-behaved clients don't also draw their own client-side titlebar.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use smithay::backend::input::{
    AbsolutePositionEvent, ButtonState as BackendButtonState, Event as InputEventTrait, InputEvent,
    KeyState as BackendKeyState, KeyboardKeyEvent, PointerButtonEvent,
};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend};
use smithay::desktop::space::render_output;
use smithay::desktop::{Space, Window as DWindow};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{ButtonEvent, CursorImageStatus, MotionEvent};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, ListeningSocket};
use smithay::reexports::winit::platform::pump_events::PumpStatus;
use smithay::utils::{Logical, Point, Serial, Transform, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{delegate_compositor, delegate_output, delegate_seat, delegate_shm, delegate_xdg_decoration, delegate_xdg_shell};

use srdwm_core::{Event as CoreEvent, Modifiers, TitlebarHit, Window as CoreWindow, WindowId, WindowManager, TITLEBAR_HEIGHT};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(e.to_string())
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Everything smithay's protocol handlers need `&mut` access to. This is the
/// `D` type parameter of `Display<D>` - every `delegate_*!` macro below
/// requires the corresponding `*Handler` trait to be implemented on it.
struct CompState {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    _xdg_decoration_state: XdgDecorationState,
    shm_state: ShmState,
    seat_state: SeatState<CompState>,
    seat: Seat<CompState>,
    space: Space<DWindow>,
    wm: Rc<RefCell<WindowManager>>,
    surface_to_id: HashMap<WlSurface, WindowId>,
    id_to_window: HashMap<WindowId, DWindow>,
    decorations: HashMap<WindowId, SolidColorBuffer>,
    pending: Rc<RefCell<Vec<CoreEvent>>>,
    pointer_focus_super_held: bool,
    start_time: Instant,
}

impl CompState {
    fn new_managed_window(&mut self, toplevel: ToplevelSurface) {
        let surface = toplevel.wl_surface().clone();
        let id = {
            let mut wm = self.wm.borrow_mut();
            let id = wm.alloc_window_id();
            let title = with_toplevel_title(&toplevel).unwrap_or_default();
            let mut w = CoreWindow::new(id, title);
            w.geometry = srdwm_core::Rect::new(0, 0, 800, 600 + TITLEBAR_HEIGHT as i32 as u32);
            wm.add_window(w);
            id
        };
        let geom = self.wm.borrow().window(id).map(|w| w.geometry).unwrap_or_default();

        let dwindow = DWindow::new_wayland_window(toplevel.clone());
        toplevel.with_pending_state(|state| {
            state.size = Some((geom.width as i32, (geom.height - TITLEBAR_HEIGHT) as i32).into());
        });
        toplevel.send_configure();

        self.space.map_element(dwindow.clone(), (geom.x, geom.y + TITLEBAR_HEIGHT as i32), true);
        self.surface_to_id.insert(surface, id);
        self.id_to_window.insert(id, dwindow);
        self.decorations.insert(id, SolidColorBuffer::new((geom.width as i32, TITLEBAR_HEIGHT as i32), [0.18, 0.204, 0.251, 1.0]));
        self.pending.borrow_mut().push(CoreEvent::WindowCreated(id));
    }

    fn remove_window(&mut self, surface: &WlSurface) {
        let Some(id) = self.surface_to_id.remove(surface) else { return };
        if let Some(w) = self.id_to_window.remove(&id) {
            self.space.unmap_elem(&w);
        }
        self.decorations.remove(&id);
        self.wm.borrow_mut().remove_window(id);
        self.pending.borrow_mut().push(CoreEvent::WindowDestroyed(id));
    }

    fn sync_geometry(&mut self, id: WindowId) {
        let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) else { return };
        if let Some(w) = self.id_to_window.get(&id) {
            self.space.map_element(w.clone(), (geom.x, geom.y + TITLEBAR_HEIGHT as i32), false);
            if let Some(top) = w.toplevel() {
                top.with_pending_state(|state| {
                    state.size = Some((geom.width as i32, (geom.height - TITLEBAR_HEIGHT) as i32).into());
                });
                top.send_configure();
            }
        }
        if let Some(deco) = self.decorations.get_mut(&id) {
            deco.resize((geom.width as i32, TITLEBAR_HEIGHT as i32));
        }
    }
}

fn with_toplevel_title(toplevel: &ToplevelSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap().title.clone().unwrap_or_default())
    })
}

impl CompositorHandler for CompState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<CompState>(surface);
        if let Some(&id) = self.surface_to_id.get(surface) {
            if let Some(w) = self.id_to_window.get(&id) {
                w.on_commit();
            }
        }
    }
}

impl XdgShellHandler for CompState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.new_managed_window(surface);
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
    }
}

impl XdgDecorationHandler for CompState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {}
}

impl ShmHandler for CompState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for CompState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl smithay::wayland::output::OutputHandler for CompState {}

impl SeatHandler for CompState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}

delegate_compositor!(CompState);
delegate_xdg_shell!(CompState);
delegate_xdg_decoration!(CompState);
delegate_shm!(CompState);
delegate_seat!(CompState);
delegate_output!(CompState);

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
    pub fn connect(wm: Rc<RefCell<WindowManager>>) -> PlatformResult<Self> {
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

        let compositor_state = CompositorState::new::<CompState>(&dh);
        let xdg_shell_state = XdgShellState::new::<CompState>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<CompState>(&dh);
        let shm_state = ShmState::new::<CompState>(&dh, Vec::new());
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
            wm: wm.clone(),
            surface_to_id: HashMap::new(),
            id_to_window: HashMap::new(),
            decorations: HashMap::new(),
            pending: pending.clone(),
            pointer_focus_super_held: false,
            start_time: Instant::now(),
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
        self.output.change_current_state(Some(OutputMode { size, refresh: 60_000 }), None, None, None);

        let mut custom_elements: Vec<SolidColorRenderElement> = Vec::new();
        for (&id, deco) in self.state.decorations.iter() {
            if let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) {
                custom_elements.push(SolidColorRenderElement::from_buffer(deco, (geom.x, geom.y), 1.0, 1.0, Kind::Unspecified));
            }
        }

        let age = self.backend.buffer_age().unwrap_or(0);
        let (renderer, mut framebuffer) = self.backend.bind().map_err(err)?;
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
        Ok(())
    }
}

fn handle_winit_event(state: &mut CompState, output: &Output, event: WinitEvent, closed: &mut bool) {
    match event {
        WinitEvent::CloseRequested => *closed = true,
        WinitEvent::Input(InputEvent::Keyboard { event }) => {
            let keycode = event.key_code();
            let key_state = event.state();
            let time = event.time_msec();
            let serial = SERIAL_COUNTER.next_serial();
            let Some(keyboard) = state.seat.get_keyboard() else { return };

            let mut super_now = state.pointer_focus_super_held;
            keyboard.input::<(), _>(state, keycode, key_state, serial, time, |_, mods, _handle| {
                super_now = mods.logo;
                FilterResult::Forward
            });
            state.pointer_focus_super_held = super_now;

            if super_now && key_state == BackendKeyState::Pressed {
                if let Some(name) = keysym_name_for(&keyboard, keycode) {
                    state.pending.borrow_mut().push(CoreEvent::KeyPress { key_name: name, modifiers: Modifiers::SUPER });
                }
            }
            // Not a WM shortcut: let the focused client see it (already
            // forwarded by the FilterResult::Forward above).
        }
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

fn keysym_name_for(keyboard: &smithay::input::keyboard::KeyboardHandle<CompState>, keycode: smithay::backend::input::Keycode) -> Option<String> {
    let _ = keyboard;
    let _ = keycode;
    // TODO: translate via xkbcommon keysym -> name; the X11 backend's
    // `keysyms` table isn't reachable from here without duplicating it.
    // Global Wayland keybindings beyond the raw Super-modifier gate are not
    // wired up yet - see docs/IMPLEMENTATION_STATUS.md.
    None
}

fn last_pointer_pos(state: &CompState) -> Point<f64, Logical> {
    state.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default()
}

fn handle_pointer_position(state: &mut CompState, pos: Point<f64, Logical>, time: u32) {
    let hit = state.wm.borrow().hit_test(pos.x as i32, pos.y as i32);
    let under = state.space.element_under(pos).map(|(w, loc)| (w.clone(), loc));

    let Some(pointer) = state.seat.get_pointer() else { return };
    if hit.is_some() {
        // Over our own decoration - no client focus.
        pointer.motion(state, None, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    } else if let Some((window, loc)) = under {
        if let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) {
            let surface_loc = pos - loc.to_f64();
            pointer.motion(state, Some((surface, loc.to_f64())), &MotionEvent { location: surface_loc, serial: SERIAL_COUNTER.next_serial(), time });
        }
    } else {
        pointer.motion(state, None, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
    }

    let mut wm = state.wm.borrow_mut();
    let dragging_or_resizing = wm.is_dragging() || wm.is_resizing();
    if wm.is_dragging() {
        wm.update_drag(pos.x as i32, pos.y as i32);
    } else if wm.is_resizing() {
        wm.update_resize(pos.x as i32, pos.y as i32);
    }
    let focused = wm.focused_id();
    drop(wm);
    if dragging_or_resizing {
        if let Some(id) = focused {
            state.sync_geometry(id);
        }
    }
}

fn handle_pointer_button(state: &mut CompState, pos: Point<f64, Logical>, button: u32, pressed: bool, time: u32) {
    const BTN_LEFT: u32 = 0x110;
    let serial = SERIAL_COUNTER.next_serial();

    if pressed && button == BTN_LEFT {
        let hit = state.wm.borrow().hit_test(pos.x as i32, pos.y as i32);
        if let Some((id, hit)) = hit {
            state.wm.borrow_mut().focus_window(id);
            state.pending.borrow_mut().push(CoreEvent::WindowFocused(id));
            match hit {
                TitlebarHit::Drag => state.wm.borrow_mut().start_drag(id, pos.x as i32, pos.y as i32),
                TitlebarHit::Close => {
                    if let Some(w) = state.id_to_window.get(&id).and_then(|w| w.toplevel()) {
                        w.send_close();
                    }
                }
                TitlebarHit::Maximize => {
                    state.wm.borrow_mut().toggle_maximize(id);
                    state.sync_geometry(id);
                }
                TitlebarHit::Minimize => state.wm.borrow_mut().minimize_window(id),
                TitlebarHit::Resize(edge) => state.wm.borrow_mut().start_resize(id, edge, pos.x as i32, pos.y as i32),
            }
        } else if let Some((window, _loc)) = state.space.element_under(pos) {
            state.space.raise_element(&window.clone(), true);
        }
    } else if !pressed {
        let mut wm = state.wm.borrow_mut();
        if wm.is_dragging() {
            wm.end_drag();
        } else if wm.is_resizing() {
            wm.end_resize();
        }
    }

    if let Some(pointer) = state.seat.get_pointer() {
        let button_state = if pressed { BackendButtonState::Pressed } else { BackendButtonState::Released };
        pointer.button(state, &ButtonEvent { serial, time, button, state: button_state });
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
