//! DRM/udev backend: runs srdwm as the real compositor on a bare TTY (no
//! host session to nest under), unlike the `backend_winit`-based path in
//! `lib.rs`.
//!
//! Scope:
//! - Single primary GPU, but **every** connected connector on it: each
//!   becomes a [`UdevHead`] with its own scanout buffers, damage tracker
//!   and page-flip state, laid out left-to-right in the global coordinate
//!   space. Connectors are probed once at startup - no hotplug, and no
//!   second GPU.
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
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
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
    connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, Event as DrmEvent, Mode as DrmMode,
    ModeTypeFlags, PageFlipFlags,
};
use smithay::reexports::drm::Device as BasicDevice;
use smithay::reexports::input::Libinput;
use smithay::reexports::pixman::{FormatCode, Image};
use smithay::reexports::rustix;
use smithay::reexports::wayland_server::{Client, Display, ListeningSocket};
use smithay::utils::{Logical, Point, Transform};
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;

use srdwm_core::{Event as CoreEvent, WindowManager};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};

use crate::err;
use crate::input::{handle_keyboard_key_event, handle_pointer_button, handle_pointer_position};
use crate::state::{ClientState, CompState};

/// A DRM device node, opened through the session (not a raw `File::open`)
/// so access is properly gated by logind/seatd and revoked on VT switch.
pub(crate) struct Card(OwnedFd);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl BasicDevice for Card {}
impl ControlDevice for Card {}

pub(crate) struct DrmBuffer {
    dumb: DumbBuffer,
    fb: framebuffer::Handle,
    image: Image<'static, 'static>,
}

/// One connector+CRTC pair srdwm scans out to - i.e. one physical monitor.
///
/// Each head owns its own scanout buffers, damage tracker and flip state,
/// because monitors have independent resolutions and refresh cycles: a flip
/// completing on one says nothing about the others. The *renderer* is not
/// here but on [`UdevState`], since all heads on one GPU share it.
pub(crate) struct UdevHead {
    pub(crate) crtc: crtc::Handle,
    pub(crate) output: Output,
    pub(crate) damage_tracker: OutputDamageTracker,
    pub(crate) buffers: [DrmBuffer; 2],
    pub(crate) front: usize,
    /// A flip is in flight; the next frame for this head waits for the DRM
    /// page-flip event (matched by `crtc`) before starting.
    pub(crate) flip_pending: bool,
    /// Origin of this head in the global coordinate space.
    pub(crate) location: Point<i32, Logical>,
    pub(crate) size: (i32, i32),
}

/// Everything the DRM/udev backend needs that the nested winit backend
/// doesn't. Lives as a field on `CompState` (rather than a separate struct)
/// because calloop callbacks registered against the event loop only ever
/// get `&mut CompState` - see the module docs in `lib.rs` for why the
/// protocol-handler state itself has to be backend-agnostic.
pub(crate) struct UdevState {
    pub(crate) card: Rc<Card>,
    /// Shared by every head: one GPU, one software renderer.
    pub(crate) renderer: PixmanRenderer,
    pub(crate) heads: Vec<UdevHead>,
    pub(crate) active: bool,
    /// Pointer position in the *global* space, so it can cross between
    /// monitors; clamped to the union of all head rectangles.
    pub(crate) pointer_pos: Point<f64, Logical>,
}

impl UdevState {
    /// Bounding box of every head, used to clamp pointer motion.
    fn bounds(&self) -> (f64, f64) {
        let w = self.heads.iter().map(|h| h.location.x + h.size.0).max().unwrap_or(0);
        let h = self.heads.iter().map(|h| h.location.y + h.size.1).max().unwrap_or(0);
        (w as f64, h as f64)
    }
}

impl CompState {
    /// Renders and (if there was damage) page-flips a new frame on every
    /// head that is ready for one. A head with a flip still in flight is
    /// skipped this pass and picked up when its page-flip event arrives, so
    /// monitors on different refresh rates each run at their own pace
    /// instead of the slowest one gating the rest.
    pub(crate) fn render_udev_frame(&mut self) {
        let locked = self.lock.locked;
        let elapsed = self.start_time.elapsed();
        // Drained before the `&mut self.udev` borrow below, so screencopy can
        // be serviced with the renderer that borrow owns.
        let mut captures = std::mem::take(&mut self.screencopy_pending);

        // Which heads are eligible, and what each needs, gathered before the
        // mutable borrow of `self.udev`.
        let Some(udev) = self.udev.as_ref() else { return };
        if !udev.active {
            return;
        }
        let ready: Vec<(usize, Output)> = udev
            .heads
            .iter()
            .enumerate()
            .filter(|(_, h)| !h.flip_pending)
            .map(|(i, h)| (i, h.output.clone()))
            .collect();

        let mut presented: Vec<Output> = Vec::new();
        for (index, output) in ready {
            let lock_surface = self.lock_surface_for(&output).cloned();

            // Decoration elements are built per head: `from_buffer` needs the
            // renderer, and geometry is translated into head-local space.
            let origin = self.udev.as_ref().map(|u| u.heads[index].location).unwrap_or_default();
            let decorations: Vec<(srdwm_core::Rect, MemoryRenderBuffer)> = if locked {
                Vec::new()
            } else {
                self.decorations
                    .iter()
                    .filter_map(|(&id, deco)| self.wm.borrow().window(id).map(|w| (w.geometry, deco.clone())))
                    .collect()
            };

            let Some(udev) = self.udev.as_mut() else { return };
            let head = &mut udev.heads[index];
            let back = 1 - head.front;

            let mut custom_elements: Vec<MemoryRenderBufferRenderElement<PixmanRenderer>> = Vec::new();
            if !locked {
                for (geom, deco) in &decorations {
                    let pos = ((geom.x - origin.x) as f64, (geom.y - origin.y) as f64);
                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, deco, None, None, None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(elem),
                        Err(e) => log::warn!("udev: failed to import titlebar buffer: {e}"),
                    }
                }
            }
            let lock_elements = if locked {
                crate::lock::lock_render_elements(lock_surface.as_ref(), &mut udev.renderer)
            } else {
                Vec::new()
            };

            let head = &mut udev.heads[index];
            let mut framebuffer = match udev.renderer.bind(&mut head.buffers[back].image) {
                Ok(fb) => fb,
                Err(e) => {
                    log::error!("udev: pixman bind failed: {e}");
                    continue;
                }
            };

            // Locked heads draw the lock surface over opaque black and
            // nothing else; unlocked heads draw the normal scene.
            let result = if locked {
                head.damage_tracker
                    .render_output(&mut udev.renderer, &mut framebuffer, 0, &lock_elements, [0.0, 0.0, 0.0, 1.0])
                    .map(|r| r.damage.is_some())
                    .map_err(|e| e.to_string())
            } else {
                render_output(
                    &head.output,
                    &mut udev.renderer,
                    &mut framebuffer,
                    1.0,
                    0, // always a full redraw: buffer "age" tracking isn't worth the complexity for a software-only backend
                    [&self.space],
                    &custom_elements,
                    &mut head.damage_tracker,
                    [0.05, 0.05, 0.08, 1.0],
                )
                .map(|r| r.damage.is_some())
                // Both arms reduce to "was there damage"; the two error types
                // differ, so they are flattened to a message here.
                .map_err(|e| e.to_string())
            };
            if !locked {
                crate::screencopy::service_pending(std::mem::take(&mut captures), &mut udev.renderer, &framebuffer);
            }
            drop(framebuffer);

            let has_damage = match result {
                Ok(d) => d,
                Err(e) => {
                    log::error!("udev: render_output failed: {e}");
                    continue;
                }
            };
            if has_damage {
                let head = &mut udev.heads[index];
                if let Err(e) = head.copy_and_flip(&udev.card, back) {
                    log::error!("udev: page flip failed: {e}");
                    continue;
                }
            }
            presented.push(output);
        }

        // Frame callbacks + lock confirmation, once the `udev` borrow is done.
        for output in presented {
            if locked {
                let surface = self.lock_surface_for(&output).cloned();
                crate::lock::send_lock_frame(surface.as_ref(), &output, elapsed);
                self.confirm_lock_if_presented(&output);
            } else {
                let out = output.clone();
                self.space.elements().for_each(|w| w.send_frame(&out, elapsed, None, |_, _| Some(out.clone())));
            }
        }
        if locked {
            crate::screencopy::fail_pending(captures);
        }
    }
}

impl UdevHead {
    /// Copies the just-rendered pixman image into buffer `back`'s dumb
    /// buffer (software rendering writes into its own owned image, not the
    /// scanout memory directly, to avoid tying that image's lifetime to an
    /// mmap - see this module's docs) and flips to it.
    fn copy_and_flip(&mut self, card: &Card, back: usize) -> std::io::Result<()> {
        let (stride, height) = (self.buffers[back].image.stride(), self.buffers[back].image.height());
        let byte_len = stride * height;
        // SAFETY: `image` owns this memory and outlives the byte slice we
        // construct from it here; we only read, and only for the duration
        // of this call.
        let src: &[u8] = unsafe { std::slice::from_raw_parts(self.buffers[back].image.data() as *const u8, byte_len) };
        {
            let mut mapping = card.map_dumb_buffer(&mut self.buffers[back].dumb)?;
            let dst = mapping.as_mut();
            let len = byte_len.min(dst.len());
            dst[..len].copy_from_slice(&src[..len]);
        }
        card.page_flip(self.crtc, self.buffers[back].fb, PageFlipFlags::EVENT, None)?;
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

        // Every connected connector becomes a head, laid out left-to-right.
        let connected = find_connected_outputs(&card)?;
        log::info!("udev: {} connected output(s)", connected.len());

        let renderer = PixmanRenderer::new().map_err(err)?;
        let dh = Display::<CompState>::new().map_err(err)?;
        let display_handle = dh.handle();

        let mut heads: Vec<UdevHead> = Vec::new();
        let mut output_entries: Vec<crate::state::OutputEntry> = Vec::new();
        let mut x_offset = 0;
        for (index, probe) in connected.iter().enumerate() {
            let (width, height) = probe.mode.size();
            let (width, height) = (width as i32, height as i32);

            let buffers = [make_drm_buffer(&card, width, height)?, make_drm_buffer(&card, width, height)?];
            card.set_crtc(probe.crtc, Some(buffers[0].fb), (0, 0), &[probe.connector], Some(probe.mode))
                .map_err(err)?;

            // Named after the real connector (eDP-1, HDMI-A-1, ...) so
            // clients and the user can tell monitors apart; `wl_output.name`
            // is what a bar's per-monitor config keys off.
            let output = Output::new(
                probe.name.clone(),
                PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "srdwm".into(), model: "drm".into() },
            );
            output.change_current_state(
                Some(OutputMode { size: (width, height).into(), refresh: mode_refresh_mhz(&probe.mode) }),
                Some(Transform::Normal),
                None,
                Some((x_offset, 0).into()),
            );
            output.set_preferred(OutputMode { size: (width, height).into(), refresh: mode_refresh_mhz(&probe.mode) });
            output.create_global::<CompState>(&display_handle);

            let location: Point<i32, Logical> = (x_offset, 0).into();
            heads.push(UdevHead {
                crtc: probe.crtc,
                output: output.clone(),
                damage_tracker: OutputDamageTracker::from_output(&output),
                buffers,
                front: 0,
                flip_pending: false,
                location,
                size: (width, height),
            });
            output_entries.push(crate::state::OutputEntry { output, location });
            log::info!("udev: head {index}: {} {width}x{height} at x={x_offset}", probe.name);
            x_offset += width;
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
        seat.add_keyboard(Default::default(), 200, 25).map_err(err)?;
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
        };

        let state = CompState {
            compositor_state,
            xdg_shell_state,
            _xdg_decoration_state: xdg_decoration_state,
            shm_state,
            seat_state,
            seat,
            space,
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
            lock: Default::default(),
            wm: wm.clone(),
            surface_to_id: HashMap::new(),
            id_to_window: HashMap::new(),
            decorations: HashMap::new(),
            pending: pending.clone(),
            bound_keys: Rc::new(bound_keys.iter().cloned().collect::<HashSet<_>>()),
            start_time: Instant::now(),
            udev: Some(udev_state),
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

/// A connector we intend to drive, paired with the CRTC that will scan it
/// out. Produced once at startup by [`find_connected_outputs`].
struct OutputProbe {
    crtc: crtc::Handle,
    connector: connector::Handle,
    mode: DrmMode,
    /// Connector name as the kernel reports it (`eDP-1`, `HDMI-A-1`, ...).
    name: String,
}

/// Every connected connector, each assigned a distinct CRTC.
///
/// CRTCs are a finite hardware resource and cannot be shared, so a CRTC
/// already claimed by an earlier connector is skipped - a machine with more
/// connected monitors than CRTCs drives as many as the hardware allows and
/// logs the rest rather than failing outright.
///
/// Still no hotplug: connectors are probed once at startup. Plugging a
/// monitor in later needs a udev event handler, which this backend does not
/// register yet (see `docs/IMPLEMENTATION_STATUS.md`).
fn find_connected_outputs(card: &Card) -> PlatformResult<Vec<OutputProbe>> {
    let res = card.resource_handles().map_err(err)?;
    let connectors: Vec<connector::Info> = res.connectors().iter().flat_map(|&h| card.get_connector(h, true)).collect();

    let mut used: Vec<crtc::Handle> = Vec::new();
    let mut probes = Vec::new();
    for con in connectors.iter().filter(|c| c.state() == connector::State::Connected) {
        let name = format!("{:?}-{}", con.interface(), con.interface_id());
        // Prefer the mode the display advertises as PREFERRED (its native
        // resolution) rather than whatever happens to be listed first --
        // the list order is not guaranteed, and picking wrong means running
        // a monitor at the wrong resolution. Falls back to the first mode
        // for connectors that flag none.
        let Some(&mode) = con
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| con.modes().first())
        else {
            log::warn!("udev: connector {name} is connected but reports no modes; skipping");
            continue;
        };
        // Prefer the CRTC already driving this connector, else any free one
        // the encoder can reach.
        let candidates: Vec<crtc::Handle> = con
            .current_encoder()
            .and_then(|enc| card.get_encoder(enc).ok())
            .map(|enc| res.filter_crtcs(enc.possible_crtcs()))
            .unwrap_or_default()
            .into_iter()
            .chain(res.crtcs().iter().copied())
            .collect();
        let Some(crtc) = candidates.into_iter().find(|c| !used.contains(c)) else {
            log::warn!("udev: no free CRTC left for connector {name}; not driving it");
            continue;
        };
        used.push(crtc);
        probes.push(OutputProbe { crtc, connector: con.handle(), mode, name });
    }

    if probes.is_empty() {
        return Err(PlatformError::Other("udev: no connected connector found".into()));
    }
    Ok(probes)
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
                    // The event names the CRTC it came from, so with several
                    // monitors only that head advances - flipping all of
                    // them would desynchronise the others' buffers.
                    let mut flipped = false;
                    for event in events {
                        let DrmEvent::PageFlip(flip) = event else { continue };
                        if let Some(udev) = data.udev.as_mut() {
                            if let Some(head) = udev.heads.iter_mut().find(|h| h.crtc == flip.crtc) {
                                head.front = 1 - head.front;
                                head.flip_pending = false;
                                flipped = true;
                            }
                        }
                    }
                    if flipped {
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
                    // switch; reassert every head before rendering again.
                    let card = udev.card.clone();
                    for head in &mut udev.heads {
                        let fb = head.buffers[head.front].fb;
                        if let Err(e) = card.set_crtc(head.crtc, Some(fb), (0, 0), &[], None) {
                            log::warn!("udev: failed to reassert crtc on resume: {e}");
                        }
                        // Force a full repaint: contents are undefined after
                        // the VT switch.
                        head.flip_pending = false;
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
            // Clamped to the union of every head, so the pointer travels
            // between monitors instead of stopping at the first one's edge.
            let (w, h) = udev.bounds();
            udev.pointer_pos.x = (udev.pointer_pos.x + delta.x).clamp(0.0, (w - 1.0).max(0.0));
            udev.pointer_pos.y = (udev.pointer_pos.y + delta.y).clamp(0.0, (h - 1.0).max(0.0));
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
                let (w, h) = head.size;
                let rect = srdwm_core::Rect::new(head.location.x, head.location.y, w as u32, h as u32);
                let mut m = srdwm_core::Monitor::new(i as u32, head.output.name(), rect);
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
