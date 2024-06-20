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
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState as BackendButtonState, Event as InputEventTrait, InputEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::backend::winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend};
use smithay::reexports::winit::dpi::LogicalSize as WinitLogicalSize;
use smithay::reexports::winit::window::Window as WinitWindow;
use smithay::desktop::space::render_output;
use smithay::desktop::{layer_map_for_output, PopupManager, Space};
use smithay::input::SeatState;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop as CalloopEventLoop;
use smithay::reexports::wayland_server::{Client, Display, ListeningSocket};
use smithay::reexports::winit::platform::pump_events::PumpStatus;
use smithay::utils::{Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::xdg_activation::XdgActivationState;

use srdwm_core::{Event as CoreEvent, Window as CoreWindow, WindowId, WindowManager};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};

use crate::input::{handle_keyboard_key_event, handle_pointer_button, handle_pointer_position, last_pointer_pos};
use crate::lock::{lock_render_elements, send_lock_frame};
use crate::lock::SessionLock;
use srdwm_platform::IpcServer;
use crate::state::{ClientState, CompState, OutputEntry};
use crate::{decoration, err, screencopy};

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
    ipc: Option<IpcServer>,
    /// Exists solely to host `IdleNotifierState`'s internal per-notification
    /// timers - this backend otherwise has no `calloop` loop of its own at
    /// all (see `ipc.rs`'s module doc comment), drawing everything instead
    /// from `winit_events`'s manual poll and this struct's own per-tick
    /// work. `ext_idle_notify_v1` needs a real `LoopHandle` to construct
    /// (`smithay::wayland::idle_notify::IdleNotifierState::new`), and the
    /// alternative - constructing the global without ever dispatching the
    /// loop backing it - would advertise a protocol whose `idled`/`resumed`
    /// events then simply never fire, a worse trap than the small addition
    /// of a second, narrowly-scoped loop dispatched non-blocking once per
    /// tick in `poll_events`.
    idle_event_loop: CalloopEventLoop<'static, CompState>,
    /// When the last frame was rendered - see `poll_events`' doc comment
    /// on why this backend has to pace itself.
    last_frame: Instant,
}

/// Target frame budget for the winit (nested) backend's self-imposed pacing
/// - see `poll_events`' doc comment. 60fps to match `OutputMode`'s own
/// `refresh: 60_000` a few lines below, not because either number is
/// special.
const TARGET_FRAME_TIME: Duration = Duration::from_micros(1_000_000 / 60);

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
        // 600ms delay, not 200 - see `state.rs`'s `REPEAT_DELAY` doc
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
        self.state.tick_animations();
        self.state.tick_dirty_broadcasts();
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

        let mut custom_elements: Vec<crate::elements::OverlayElement<GlesRenderer>> = Vec::new();
        // The right-click titlebar menu, if open - pushed first so it's
        // topmost over every window (this backend draws no cursor of its
        // own, see this module's doc comment, so there's no "stay under
        // the pointer" ordering concern like udev.rs's matching push has).
        if let (Some(menu), Some(buffer)) = (self.state.context_menu.as_ref(), self.state.context_menu_buffer.as_ref()) {
            let pos = (menu.pos.0 as f64, menu.pos.1 as f64);
            match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                Err(e) => log::warn!("failed to import context menu buffer: {e}"),
            }
        }
        // Decoration/border built per window, front-to-back (topmost
        // first), so a window's own titlebar/border at least stay ordered
        // consistently relative to *other* windows' decoration/border.
        //
        // Content deliberately still goes through `render_output`'s own
        // `spaces` argument below (`self.state.space`), not through this
        // loop: an earlier version of this fix pushed each window's own
        // `Window::render_elements` output into `custom_elements` here too,
        // in per-window stacking order, specifically to let decoration/
        // border interleave correctly with *other* windows' content
        // (custom_elements otherwise draws entirely above every window's
        // content, `spaces` or not - seeing `render_output`'s source is
        // what motivated that attempt). It was reverted: with two or more
        // native Wayland toplevels on screen, whichever was created
        // *first* always painted in front of later ones regardless of
        // real focus/stacking order, reproduced consistently across three
        // separate test windows, independent of push order, forced full
        // redraws (`age = 0`), and buffer age - i.e. a real ordering bug
        // in mixing multiple windows' own `render_elements` output this
        // way, not a damage-tracking artifact. The real root cause (found
        // later, by instrumenting a locally vendored smithay copy
        // directly) turned out to be unrelated to content-vs-decoration
        // mixing at all: `sync_geometry`'s `Space::map_element` call
        // silently re-stacked windows to the top of `Space`'s own order
        // any time position/size synced for *any* reason, independent of
        // which rendering path was used - see `state.rs`'s
        // `resync_stacking_order` doc comment for the full story and the
        // actual fix. `self.state.space` stayed the content path here
        // since reverting it was never itself wrong, just insufficient on
        // its own.
        let ids: Vec<WindowId> = self.wm.borrow().visible_windows_front_to_back().map(|w| w.id).collect();
        let focused = self.wm.borrow().focused_id();
        // Windows stacked in front of whichever one border/decoration is
        // being built right now - `ids` is already front-to-back, so this
        // only ever needs appending to, not recomputing. Only a window's
        // own *content* occludes correctly on its own path (via `space`,
        // real stacking order respected); everything drawn here goes
        // through `custom_elements`, which composites above *all* content
        // unconditionally, so both the border strips and the titlebar
        // bitmap below need this explicit occlusion test against it.
        let mut occluders: Vec<srdwm_core::Rect> = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(w) = self.wm.borrow().window(id).cloned() else { continue };
            // `w.geometry` is the animation's target, not necessarily where
            // the window is actually drawn this frame - see the matching
            // comment in `udev.rs`'s render loop for the full story
            // (reported live as the border "not flush" with the window
            // during an animated maximize/fullscreen/open-slide transition).
            let geom = self.state.window_anims.get(&id).map(crate::state::WindowAnim::current_rect).unwrap_or(w.geometry);
            // Same reasoning as udev.rs's matching push: positioned from
            // `geom`, not `w.geometry`, and not fragment-clipped against
            // `occluders` - see that comment.
            if let Some(shadow) = self.state.shadow_buffers.get(&id) {
                let rect = decoration::shadow_rect(geom);
                let pos = (rect.x as f64, rect.y as f64);
                match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, shadow, None, None, None, Kind::Unspecified) {
                    Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                    Err(e) => log::warn!("failed to import shadow buffer for window {id}: {e}"),
                }
            }
            if let Some(deco) = self.state.decorations.get(&id) {
                // Fragment-clipped, same as udev.rs's matching titlebar
                // push - see that comment for why all-or-nothing (skip
                // only once *fully* covered) wasn't enough: a titlebar
                // only partially covered, the common case for cascaded
                // windows, still bled through the covered part.
                let titlebar_rect = srdwm_core::Rect::new(geom.x, geom.y, geom.width, srdwm_core::TITLEBAR_HEIGHT);
                for fragment in crate::elements::visible_border_fragments(titlebar_rect, &occluders) {
                    let pos = (fragment.x as f64, fragment.y as f64);
                    let src = Rectangle::new(
                        Point::from(((fragment.x - titlebar_rect.x) as f64, (fragment.y - titlebar_rect.y) as f64)),
                        Size::from((fragment.width as f64, fragment.height as f64)),
                    );
                    match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, deco, None, Some(src), None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                        Err(e) => log::warn!("failed to import titlebar buffer for window {id}: {e}"),
                    }
                }
            }
            // Border strips sit entirely outside `geometry` (see
            // `decoration::border_strips`), so they never overlap this same
            // window's own decoration/content pixels - draw order relative
            // to those doesn't matter, only relative to other windows'.
            if w.border_width > 0 {
                let color = crate::state::effective_border_color(w.border_color, focused == Some(id));
                let strips = decoration::border_strips(geom, w.border_width);
                // Strip 0 (top) is rounded to match the titlebar underneath
                // it - see `render_border_top`'s doc comment - so it's a
                // cached bitmap (rebuilt only in `redraw_decoration_buffer`,
                // same as the titlebar itself), not rasterized fresh here
                // every frame; the other three don't touch a rounded corner
                // and stay persistent solid-colour buffers instead - see
                // `elements::border_side_render_element`'s doc comment for
                // why a per-frame rebuild of either was a real, continuous
                // cost, not a cosmetic one. Not fragment-clipped like the
                // other three below - see the matching comment in
                // `udev.rs` for why the top strip only gets the cheaper
                // all-or-nothing occlusion check.
                if strips[0].width > 0 && strips[0].height > 0 && !strips[0].subtract_all(&occluders).is_empty() {
                    if let Some(buffer) = self.state.border_top_decorations.get(&id) {
                        match MemoryRenderBufferRenderElement::from_buffer(renderer, (strips[0].x as f64, strips[0].y as f64), buffer, None, None, None, Kind::Unspecified) {
                            Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                            Err(e) => log::warn!("failed to import top border buffer for window {id}: {e}"),
                        }
                    }
                }
                let pool = self.state.border_side_buffers.entry(id).or_default();
                let mut buf_index = 0;
                for strip in &strips[1..] {
                    if strip.width == 0 || strip.height == 0 {
                        continue;
                    }
                    for fragment in crate::elements::visible_border_fragments(*strip, &occluders) {
                        let buf = crate::elements::border_fragment_buffer(pool, buf_index);
                        buf_index += 1;
                        custom_elements.push(crate::elements::OverlayElement::Solid(crate::elements::border_side_render_element(buf, fragment, color, (0, 0))));
                    }
                }
            }
            occluders.push(geom);
        }
        // Single output at the global origin - no offset to subtract, see
        // `elements.rs`'s doc comment on why udev.rs's per-head call does.
        let popup_targets = crate::elements::popup_targets(&self.state);
        custom_elements.extend(crate::elements::popup_render_elements(&popup_targets, renderer, (0, 0)));

        let result = render_output(
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
        let damage_rects: Vec<Rectangle<i32, Physical>> = result.damage.cloned().unwrap_or_default();
        let has_damage = !damage_rects.is_empty();
        drop(framebuffer);
        // Both the buffer swap and the frame-callback notification are
        // conditional on real damage now - this used to run
        // unconditionally on every call to this function (every ~16ms
        // regardless of activity), which told every window it could render
        // its next frame whether or not the screen had actually changed.
        // Any client using the standard wait-for-frame-callback render
        // pattern (most of them) had no reason not to redraw at whatever
        // rate this loop cycled, forever - confirmed live on the udev
        // backend: wezterm-gui pinned at 140%+ CPU sitting on a fully idle,
        // unchanged terminal, from the identical bug there.
        //
        // That output-wide gate wasn't enough on its own: cursor motion
        // alone damages the small region around the pointer, which still
        // marked the *whole output* damaged and sent every mapped window a
        // callback regardless of whether the cursor was anywhere near it.
        // `windows_touched_by_damage` narrows this to windows the actual
        // damage rectangles overlap - see its doc comment in elements.rs.
        if has_damage {
            self.backend.submit(None).map_err(err)?;
            let scale = Scale::from(self.output.current_scale().fractional_scale());
            let now = self.state.start_time.elapsed();
            for w in crate::elements::windows_touched_by_damage(&self.state.space, &damage_rects, scale) {
                w.send_frame(&self.output, now, None, |_, _| Some(self.output.clone()));
            }
        }
        // Deliberately outside `if has_damage`: the whole point of
        // `always_notify` is covering the case where the output has *no*
        // damage at all (a fully idle desktop, cursor not moving) but the
        // focused/hovered window still has a pending callback it needs
        // answered to unblock an input-driven redraw. Nesting this inside
        // `if has_damage` (the first version of this fix) meant it only
        // ever ran on a tick that already had damage from something else
        // happening - i.e. never in the exact scenario it exists for.
        // Reported live as clicks in Firefox still doing nothing at all,
        // not just intermittently, after the first version of this fix.
        {
            let pointer_pos = last_pointer_pos(&self.state);
            let now = self.state.start_time.elapsed();
            let wm = self.wm.borrow();
            let always_notify = [wm.focused_id(), wm.window_at(pointer_pos.x as i32, pointer_pos.y as i32)];
            drop(wm);
            for w in always_notify.into_iter().flatten().filter_map(|id| self.state.id_to_window.get(&id)) {
                w.send_frame(&self.output, now, None, |_, _| Some(self.output.clone()));
            }
        }
        // Layer-shell surfaces get their callback every pass, unconditionally
        // - NOT folded into the `has_damage` gate above. See the matching
        // (much longer) comment in udev.rs's `render_udev_frame`: many
        // layer-shell clients (GTK4/AGS among them) drive their entire
        // repaint loop off frame callbacks with no independent timer
        // fallback, so withholding the callback until *something* on the
        // desktop happens to produce damage deadlocks them permanently
        // after their first frame - confirmed live, AGS and waybar both
        // froze exactly this way. Toplevel windows keep the damage gate
        // (that's what fixed the wezterm-gui CPU-burn bug); layer surfaces
        // are few, cheap to redraw, and are exactly the periodic-UI-chrome
        // case frame callbacks exist to pace.
        for layer in layer_map_for_output(&self.output).layers() {
            layer.send_frame(&self.output, self.state.start_time.elapsed(), None, |_, _| Some(self.output.clone()));
        }

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

        let mut custom_elements: Vec<crate::elements::OverlayElement<GlesRenderer>> = Vec::new();
        for (&id, deco) in self.state.decorations.iter() {
            let Some(geom) = self.wm.borrow().visible_windows().find(|w| w.id == id).map(|w| w.geometry) else { continue };
            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(renderer, (geom.x as f64, geom.y as f64), deco, None, None, None, Kind::Unspecified) {
                custom_elements.push(crate::elements::OverlayElement::Memory(elem));
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
        // This backend had no scroll handling at all - `InputEvent::
        // PointerAxis` fell into the catch-all below and was silently
        // dropped, unconditionally, on every device. Same forwarding as
        // `udev.rs`'s equivalent (see its own comment for the `stop()`/
        // `v120()` reasoning); duplicated rather than shared since the two
        // backends' `InputEvent` generic parameters differ and there's no
        // shared event type to write one function against.
        WinitEvent::Input(InputEvent::PointerAxis { event }) => {
            if crate::input::handle_workspace_scroll(state, &event) {
                return;
            }
            let Some(pointer) = state.seat.get_pointer() else { return };
            let source = event.source();
            let mut frame = smithay::input::pointer::AxisFrame::new(event.time_msec()).source(source);
            for axis in [Axis::Horizontal, Axis::Vertical] {
                match event.amount(axis) {
                    Some(value) => frame = frame.value(axis, value),
                    None if source == AxisSource::Finger => frame = frame.stop(axis),
                    None => {}
                }
                if let Some(v120) = event.amount_v120(axis) {
                    frame = frame.v120(axis, v120 as i32);
                }
            }
            pointer.axis(state, frame);
            pointer.frame(state);
        }
        WinitEvent::Resized { .. } => {}
        _ => {}
    }
}

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
            }
        }
        let wait = TARGET_FRAME_TIME.saturating_sub(self.last_frame.elapsed());
        let _ = self.idle_event_loop.dispatch(Some(wait), &mut self.state);
        self.last_frame = Instant::now();
        self.render_frame()?;
        Ok(self.pending.borrow_mut().drain(..).collect())
    }

    fn monitors(&mut self) -> PlatformResult<Vec<srdwm_core::Monitor>> {
        // Shrunk by any layer-shell exclusive zone - see the matching
        // comment in `udev.rs`'s `monitors()`. This backend is always a
        // single output at the global origin, so the output-local zone
        // rectangle already is the usable global-space rect.
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        Ok(vec![{
            let rect = srdwm_core::Rect::new(zone.loc.x, zone.loc.y, zone.size.w as u32, zone.size.h as u32);
            let mut m = srdwm_core::Monitor::new(0, "winit", rect);
            // Same fix as `udev.rs`'s matching function: `Monitor::new`
            // defaults `full_geometry` to `geometry`, which is already
            // zone-shrunk here - without this, `toggle_fullscreen` had no
            // way to actually cover a bar/dock's reserved strip, since the
            // "true full rect" it targets was silently identical to the
            // "usable, shrunk rect" `toggle_maximize` targets.
            let full = self.backend.window_size();
            m.full_geometry = srdwm_core::Rect::new(0, 0, full.w as u32, full.h as u32);
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

    /// See `udev.rs`'s matching impl for why this has to go through
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
