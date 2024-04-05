//! DRM/udev backend: runs srdwm as the real compositor on a bare TTY (no
//! host session to nest under), unlike the `backend_winit`-based path in
//! `lib.rs`.
//!
//! Scope, kept deliberately narrow for a first real (not faked) pass:
//! - Single primary GPU, first connected connector, its preferred (first
//!   listed) mode, positioned at `(0, 0)` - no hotplug of connectors or
//!   GPUs after startup, no multi-monitor layout.
//! - Rendering is **software**, via smithay's `PixmanRenderer` compositing
//!   into plain KMS "dumb buffers" through the legacy (non-atomic) mode-set
//!   API (`set_crtc`/`page_flip`). This deliberately avoids the
//!   GBM/EGL/`DrmCompositor` pipeline real hardware-accelerated compositors
//!   (and smithay's own `anvil` example) use: that path needs a GPU with
//!   working KMS+3D driver support, which is not guaranteed in a low-spec
//!   machine's VM (QEMU's plainest virtual display devices only support
//!   dumb-buffer scanout). Dumb buffers work on essentially any DRM driver.
//! - Session/seat handling is real, via `libseat` (VT-switch-safe device
//!   access, no root required if the seatd/logind + libseat setup is
//!   present) - not a raw `/dev/dri/cardN` open.
//! - Input is real, via `libinput`, sharing the exact same precise
//!   keybinding matching and pointer/titlebar hit-testing code paths
//!   `handle_keyboard_key_event`/`handle_pointer_position`/
//!   `handle_pointer_button` in `lib.rs` use for the nested winit backend.
//! - Session pause/resume (VT switch away/back) stops/resumes rendering,
//!   but does not yet re-probe connectors on resume.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::input::{
    Axis, ButtonState as BackendButtonState, Event as InputEventTrait, InputEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::Bind;
use smithay::backend::session::{libseat::LibSeatSession, libseat::LibSeatSessionNotifier, Event as SessionEvent, Session};
use smithay::backend::udev;
use smithay::desktop::space::render_output;
use smithay::desktop::Space;
use smithay::input::pointer::AxisFrame;
use smithay::input::SeatState;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::{FdWrapper, Generic};
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode as CalloopMode, PostAction};
use smithay::reexports::drm::buffer::DrmFourcc;
use smithay::reexports::drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, Event as DrmEvent, Mode as DrmMode, PageFlipFlags,
};
use smithay::reexports::drm::Device as BasicDevice;
use smithay::reexports::input::Libinput;
use smithay::reexports::pixman::{FormatCode, Image};
use smithay::reexports::rustix;
use smithay::reexports::wayland_server::{Client, Display, ListeningSocket};
use smithay::utils::{Logical, Point, Transform};
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;

use srdwm_core::{Event as CoreEvent, WindowManager};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};

use crate::{
    err, handle_keyboard_key_event, handle_pointer_button, handle_pointer_position, ClientState, CompState,
};

/// A DRM device node, opened through the session (not a raw `File::open`)
/// so access is properly gated by logind/seatd and revoked on VT switch.
struct Card(OwnedFd);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl BasicDevice for Card {}
impl ControlDevice for Card {}

struct DrmBuffer {
    dumb: DumbBuffer,
    fb: framebuffer::Handle,
    image: Image<'static, 'static>,
}

/// Everything the DRM/udev backend needs that the nested winit backend
/// doesn't. Lives as a field on `CompState` (rather than a separate struct)
/// because calloop callbacks registered against the event loop only ever
/// get `&mut CompState` - see the module docs in `lib.rs` for why the
/// protocol-handler state itself has to be backend-agnostic.
pub(crate) struct UdevOutput {
    card: Rc<Card>,
    crtc: crtc::Handle,
    renderer: PixmanRenderer,
    damage_tracker: OutputDamageTracker,
    output: Output,
    buffers: [DrmBuffer; 2],
    front: usize,
    flip_pending: bool,
    active: bool,
    pointer_pos: Point<f64, Logical>,
    size: (i32, i32),
}

impl CompState {
    /// Renders and (if there was damage) page-flips a new frame. No-op if
    /// a flip is already in flight (we wait for the DRM page-flip event
    /// before starting the next frame) or the session is paused.
    pub(crate) fn render_udev_frame(&mut self) {
        let Some(udev) = self.udev.as_mut() else { return };
        if udev.flip_pending || !udev.active {
            return;
        }

        let back = 1 - udev.front;

        let mut custom_elements: Vec<MemoryRenderBufferRenderElement<PixmanRenderer>> = Vec::new();
        for (&id, deco) in self.decorations.iter() {
            let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) else { continue };
            match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, (geom.x as f64, geom.y as f64), deco, None, None, None, Kind::Unspecified) {
                Ok(elem) => custom_elements.push(elem),
                Err(e) => log::warn!("udev: failed to import titlebar buffer for window {id}: {e}"),
            }
        }

        let mut framebuffer = match udev.renderer.bind(&mut udev.buffers[back].image) {
            Ok(fb) => fb,
            Err(e) => {
                log::error!("udev: pixman bind failed: {e}");
                return;
            }
        };

        let result = render_output(
            &udev.output,
            &mut udev.renderer,
            &mut framebuffer,
            1.0,
            0, // always a full redraw: buffer "age" tracking isn't worth the complexity for a software-only, single-output backend
            [&self.space],
            &custom_elements,
            &mut udev.damage_tracker,
            [0.05, 0.05, 0.08, 1.0],
        );
        drop(framebuffer);

        let has_damage = match result {
            Ok(res) => res.damage.is_some(),
            Err(e) => {
                log::error!("udev: render_output failed: {e}");
                false
            }
        };
        if !has_damage {
            return;
        }

        if let Err(e) = udev.copy_and_flip(back) {
            log::error!("udev: page flip failed: {e}");
            return;
        }
        self.space.elements().for_each(|w| w.send_frame(&udev.output, self.start_time.elapsed(), None, |_, _| Some(udev.output.clone())));
    }
}

impl UdevOutput {
    /// Copies the just-rendered pixman image into buffer `back`'s dumb
    /// buffer (software rendering writes into its own owned image, not the
    /// scanout memory directly, to avoid tying that image's lifetime to an
    /// mmap - seem `crates/wayland/src/udev.rs` module docs) and flips to it.
    fn copy_and_flip(&mut self, back: usize) -> std::io::Result<()> {
        let (stride, height) = (self.buffers[back].image.stride(), self.buffers[back].image.height());
        let byte_len = stride * height;
        // SAFETY: `image` owns this memory and outlives the byte slice we
        // construct from it here; we only read, and only for the duration
        // of this call.
        let src: &[u8] = unsafe { std::slice::from_raw_parts(self.buffers[back].image.data() as *const u8, byte_len) };
        {
            let mut mapping = self.card.map_dumb_buffer(&mut self.buffers[back].dumb)?;
            let dst = mapping.as_mut();
            let len = byte_len.min(dst.len());
            dst[..len].copy_from_slice(&src[..len]);
        }
        self.card.page_flip(self.crtc, self.buffers[back].fb, PageFlipFlags::EVENT, None)?;
        self.flip_pending = true;
        Ok(())
    }
}

pub struct UdevPlatform {
    event_loop: EventLoop<'static, CompState>,
    display: Display<CompState>,
    state: CompState,
    listener: ListeningSocket,
    clients: Vec<Client>,
    pending: Rc<RefCell<Vec<CoreEvent>>>,
}

impl UdevPlatform {
    pub fn connect(wm: Rc<RefCell<WindowManager>>, bound_keys: &[String]) -> PlatformResult<Self> {
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

        let (crtc, connector, mode) = find_connected_output(&card)?;
        let (width, height) = mode.size();
        let (width, height) = (width as i32, height as i32);

        let buffers = [make_drm_buffer(&card, width, height)?, make_drm_buffer(&card, width, height)?];
        card.set_crtc(crtc, Some(buffers[0].fb), (0, 0), &[connector], Some(mode)).map_err(err)?;

        let output = Output::new(
            "srdwm-udev".to_string(),
            PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "srdwm".into(), model: "drm".into() },
        );
        let refresh = mode_refresh_mhz(&mode);
        output.change_current_state(
            Some(OutputMode { size: (width, height).into(), refresh }),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );

        let renderer = PixmanRenderer::new().map_err(err)?;
        let damage_tracker = OutputDamageTracker::from_output(&output);

        let dh = Display::<CompState>::new().map_err(err)?;
        let display_handle = dh.handle();
        output.create_global::<CompState>(&display_handle);

        let compositor_state = CompositorState::new::<CompState>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<CompState>(&display_handle);
        let xdg_decoration_state = XdgDecorationState::new::<CompState>(&display_handle);
        let shm_state = ShmState::new::<CompState>(&display_handle, Vec::new());
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "seat0");
        seat.add_keyboard(Default::default(), 200, 25).map_err(err)?;
        seat.add_pointer();

        let mut space = Space::default();
        space.map_output(&output, (0, 0));

        let pending = Rc::new(RefCell::new(Vec::new()));
        let udev_output = UdevOutput {
            card: card.clone(),
            crtc,
            renderer,
            damage_tracker,
            output: output.clone(),
            buffers,
            front: 0,
            flip_pending: false,
            active: true,
            pointer_pos: (width as f64 / 2.0, height as f64 / 2.0).into(),
            size: (width, height),
        };

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
            bound_keys: Rc::new(bound_keys.iter().cloned().collect::<HashSet<_>>()),
            start_time: Instant::now(),
            udev: Some(udev_output),
            xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState::new::<CompState>(&display_handle),
            xwm: None,
            xwayland_windows: HashMap::new(),
            xwayland_pending: Vec::new(),
        };

        let listener = ListeningSocket::bind_auto("wayland", 0..32).map_err(err)?;
        if let Some(name) = listener.socket_name() {
            std::env::set_var("WAYLAND_DISPLAY", name);
            log::info!("wayland socket: {}", name.to_string_lossy());
        }

        let handle = event_loop.handle();
        register_drm_fd(&handle, &card)?;
        register_libinput(&handle, &session, &seat_name)?;
        register_session_notifier(&handle, notifier)?;
        if let Err(e) = crate::xwayland::spawn(&handle, &display_handle) {
            log::warn!("XWayland unavailable ({e}); X11-only clients will not run");
        }

        Ok(Self { event_loop, display: dh, state, listener, clients: Vec::new(), pending })
    }

    fn accept_clients(&mut self) -> PlatformResult<()> {
        if let Some(stream) = self.listener.accept().map_err(err)? {
            let client = self.display.handle().insert_client(stream, std::sync::Arc::new(ClientState::default())).map_err(err)?;
            self.clients.push(client);
        }
        Ok(())
    }
}

fn mode_refresh_mhz(mode: &DrmMode) -> i32 {
    let vrefresh = mode.vrefresh();
    if vrefresh > 0 {
        vrefresh as i32 * 1000
    } else {
        60_000
    }
}

// TODO: multi-monitor - returns only the first connected connector; no
// hotplug, no independent per-output layout. See
// docs/IMPLEMENTATION_STATUS.md's "Not implemented anywhere yet" section.
fn find_connected_output(card: &Card) -> PlatformResult<(crtc::Handle, connector::Handle, DrmMode)> {
    let res = card.resource_handles().map_err(err)?;
    let connectors: Vec<connector::Info> = res.connectors().iter().flat_map(|&h| card.get_connector(h, true)).collect();
    let con = connectors
        .iter()
        .find(|c| c.state() == connector::State::Connected)
        .ok_or_else(|| PlatformError::Other("udev: no connected connector found".into()))?;
    let mode = *con.modes().first().ok_or_else(|| PlatformError::Other("udev: connected connector has no modes".into()))?;

    let crtcs: Vec<crtc::Handle> = res.crtcs().to_vec();
    let crtc = con
        .current_encoder()
        .and_then(|enc| card.get_encoder(enc).ok())
        .and_then(|enc| res.filter_crtcs(enc.possible_crtcs()).into_iter().next())
        .or_else(|| crtcs.first().copied())
        .ok_or_else(|| PlatformError::Other("udev: no usable crtc found".into()))?;

    Ok((crtc, con.handle(), mode))
}

fn make_drm_buffer(card: &Card, width: i32, height: i32) -> PlatformResult<DrmBuffer> {
    let dumb = card.create_dumb_buffer((width as u32, height as u32), DrmFourcc::Xrgb8888, 32).map_err(err)?;
    let fb = card.add_framebuffer(&dumb, 24, 32).map_err(err)?;
    let format = FormatCode::try_from(DrmFourcc::Xrgb8888).map_err(|_| PlatformError::Other("udev: unsupported pixel format".into()))?;
    let image = Image::new(format, width as usize, height as usize, true).map_err(|_| PlatformError::Other("udev: failed to allocate render buffer".into()))?;
    Ok(DrmBuffer { dumb, fb, image })
}

fn register_drm_fd(handle: &LoopHandle<'static, CompState>, card: &Rc<Card>) -> PlatformResult<()> {
    let raw = card.as_fd().as_raw_fd();
    // SAFETY: `FdWrapper` does not close `raw`; the owning `Card` lives in
    // `CompState::udev` for as long as this event source is registered.
    let wrapper = unsafe { FdWrapper::new(raw) };
    let source = Generic::new(wrapper, Interest::READ, CalloopMode::Level);
    handle
        .insert_source(source, move |_, _, data: &mut CompState| {
            let Some(udev) = data.udev.as_ref() else { return Ok(PostAction::Continue) };
            let card = udev.card.clone();
            match card.receive_events() {
                Ok(events) => {
                    let mut flipped = false;
                    for event in events {
                        if let DrmEvent::PageFlip(_) = event {
                            flipped = true;
                        }
                    }
                    if flipped {
                        if let Some(udev) = data.udev.as_mut() {
                            udev.front = 1 - udev.front;
                            udev.flip_pending = false;
                        }
                        data.render_udev_frame();
                    }
                }
                Err(e) => log::warn!("udev: receive_events failed: {e}"),
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| PlatformError::Other(format!("failed to register DRM fd: {e}")))?;
    Ok(())
}

fn register_libinput(handle: &LoopHandle<'static, CompState>, session: &LibSeatSession, seat_name: &str) -> PlatformResult<()> {
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context.udev_assign_seat(seat_name).map_err(|_| PlatformError::Other("udev: libinput udev_assign_seat failed".into()))?;
    let libinput_backend = LibinputInputBackend::new(libinput_context);

    handle
        .insert_source(libinput_backend, move |event, _, data: &mut CompState| {
            handle_libinput_event(data, event);
        })
        .map_err(|e| PlatformError::Other(format!("failed to register libinput backend: {e}")))?;
    Ok(())
}

fn register_session_notifier(handle: &LoopHandle<'static, CompState>, notifier: LibSeatSessionNotifier) -> PlatformResult<()> {
    handle
        .insert_source(notifier, move |event, &mut (), data: &mut CompState| {
            let Some(udev) = data.udev.as_mut() else { return };
            match event {
                SessionEvent::PauseSession => {
                    log::info!("udev: session paused (VT switch away)");
                    udev.active = false;
                }
                SessionEvent::ActivateSession => {
                    log::info!("udev: session resumed (VT switch back)");
                    udev.active = true;
                    // Some drivers reset mode-setting state across a VT
                    // switch; reassert it before rendering again.
                    let fb = udev.buffers[udev.front].fb;
                    if let Err(e) = udev.card.set_crtc(udev.crtc, Some(fb), (0, 0), &[], None) {
                        log::warn!("udev: failed to reassert crtc on resume: {e}");
                    }
                    data.render_udev_frame();
                }
            }
        })
        .map_err(|e| PlatformError::Other(format!("failed to register session notifier: {e}")))?;
    Ok(())
}

fn handle_libinput_event(state: &mut CompState, event: InputEvent<LibinputInputBackend>) {
    match event {
        InputEvent::Keyboard { event } => handle_keyboard_key_event(state, &event),
        InputEvent::PointerMotion { event } => {
            let Some(udev) = state.udev.as_mut() else { return };
            let delta = event.delta();
            let (w, h) = (udev.size.0 as f64, udev.size.1 as f64);
            udev.pointer_pos.x = (udev.pointer_pos.x + delta.x).clamp(0.0, w - 1.0);
            udev.pointer_pos.y = (udev.pointer_pos.y + delta.y).clamp(0.0, h - 1.0);
            let pos = udev.pointer_pos;
            handle_pointer_position(state, pos, event.time_msec());
        }
        InputEvent::PointerButton { event } => {
            let Some(pos) = state.udev.as_ref().map(|u| u.pointer_pos) else { return };
            let button = event.button_code();
            let pressed = event.state() == BackendButtonState::Pressed;
            handle_pointer_button(state, pos, button, pressed, event.time_msec());
        }
        InputEvent::PointerAxis { event } => {
            // Scroll: forwarded to the focused client via the pointer axis
            // frame, no WM-level handling (matches the winit backend, which
            // doesn't handle scroll either).
            let Some(pointer) = state.seat.get_pointer() else { return };
            let source = event.source();
            let mut frame = AxisFrame::new(event.time_msec()).source(source);
            for axis in [Axis::Horizontal, Axis::Vertical] {
                if let Some(value) = event.amount(axis) {
                    frame = frame.value(axis, value);
                }
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }
        _ => {}
    }
}

impl Platform for UdevPlatform {
    fn kind(&self) -> PlatformKind {
        PlatformKind::Wayland
    }

    fn poll_events(&mut self) -> PlatformResult<Vec<CoreEvent>> {
        self.accept_clients()?;
        self.event_loop.dispatch(Some(Duration::from_millis(16)), &mut self.state).map_err(err)?;
        self.display.dispatch_clients(&mut self.state).map_err(err)?;
        self.display.flush_clients().map_err(err)?;
        self.state.render_udev_frame();
        Ok(self.pending.borrow_mut().drain(..).collect())
    }

    fn monitors(&mut self) -> PlatformResult<Vec<srdwm_core::Monitor>> {
        let Some(udev) = self.state.udev.as_ref() else { return Ok(Vec::new()) };
        let (w, h) = udev.size;
        Ok(vec![{
            let mut m = srdwm_core::Monitor::new(0, "drm", srdwm_core::Rect::new(0, 0, w as u32, h as u32));
            m.primary = true;
            m
        }])
    }

    fn apply_geometry(&mut self, window: srdwm_core::WindowId, _geometry: srdwm_core::Rect) -> PlatformResult<()> {
        self.state.sync_geometry(window);
        Ok(())
    }

    fn set_title(&mut self, _window: srdwm_core::WindowId, _title: &str) -> PlatformResult<()> {
        Ok(())
    }

    fn focus(&mut self, window: srdwm_core::WindowId) -> PlatformResult<()> {
        self.state.wm.borrow_mut().focus_window(window);
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
        if let Some(w) = self.state.id_to_window.get(&window).and_then(|w| w.toplevel()) {
            w.send_close();
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
}
