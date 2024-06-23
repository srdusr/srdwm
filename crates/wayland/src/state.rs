//! Compositor state shared by both backends.
//!
//! [`CompState`] is the `D` type parameter of smithay's `Display<D>`: every
//! protocol handler in [`crate::protocols`] is implemented on it, and every
//! calloop callback the udev backend registers receives `&mut CompState`.
//! That makes it the one place backend-agnostic state has to live - hence
//! the `udev` field, which is `Some` only for the DRM backend.
//!
//! The inherent methods here are srdwm's own window bookkeeping (mapping
//! toplevels to `srdwm_core::WindowId`s, pushing geometry back out, keeping
//! titlebar buffers current). Protocol *reactions* live in
//! [`crate::protocols`]; input routing lives in [`crate::input`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::element::solid::SolidColorBuffer;
use smithay::desktop::{layer_map_for_output, PopupManager, Space, Window as DWindow, WindowSurfaceType};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::utils::{Logical, Point, Rectangle, Size, Transform, SERIAL_COUNTER};
use smithay::wayland::compositor::{with_states, CompositorClientState, CompositorState};
use smithay::wayland::selection::data_device::{set_data_device_focus, DataDeviceState};
use smithay::wayland::selection::primary_selection::{set_primary_focus, PrimarySelectionState};
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, LayerSurfaceData, WlrLayerShellState};
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::xdg_activation::XdgActivationState;

use srdwm_core::{Event as CoreEvent, Window as CoreWindow, WindowId, WindowManager, TITLEBAR_HEIGHT};

use crate::lock::SessionLock;
use crate::{decoration, foreign_toplevel, gamma_control, output_management, output_power, screencopy, udev, workspace, xwayland};

#[derive(Default)]
pub(crate) struct ClientState {
    pub(crate) compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// One output srdwm drives, and where it sits in the global coordinate
/// space.
///
/// The smithay [`Output`] is the protocol object: it carries the
/// `wl_output` global and owns that output's `LayerMap`.
///
/// A entry's **index in `CompState::outputs` is its
/// [`srdwm_core::Monitor`] id** - the udev backend builds both lists in
/// the same connector order, so core's already-multi-monitor-aware layout
/// code (`WindowManager::arrange_workspace` groups windows by monitor)
/// lines up with what is actually on screen without a separate mapping.
pub(crate) struct OutputEntry {
    pub(crate) output: Output,
    /// Origin of this output in the global space. Outputs are laid out
    /// left-to-right, so this is `(sum of widths to the left, 0)`.
    pub(crate) location: Point<i32, Logical>,
}

impl OutputEntry {
    /// Size in logical coordinates, or `(0, 0)` if no mode is set yet.
    pub(crate) fn size(&self) -> Size<i32, Logical> {
        self.output
            .current_mode()
            .map(|m| (m.size.w, m.size.h).into())
            .unwrap_or_default()
    }

    /// This output's rectangle in the global space.
    pub(crate) fn geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.location, self.size())
    }
}

/// Everything smithay's protocol handlers need `&mut` access to. This is the
/// `D` type parameter of `Display<D>` - every `delegate_*!` macro below
/// requires the corresponding `*Handler` trait to be implemented on it.
pub(crate) struct CompState {
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) _xdg_decoration_state: XdgDecorationState,
    pub(crate) shm_state: ShmState,
    /// `zwp_linux_dmabuf_v1` - see `protocols.rs`'s `DmabufHandler` impl.
    /// Without this global, no client can hand the compositor a GPU buffer
    /// at all; GTK4 in particular tries to open a DRM render node to
    /// allocate one anyway, fails with no global to query, and crashes
    /// instead of falling back (see `docs/PANEL_SUPPORT_TODO.md`'s P0.3).
    pub(crate) dmabuf_state: DmabufState,
    /// `xdg_activation_v1` - see `protocols.rs`'s `XdgActivationHandler`.
    /// Without this, a launcher's freshly-spawned app opens unfocused
    /// behind everything: nothing raises it once its window actually maps.
    pub(crate) xdg_activation_state: XdgActivationState,
    /// `zwp_text_input_manager_v3` + `zwp_input_method_manager_v2` - lets a
    /// real IME (fcitx5, ibus) attach to the focused text field and draw
    /// its own composition/candidate popup. See `protocols.rs`'s
    /// `InputMethodHandler` impl for why no extra focus-tracking is needed
    /// beyond registering these two globals.
    pub(crate) _text_input_manager_state: smithay::wayland::text_input::TextInputManagerState,
    pub(crate) _input_method_manager_state: smithay::wayland::input_method::InputMethodManagerState,
    /// `gtk_shell1` - the Wayland-native half of global-menu support. See
    /// `gtk_shell.rs`'s module doc comment.
    pub(crate) _gtk_shell_state: crate::gtk_shell::GtkShellState,
    pub(crate) seat_state: SeatState<CompState>,
    pub(crate) seat: Seat<CompState>,
    pub(crate) space: Space<DWindow>,
    /// Tracks every live `xdg_popup` (position, parent, grabs) - see
    /// `protocols.rs`'s `new_popup`/`reposition_request`/`commit` and
    /// `popup_render_elements` below.
    pub(crate) popups: PopupManager,
    /// Every output srdwm drives, left-to-right in the global coordinate
    /// space. The winit backend always has exactly one (its nested window);
    /// the udev backend has one per connected connector.
    ///
    /// Nothing outside this module should index this directly - go through
    /// [`CompState::primary_output`], [`CompState::output_at`] or
    /// [`CompState::output_for_wl`], so the single- and multi-output cases
    /// stay the same code path.
    pub(crate) outputs: Vec<OutputEntry>,
    pub(crate) layer_shell_state: WlrLayerShellState,
    /// Needed by the selection (clipboard) protocols: `set_data_device_focus`
    /// and `set_primary_focus` both take a `DisplayHandle`, and focus has to
    /// be re-pointed on every focus change (see `set_keyboard_focus`).
    pub(crate) dh: DisplayHandle,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) primary_selection_state: PrimarySelectionState,
    pub(crate) data_control_state: DataControlState,
    pub(crate) session_lock_state: SessionLockManagerState,
    /// `wp_viewporter` and `wp_fractional_scale_manager_v1`. Held only to
    /// keep the globals alive: surface scaling is handled inside smithay,
    /// and the compositor needs no logic of its own for either.
    ///
    /// Not optional in practice - a wallpaper daemon (`awww`/`swww`) hard
    /// *requires* both and panics on startup without them, and video
    /// players use viewporter for scaled playback.
    pub(crate) _viewporter_state: smithay::wayland::viewporter::ViewporterState,
    pub(crate) _fractional_scale_state: smithay::wayland::fractional_scale::FractionalScaleManagerState,
    pub(crate) _cursor_shape_state: smithay::wayland::cursor_shape::CursorShapeManagerState,
    pub(crate) _screencopy_state: screencopy::ScreencopyState,
    /// Captures requested via `wlr-screencopy` but not yet serviced; drained
    /// inside the render pass (see `screencopy::service_pending`).
    pub(crate) screencopy_pending: Vec<screencopy::PendingCapture>,
    pub(crate) _foreign_toplevel_state: foreign_toplevel::ForeignToplevelState,
    /// Every bound `zwlr_foreign_toplevel_manager_v1` (one per dock/switcher
    /// client), so a newly-created window can be announced to all of them --
    /// see `foreign_toplevel::window_created`.
    pub(crate) foreign_toplevel_managers: Vec<wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    /// The live `zwlr_foreign_toplevel_handle_v1` objects for each window --
    /// one per bound manager, since each manager only ever sees the handles
    /// created for *it*.
    pub(crate) foreign_toplevel_handles: HashMap<WindowId, Vec<wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1>>,
    pub(crate) _workspace_state: workspace::WorkspaceManagerState,
    /// `zwlr_output_power_management_v1` - `None` on the winit (nested)
    /// backend, which has no real display to power down. See
    /// `output_power.rs`'s module doc comment.
    pub(crate) _output_power_state: Option<output_power::OutputPowerManagerState>,
    /// `zwlr_gamma_control_manager_v1` - `None` on the winit (nested)
    /// backend, same reasoning as `_output_power_state`. See
    /// `gamma_control.rs`'s module doc comment.
    pub(crate) _gamma_control_state: Option<gamma_control::GammaControlManagerState>,
    /// `zwlr_output_management_v1` - unlike `_output_power_state`/
    /// `_gamma_control_state` above, not `Option`-gated: enumerating
    /// outputs and applying position/scale/transform changes works the
    /// same way (`Output::change_current_state`) on both backends, so
    /// there's no backend where advertising this global would be
    /// dishonest. See `output_management.rs`'s module doc comment.
    pub(crate) _output_management_state: output_management::OutputManagementState,
    pub(crate) output_managers: Vec<wayland_protocols_wlr::output_management::v1::server::zwlr_output_manager_v1::ZwlrOutputManagerV1>,
    pub(crate) output_heads: HashMap<String, Vec<wayland_protocols_wlr::output_management::v1::server::zwlr_output_head_v1::ZwlrOutputHeadV1>>,
    pub(crate) output_modes: HashMap<String, Vec<wayland_protocols_wlr::output_management::v1::server::zwlr_output_mode_v1::ZwlrOutputModeV1>>,
    /// Bumped on every real output-state change; sent with `zwlr_output_
    /// manager_v1.done` and checked against a `create_configuration`
    /// request's own serial so a client configuring against stale
    /// (pre-hotplug) state gets `cancelled` rather than silently
    /// clobbering whatever changed after it last saw a `done`.
    pub(crate) output_serial: u32,
    /// What was last broadcast to output-management clients - see
    /// `output_management::broadcast_dirty_outputs`'s doc comment.
    pub(crate) last_broadcast_outputs: Vec<output_management::OutputSnapshot>,
    pub(crate) workspace_managers: Vec<wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::ExtWorkspaceManagerV1>,
    pub(crate) workspace_groups: Vec<wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1>,
    pub(crate) workspace_handles: HashMap<srdwm_core::WorkspaceId, Vec<wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::ExtWorkspaceHandleV1>>,
    /// Session lock (`ext-session-lock-v1`). While `locked` is set, client
    /// content is never rendered and input never reaches normal clients --
    /// see `SessionLockHandler` below.
    pub(crate) lock: SessionLock,
    /// What the pointer should look like, as set by the focused client (or
    /// the default when no client has said). See `cursor.rs` for why this
    /// has to be drawn by us on the DRM backend.
    pub(crate) cursor_status: smithay::input::pointer::CursorImageStatus,
    /// Built-in cursor bitmaps (arrow, text, resize directions), built once
    /// at startup rather than per frame.
    pub(crate) cursor_buffers: crate::cursor::CursorBuffers,
    /// Last titlebar press, for double-click detection.
    pub(crate) last_titlebar_click: Option<(WindowId, u32)>,
    /// The right-click titlebar window menu, if one is currently open --
    /// see `context_menu.rs`. `None` almost always; a click anywhere while
    /// `Some` resolves (selects a row) or dismisses it, never falls
    /// through to normal click handling underneath.
    pub(crate) context_menu: Option<crate::context_menu::ContextMenu>,
    /// Rasterised pixels for the currently-open `context_menu`, rebuilt
    /// once when it opens - same cached-until-something-changes pattern
    /// `decorations`/`border_top_decorations` already use, not rebuilt
    /// per frame.
    pub(crate) context_menu_buffer: Option<MemoryRenderBuffer>,
    pub(crate) wm: Rc<RefCell<WindowManager>>,
    pub(crate) surface_to_id: HashMap<WlSurface, WindowId>,
    pub(crate) id_to_window: HashMap<WindowId, DWindow>,
    /// Surfaces whose `zwlr_layer_surface_v1` role has been destroyed --
    /// consulted by the pre-commit hook `CompositorHandler::new_surface`
    /// registers (see its doc comment) to work around a real smithay bug
    /// where a later commit of one of these surfaces gets spuriously
    /// rejected. Entries are added in `layer_destroyed`; there's
    /// deliberately no removal, since a `WlSurface` here is meaningless
    /// after the client destroys it too and Rust never reuses the id while
    /// any handle (including this one) still exists.
    pub(crate) dead_layer_surfaces: HashSet<WlSurface>,
    pub(crate) decorations: HashMap<WindowId, MemoryRenderBuffer>,
    /// The top border strip's rounded-corner bitmap, cached the same way
    /// and at the same trigger points as `decorations` (built in
    /// `redraw_decoration_buffer`) - see that method's doc comment for why
    /// this has to be rebuilt at the same points a titlebar is, and
    /// `elements::border_side_render_element`'s doc comment for the damage-
    /// tracking reason a per-frame rebuild was wrong in the first place.
    pub(crate) border_top_decorations: HashMap<WindowId, MemoryRenderBuffer>,
    /// A window's drop-shadow bitmap (`decoration::shadow_bitmap`), cached
    /// the same way and at the same trigger points as `border_top_decorations`
    /// - rebuilt only on creation or a real size change, not per frame, for
    /// the identical damage-tracking reason (a fresh `Id` every frame means
    /// `OutputDamageTracker` never finds a previous-frame match, so the
    /// shadow - like the border strips before this caching existed - would
    /// mark itself fully damaged forever, keeping the output page-flipping
    /// on an otherwise fully static screen). `None` for a maximized or
    /// fullscreen window, or with `general.shadows` off - see the render
    /// call site for why those don't get a shadow at all rather than a
    /// zero-alpha one.
    pub(crate) shadow_buffers: HashMap<WindowId, MemoryRenderBuffer>,
    /// The compiled rounded-corner GLES shader program (`rounded_corners::
    /// compile`), if that succeeded - `None` on the udev backend always
    /// (it never even tries, `PixmanRenderer` has no shader stage) and on
    /// winit only if compilation itself failed (an old/software GL driver
    /// missing something the shader needs), in which case content falls
    /// back to plain, unrounded rendering rather than the compositor
    /// refusing to start over a cosmetic feature. A concrete, non-generic
    /// smithay type (`GlesTexProgram`), so this field costs nothing to
    /// declare on the shared `CompState` even though only one backend ever
    /// populates it.
    pub(crate) rounded_corners_program: Option<smithay::backend::renderer::gles::GlesTexProgram>,
    /// Persistent solid-colour buffers backing a window's other three
    /// border strips (bottom, left, right - `decoration::border_strips`'
    /// order past index 0), reused by position every frame rather than
    /// rebuilt - see `elements::border_side_render_element`'s doc comment.
    /// A flat pool rather than a fixed `[_; 3]`, one buffer per rendered
    /// *fragment* rather than per strip: a strip occluded by a window
    /// stacked in front of it is split into however many visible pieces
    /// remain (see `elements::visible_border_fragments`), so the count
    /// needed varies frame to frame as windows move. Never shrunk once
    /// grown - a few idle unused buffers cost nothing meaningful, and
    /// dropping them would lose the damage-tracking stability the whole
    /// scheme exists for the moment fragment counts fluctuate back up.
    pub(crate) border_side_buffers: HashMap<WindowId, Vec<SolidColorBuffer>>,
    /// Client-visible size (`geometry` minus the titlebar band) last sent to
    /// each window via `xdg_toplevel.configure`. `sync_geometry` runs on
    /// every pointer-motion tick while a window is being dragged or resized
    /// (see `input::handle_pointer_position`); a plain move changes only
    /// position, not size, so without this it was re-sending a configure
    /// and re-rasterizing the titlebar's text from scratch on every single
    /// motion event of every drag, which is what made moving a window
    /// stutter. Only a real size change now does either.
    pub(crate) last_synced_size: HashMap<WindowId, (i32, i32)>,
    pub(crate) pending: Rc<RefCell<Vec<CoreEvent>>>,
    pub(crate) bound_keys: Rc<HashSet<String>>,
    /// Combos that repeat while held (`srd.bind_repeat`).
    pub(crate) repeat_keys: Rc<HashSet<String>>,
    /// The binding currently held down and repeating, if any.
    pub(crate) repeat: Option<RepeatState>,
    pub(crate) start_time: Instant,
    /// `Some` only for the udev/DRM backend; see `udev.rs` module docs for
    /// why its runtime state lives here rather than on a separate struct.
    pub(crate) udev: Option<udev::UdevState>,
    /// XWayland support; see `xwayland.rs` module docs. `xwm` is `None`
    /// until `XWaylandEvent::Ready` fires.
    pub(crate) xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    pub(crate) xwm: Option<smithay::xwayland::X11Wm>,
    pub(crate) xwayland_windows: HashMap<xwayland::X11Window, WindowId>,
    /// Mapped X11 windows still waiting for XWayland to associate a
    /// `wl_surface` - see `xwayland.rs` and `commit()` above.
    pub(crate) xwayland_pending: Vec<smithay::xwayland::X11Surface>,
    /// A second, independent connection to the XWayland X server, used to
    /// keep `_NET_ACTIVE_WINDOW`/`_NET_CLIENT_LIST`/`_NET_CLIENT_LIST_STACKING`
    /// on the root window up to date - see `xwayland::EwmhState` and its
    /// module docs for why this needs its own connection rather than going
    /// through `X11Wm`. `None` until XWayland is ready, same as `xwm`.
    pub(crate) ewmh: Option<xwayland::EwmhState>,
    /// `ext_idle_notify_v1` - lets a client (a lock daemon, a bar's idle
    /// indicator) ask to be told after N seconds of no real input. Both
    /// this and `_idle_inhibit_manager_state` below use smithay's own
    /// complete built-in modules (`wayland::idle_notify`/`idle_inhibit`),
    /// unlike every other hand-written protocol in this crate - neither
    /// had a raw-XML precedent to follow since smithay already ships full
    /// working implementations of both.
    pub(crate) idle_notifier_state: smithay::wayland::idle_notify::IdleNotifierState<CompState>,
    pub(crate) _idle_inhibit_manager_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState,
    /// Surfaces currently holding a live `zwp_idle_inhibitor_v1` (a video
    /// player's "keep the screen on while playing" request) - tracked so
    /// `uninhibit`/`toplevel_destroyed`/`remove_window` can tell whether any
    /// inhibitor is still alive after one goes away. Deliberately not
    /// workspace-visibility-aware (an inhibiting window on a workspace
    /// you've switched away from still keeps the system awake) - a real,
    /// smaller gap, but matching how several other real compositors treat
    /// this in practice, and far simpler than threading a re-check through
    /// every workspace-switch/minimize call site for a video-player-only
    /// protocol most sessions have at most one client using at a time.
    pub(crate) idle_inhibiting_surfaces: Vec<WlSurface>,
    /// Throttles `input::notify_idle_activity` - see its own doc comment
    /// for why pointer motion (a genuinely high-frequency event, and the
    /// event this session's earlier per-motion diagnostic-logging
    /// regression already proved is worth being careful around) needs one.
    pub(crate) last_idle_notify: Option<Instant>,
    /// Windows currently mid-tween - see `WindowAnim` and `sync_geometry`'s
    /// `anim_from` handling. Driven forward once per frame by
    /// `tick_animations`, called from both backends' poll loops.
    pub(crate) window_anims: HashMap<WindowId, WindowAnim>,
    /// Last (maximized, minimized, fullscreen) broadcast to
    /// `zwlr_foreign_toplevel_handle_v1` listeners for each window - see
    /// `foreign_toplevel::broadcast_dirty_state`'s doc comment for why this
    /// exists alongside that module's own immediate `send_state` calls.
    pub(crate) last_broadcast_flags: HashMap<WindowId, (bool, bool, bool)>,
    /// Last workspace id broadcast as active to `ext_workspace_v1`
    /// listeners - see `workspace::broadcast_dirty_active`'s doc comment.
    pub(crate) last_broadcast_workspace: Option<srdwm_core::WorkspaceId>,
}

/// An in-flight geometry tween for one window, driven by `tick_animations`.
///
/// Deliberately geometry-only (no alpha/scale-of-content): content is
/// composited through `self.space` like every other window (see
/// `resync_stacking_order`'s doc comment for why that path was chosen over
/// per-window custom render elements), which has no per-element opacity or
/// scale knob to animate independently of the rest of the output. What
/// *can* animate through `self.space` alone is exactly what interactive
/// drag/resize already proves out every frame: a `Window.geometry` change
/// applied via repeated `map_element`/`xdg_toplevel.configure` calls. This
/// reuses that same, already-live mechanism at a fixed frame rate instead
/// of on pointer motion.
pub(crate) struct WindowAnim {
    pub(crate) from: srdwm_core::Rect,
    pub(crate) to: srdwm_core::Rect,
    pub(crate) start: Instant,
    pub(crate) duration: Duration,
}

impl WindowAnim {
    /// Eased (ease-out-cubic) interpolation between `from` and `to`; past
    /// `duration` this returns `to` exactly, so a caller that keeps polling
    /// after completion never overshoots.
    pub(crate) fn current_rect(&self) -> srdwm_core::Rect {
        let t = (self.start.elapsed().as_secs_f64() / self.duration.as_secs_f64().max(0.001)).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let lerp = |a: i32, b: i32| a + ((b - a) as f64 * eased).round() as i32;
        let lerp_u = |a: u32, b: u32| (a as i64 + ((b as i64 - a as i64) as f64 * eased).round() as i64).max(0) as u32;
        srdwm_core::Rect {
            x: lerp(self.from.x, self.to.x),
            y: lerp(self.from.y, self.to.y),
            width: lerp_u(self.from.width, self.to.width),
            height: lerp_u(self.from.height, self.to.height),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.start.elapsed() >= self.duration
    }
}

/// How far below its resting position a newly-opened window starts before
/// sliding up into place, in logical pixels. Deliberately a pure position
/// offset with no size change (see `WindowAnim`'s doc comment on why a
/// resize tween is reserved for maximize/fullscreen, where the client is
/// already live and redrawing, not for a window whose first paint may not
/// have arrived yet).
const OPEN_SLIDE_OFFSET: i32 = 24;

/// A held keybinding that is firing repeatedly.
///
/// Driven from the poll loop rather than a timer source: the winit backend
/// has no `calloop` loop of its own, and `poll_events` already runs
/// continuously in both backends, so this works the same in each.
pub(crate) struct RepeatState {
    /// Which physical key is held - repeat stops when *this* key is
    /// released, not when any key is.
    pub(crate) keycode: smithay::input::keyboard::Keycode,
    pub(crate) key_name: String,
    pub(crate) modifiers: srdwm_core::Modifiers,
    pub(crate) next_fire: Instant,
}

/// Matches the seat's own repeat settings (`add_keyboard(.., 600, 25)`), so
/// held bindings feel the same as held keys in a text field.
///
/// 600ms, not smithay's own 200ms stock example value this used to copy --
/// found comparing against Hyprland's default (`repeat_delay = 600`) after
/// a live report that typing felt "too sensitive" compared to other
/// compositors on the same hardware. 200ms is short enough that a key held
/// even slightly past a fifth of a second - well within normal variance in
/// how long a real keystroke's finger-down/finger-up dwell actually is, let
/// alone under any momentary scheduling hiccup delaying when the release
/// gets processed - starts a client-side repeat and inserts an unintended
/// extra character, which reads indistinguishably from "double-typing".
/// This is entirely a client-side effect (repeat_info is sent once and the
/// client manages its own timer from then on, never re-driven by the
/// compositor per keystroke - see `crates/wayland/src/input.rs`'s
/// `handle_keyboard_key_event`), so it was never visible to the diagnostic
/// logging used earlier to rule out server-side event duplication.
const REPEAT_DELAY: Duration = Duration::from_millis(600);
const REPEAT_INTERVAL: Duration = Duration::from_millis(1000 / 25);

impl CompState {
    /// Starts repeating `combo` if it was registered with `srd.bind_repeat`.
    pub(crate) fn begin_repeat(&mut self, keycode: smithay::input::keyboard::Keycode, key_name: &str, modifiers: srdwm_core::Modifiers) {
        let combo = srdwm_core::key_combo_string(modifiers, key_name);
        if !self.repeat_keys.contains(&combo) {
            return;
        }
        self.repeat = Some(RepeatState {
            keycode,
            key_name: key_name.to_string(),
            modifiers,
            next_fire: Instant::now() + REPEAT_DELAY,
        });
    }

    /// Stops repeating when the held key is released.
    pub(crate) fn end_repeat(&mut self, keycode: smithay::input::keyboard::Keycode) {
        if self.repeat.as_ref().is_some_and(|r| r.keycode == keycode) {
            self.repeat = None;
        }
    }

    /// Emits another `KeyPress` if the held binding is due. Called once per
    /// poll from both backends.
    pub(crate) fn tick_repeat(&mut self) {
        let Some(repeat) = self.repeat.as_mut() else { return };
        let now = Instant::now();
        if now < repeat.next_fire {
            return;
        }
        repeat.next_fire = now + REPEAT_INTERVAL;
        let (key_name, modifiers) = (repeat.key_name.clone(), repeat.modifiers);
        self.pending.borrow_mut().push(CoreEvent::KeyPress { key_name, modifiers });
    }
}


/// The border (added this session, see `decoration::border_strips`) is a
/// much bigger, more obvious visual element than the titlebar's text
/// color, so this is what actually answers "which window is focused" --
/// reported live as genuinely unanswerable, since `Window.border_color` is
/// a single fixed color with no focus distinction at all, applied
/// identically to every window regardless of focus.
///
/// Dims the window's own configured colour toward gray rather than
/// replacing it outright with one fixed "unfocused" colour: per-window
/// `border_color` is a real, used feature (rules set distinct colours per
/// app), and dimming keeps that distinction visible at a glance while
/// still making focus unambiguous.
pub(crate) fn effective_border_color(configured: (u8, u8, u8), focused: bool) -> (u8, u8, u8) {
    if focused {
        return configured;
    }
    const DIM: f32 = 0.35;
    let dim = |c: u8| (c as f32 * DIM) as u8;
    (dim(configured.0), dim(configured.1), dim(configured.2))
}

/// Output lookup. Everything that used to reach for a single
/// `CompState::output` goes through one of these, so adding outputs did not
/// require every call site to learn about multiple ones.
impl CompState {
    /// The output new surfaces land on when nothing else determines it.
    /// First in the list, matching the udev backend's connector order.
    pub(crate) fn primary_output(&self) -> Option<&Output> {
        self.outputs.first().map(|e| &e.output)
    }

    /// The output containing a point in the global space - pointer
    /// hit-testing, and deciding which output a window belongs to. Falls
    /// back to the primary output if the point is outside every output
    /// (possible between mismatched-height monitors).
    pub(crate) fn output_at(&self, pos: Point<f64, Logical>) -> Option<&OutputEntry> {
        let point = pos.to_i32_round();
        self.outputs
            .iter()
            .find(|e| e.geometry().contains(point))
            .or_else(|| self.outputs.first())
    }

    /// Resolves a client-supplied `wl_output` to one of ours. Clients name
    /// outputs in layer-shell, session-lock and screencopy requests.
    pub(crate) fn output_for_wl(&self, wl: &WlOutput) -> Option<&OutputEntry> {
        let output = Output::from_resource(wl)?;
        self.outputs.iter().find(|e| e.output == output)
    }

    /// Iterator over the smithay outputs, for render loops.
    pub(crate) fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.outputs.iter().map(|e| &e.output)
    }
}

impl CompState {
    pub(crate) fn new_managed_window(&mut self, toplevel: ToplevelSurface) {
        let surface = toplevel.wl_surface().clone();
        let id = {
            let mut wm = self.wm.borrow_mut();
            let id = wm.alloc_window_id();
            let title = with_toplevel_title(toplevel.wl_surface()).unwrap_or_default();
            let mut w = CoreWindow::new(id, title);
            w.app_id = with_toplevel_app_id(toplevel.wl_surface()).unwrap_or_default();
            w.geometry = srdwm_core::Rect::new(0, 0, 800, 600 + TITLEBAR_HEIGHT as i32 as u32);
            wm.add_window(w);
            // Starts the open-slide tween (see `WindowAnim`'s doc comment):
            // the window's first `sync_geometry` call below will see this,
            // register the tween, and place it here - a few pixels below
            // its resting position - rather than jumping straight to
            // `geometry`. Same size throughout, so no extra client configure
            // is needed for the tween itself.
            if wm.animations_enabled {
                if let Some(win) = wm.window_mut(id) {
                    let g = win.geometry;
                    win.anim_from = Some(srdwm_core::Rect { y: g.y + OPEN_SLIDE_OFFSET, ..g });
                }
            }
            id
        };

        let dwindow = DWindow::new_wayland_window(toplevel.clone());
        self.surface_to_id.insert(surface.clone(), id);
        self.id_to_window.insert(id, dwindow);
        // `sync_geometry` handles the initial placement itself (map_element
        // + the first configure, since `last_synced_size` has no entry yet
        // for this id) as well as starting the open-slide tween registered
        // above - see its own doc comment.
        self.sync_geometry(id);
        self.redraw_decoration_buffer(id);
        // `WindowManager::add_window` already made this the focused window in
        // srdwm's own state, but that alone is purely internal bookkeeping --
        // without this, a freshly-opened window receives no keystrokes and
        // can't copy/paste until it's clicked, because nothing ever gave it
        // real Wayland keyboard/selection focus. (Same class of bug as the
        // click-to-focus one fixed earlier; this is the creation path.)
        self.set_keyboard_focus(Some(surface));
        // A newly-mapped window goes on top, but not over a pinned one.
        self.raise_pinned();
        self.pending.borrow_mut().push(CoreEvent::WindowCreated(id));
        foreign_toplevel::window_created(self, id);
    }

    /// Applies a negotiated `zxdg_toplevel_decoration_v1` mode to our own
    /// `Window.decorated` flag and refreshes (or drops) its titlebar buffer
    /// to match - see `XdgDecorationHandler::request_mode`'s doc comment
    /// for why. A no-op if the surface has no window yet (decoration
    /// negotiation racing ahead of `new_toplevel`, which shouldn't happen
    /// in practice but costs nothing to guard against).
    pub(crate) fn set_decorated_from_mode(&mut self, surface: &WlSurface, decorated: bool) {
        let Some(&id) = self.surface_to_id.get(surface) else { return };
        if let Some(w) = self.wm.borrow_mut().window_mut(id) {
            w.decorated = decorated;
        }
        self.redraw_decoration_buffer(id);
        // Re-applies content size/position for the now-changed titlebar
        // reservation - see `sync_geometry`'s own doc comment on why this
        // can't be skipped: redrawing the titlebar buffer alone doesn't
        // touch the content area's size or offset at all.
        self.sync_geometry(id);
    }

    /// (Re)renders the titlebar band for `id` - background plus title text
    /// via `decoration::render_titlebar` - and replaces the buffer in
    /// `self.decorations`. Called on creation, geometry change (width
    /// affects layout), and focus change (text color).
    pub(crate) fn redraw_decoration_buffer(&mut self, id: WindowId) {
        let Some(w) = self.wm.borrow().window(id).cloned() else { return };
        let focused = self.wm.borrow().focused_id() == Some(id);
        let theme = self.wm.borrow().theme;
        if w.decorated {
            let fg = if focused { theme.titlebar_fg_focused } else { theme.titlebar_fg_unfocused };
            let width = w.geometry.width.max(1);
            // Always rounded now, bordered or not - `render_border_top`
            // gives a bordered window's border strip the matching rounded
            // cut, so there's no more square-frame-around-a-round-titlebar
            // clash to avoid. See `render_titlebar`'s `round_corners` doc
            // comment.
            let data = decoration::render_titlebar(width, TITLEBAR_HEIGHT, &w.title, theme.titlebar_bg, fg, true);
            let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (width as i32, TITLEBAR_HEIGHT as i32), 1, Transform::Normal, None);
            self.decorations.insert(id, buffer);
        } else {
            self.decorations.remove(&id);
        }
        // The border-top bitmap is independent of `decorated` - an
        // undecorated (CSD) window can still have `border_width > 0` - so
        // it's rebuilt here unconditionally rather than falling under the
        // early return above. Cached the same way `decorations` is, at the
        // same trigger points (creation, a size change, a rule re-applying,
        // and - since this call is now also reached from focus changes --
        // `w.border_color`'s focused/unfocused dimming): see `elements::
        // border_side_render_element`'s doc comment for why re-rasterizing
        // this every render frame (an earlier version of this method did)
        // was a real, continuous cost, not just a redundant one.
        if w.border_width > 0 {
            let color = effective_border_color(w.border_color, focused);
            let strips = decoration::border_strips(w.geometry, w.border_width);
            if strips[0].width > 0 && strips[0].height > 0 {
                let data = decoration::render_border_top(strips[0].width, w.border_width, color);
                let buffer =
                    MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (strips[0].width as i32, w.border_width as i32), 1, Transform::Normal, None);
                self.border_top_decorations.insert(id, buffer);
            } else {
                self.border_top_decorations.remove(&id);
            }
        } else {
            self.border_top_decorations.remove(&id);
        }
        // No shadow for a maximized/fullscreen window: it already reaches
        // (or, for fullscreen, exceeds) the monitor's own edge, so there is
        // nowhere for `SHADOW_SIZE` pixels of shadow to actually fall, and
        // a shadow drawn there would either be clipped to nothing useful or
        // - for a maximized window short of the true monitor edge - read
        // as a shadow the window doesn't visually need. Matches the
        // Hyprland/GNOME convention `MISSING.md` measures this compositor
        // against.
        let shadows_enabled = self.wm.borrow().shadows_enabled;
        if shadows_enabled && !w.maximized && !w.fullscreen {
            let data = decoration::shadow_bitmap(w.geometry.width, w.geometry.height);
            let rect = decoration::shadow_rect(w.geometry);
            let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (rect.width as i32, rect.height as i32), 1, Transform::Normal, None);
            self.shadow_buffers.insert(id, buffer);
        } else {
            self.shadow_buffers.remove(&id);
        }
    }

    pub(crate) fn remove_window(&mut self, surface: &WlSurface) {
        let Some(id) = self.surface_to_id.remove(surface) else { return };
        if let Some(w) = self.id_to_window.remove(&id) {
            self.space.unmap_elem(&w);
        }
        self.decorations.remove(&id);
        self.border_top_decorations.remove(&id);
        self.shadow_buffers.remove(&id);
        self.border_side_buffers.remove(&id);
        self.last_synced_size.remove(&id);
        // A window closing (crash, kill, or its own menu's "Close" action
        // racing ahead of this) while its context menu is still open would
        // otherwise leave the menu pointing at a dead id - selecting any
        // row on it would then silently no-op against a window that no
        // longer exists, with no indication anything went wrong.
        if self.context_menu.as_ref().is_some_and(|m| m.window == id) {
            self.close_context_menu();
        }
        self.wm.borrow_mut().remove_window(id);
        self.pending.borrow_mut().push(CoreEvent::WindowDestroyed(id));
        foreign_toplevel::window_closed(self, id);
        // `remove_window` may have picked a new focused window on its own
        // (falls back to whatever's now on top) - see `sync_keyboard_focus`'s
        // doc comment for why the Wayland/X11 side needs a separate nudge to
        // actually catch up to that.
        crate::input::sync_keyboard_focus(self);
        // Safety net for `zwp_idle_inhibit_manager_v1`: smithay's own
        // `IdleInhibitorState` only calls `uninhibit` on an explicit
        // `destroy` request, never on `Dispatch::destroyed` - so a video
        // player that crashes or gets killed instead of exiting cleanly
        // would leave its inhibitor permanently stuck, holding the whole
        // system awake forever with no client left to ever release it.
        // Its window closing is the one thing guaranteed to happen either
        // way, so this is what actually catches that case.
        if self.idle_inhibiting_surfaces.contains(surface) {
            self.idle_inhibiting_surfaces.retain(|s| s != surface);
            self.idle_notifier_state.set_is_inhibited(!self.idle_inhibiting_surfaces.is_empty());
        }
    }

    /// Layer surfaces need a configure sent in direct response to their
    /// first commit (sending it any earlier violates the protocol - see
    /// `smithay::desktop::LayerMap::arrange`'s doc comment on why `arrange`
    /// itself deliberately won't send one). Also the point at which an
    /// `Exclusive`-interactivity layer (e.g. a lock screen, or a launcher
    /// configured to grab all keyboard input) claims keyboard focus, since
    /// its `keyboard_interactivity` isn't reliably known until the client's
    /// state has actually committed.
    pub(crate) fn ensure_layer_initial_configure(&mut self, surface: &WlSurface) {
        // Called unconditionally from `commit()` for every surface in the
        // whole desktop, on every single commit - so before doing anything
        // that scales with output/layer count, a cheap O(1) check: has this
        // surface ever gone through `get_layer_surface` at all? Only that
        // request ever inserts `LayerSurfaceData` into a surface's
        // `data_map` (smithay's own `handlers.rs`), so this is `None` for
        // every ordinary xdg-toplevel/subsurface commit - the overwhelming
        // majority of commits on any real desktop. Skipping straight past
        // the per-output `layer_for_surface` surface-tree walk for all of
        // those is the difference between this function costing something
        // on every single frame any window renders versus only on commits
        // from the handful of surfaces that were ever layer surfaces.
        if with_states(surface, |states| states.data_map.get::<LayerSurfaceData>().is_none()) {
            return;
        }
        // A layer surface lives in exactly one output's `LayerMap` (whichever
        // one `new_layer_surface` mapped it into), so find that output rather
        // than assuming a single global one.
        let found = self.outputs().find_map(|output| {
            let layer = layer_map_for_output(output).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned();
            layer.map(|l| (output.clone(), l))
        });
        // Not a layer surface (or a destroyed one - see `new_surface`'s
        // pre-commit-hook workaround, which is what stops this from being
        // a protocol error). Every ordinary commit from every window in
        // the desktop passes through here and takes this branch, so this
        // used to log unconditionally during the P0 investigation
        // (docs/PANEL_SUPPORT_TODO.md) - diagnostic purpose long since
        // served, and left running it logged upwards of 15k lines in a few
        // minutes of normal use (every Firefox frame, every terminal
        // redraw, ...), which is real wasted I/O, not just noise.
        let Some((output, layer)) = found else {
            return;
        };

        // Recompute geometry from whatever the client just committed
        // (`set_size`/`set_anchor`/`set_margin`/`set_exclusive_zone` are all
        // double-buffered, applied on this commit) *before* looking at
        // `initial_configure_sent` - `map_layer`'s own `arrange()` call ran
        // before the client had sent any of that, so without this, the
        // first configure would carry stale, pre-request-processed geometry
        // (verified live: wofi's `set_size(420, 550)` was otherwise ignored
        // and it got stuck at the half-output fallback size instead). Every
        // later commit needs the same treatment for live resizes/anchor
        // changes; `arrange()` only actually sends a configure when
        // something changed, so this is a no-op on a commit that didn't
        // touch layer-shell state.
        let zone_before = layer_map_for_output(&output).non_exclusive_zone();
        layer_map_for_output(&output).arrange();
        let zone_after = layer_map_for_output(&output).non_exclusive_zone();
        if zone_after != zone_before {
            // A bar/dock claiming (or releasing) an exclusive zone changes
            // the area core's placement/tiling should actually use --
            // without this, `WindowManager`'s notion of the monitor rect
            // is whatever `Platform::monitors()` returned once at startup
            // (before any layer-shell client had connected and set a real
            // exclusive zone), so every window keeps being placed across
            // the *whole* output including the strip a bar now occupies:
            // new windows spawn with their titlebar directly under the bar,
            // rendered beneath it and unreachable to drag. Reusing
            // `MonitorAdded` here rather than a new event type: main.rs's
            // handler for it already re-queries the full monitor list from
            // the platform rather than trusting the event's payload (see
            // its own comment on why), which is exactly "go recompute the
            // usable area" - the placeholder `Monitor` below is discarded
            // unread on that path.
            self.pending.borrow_mut().push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
        }

        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|d| d.lock().unwrap().initial_configure_sent)
                .unwrap_or(false)
        });
        if !initial_configure_sent {
            layer.layer_surface().send_configure();
            log::debug!("layer-shell: sent initial configure for surface {:?}", surface.id());
        }

        // Checked on every commit, not just the first: a client can flip
        // `keyboard_interactivity` to `Exclusive` after already being
        // mapped (and this is also, in practice, where a freshly-mapped
        // `Exclusive` surface - e.g. wofi, which requests it from the very
        // first commit - actually gets focus, since `set_keyboard_focus`
        // is idempotent against a surface that's already focused).
        if layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive {
            self.set_keyboard_focus(Some(surface.clone()));
        }
    }

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

    /// Opens the right-click titlebar window menu for `window`, top-left
    /// corner at `pos` (global space, wherever the click landed). Rebuilds
    /// and caches the rasterised buffer once here rather than per frame --
    /// same reasoning as `redraw_decoration_buffer`.
    pub(crate) fn open_context_menu(&mut self, window: WindowId, pos: (i32, i32)) {
        let Some(menu) = ({
            let wm = self.wm.borrow();
            crate::context_menu::ContextMenu::open(&wm, window, pos)
        }) else {
            return;
        };
        let theme = self.wm.borrow().theme;
        let items: Vec<(&str, bool)> = menu.items.iter().map(|&(label, _)| (label, false)).collect();
        let data = decoration::render_context_menu(menu.width, menu.row_height, &items, theme.titlebar_bg, theme.titlebar_fg_focused, theme.titlebar_fg_unfocused, theme.default_border_color);
        let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (menu.width as i32, menu.height()), 1, Transform::Normal, None);
        self.context_menu_buffer = Some(buffer);
        self.context_menu = Some(menu);
    }

    pub(crate) fn close_context_menu(&mut self) {
        self.context_menu = None;
        self.context_menu_buffer = None;
    }

    /// Runs whichever action a click on `row` of the currently-open context
    /// menu selected. Takes the menu's `window`/action rather than reading
    /// `self.context_menu` itself so the caller can close the menu first
    /// (clearing the borrow) before this runs - several of these actions
    /// (`sync_geometry`, `redraw_decoration_buffer`) need `&mut self` in
    /// ways that would otherwise conflict with an active `self.context_menu`
    /// borrow.
    pub(crate) fn run_context_menu_action(&mut self, window: WindowId, action: crate::context_menu::MenuAction) {
        use crate::context_menu::MenuAction;
        match action {
            MenuAction::Minimize => {
                self.wm.borrow_mut().minimize_window(window);
                foreign_toplevel::send_state(self, window);
            }
            MenuAction::ToggleMaximize => {
                self.wm.borrow_mut().toggle_maximize(window);
                self.sync_geometry(window);
                foreign_toplevel::send_state(self, window);
            }
            MenuAction::ToggleAlwaysOnTop => {
                self.wm.borrow_mut().toggle_always_on_top(window);
            }
            MenuAction::Close => {
                if let Some(w) = self.id_to_window.get(&window) {
                    crate::input::close_dwindow(w);
                }
            }
        }
    }

    /// True when this titlebar press is the second of a double-click on the
    /// same window. Threshold is the usual 400ms.
    pub(crate) fn is_double_click(&mut self, id: WindowId, time: u32) -> bool {
        const DOUBLE_CLICK_MS: u32 = 400;
        let doubled = match self.last_titlebar_click {
            Some((last_id, last_time)) => last_id == id && time.saturating_sub(last_time) <= DOUBLE_CLICK_MS,
            None => false,
        };
        // Reset after a double, so a third click starts a fresh pair rather
        // than counting as another double.
        self.last_titlebar_click = if doubled { None } else { Some((id, time)) };
        doubled
    }

    /// Re-raises always-on-top windows in the `Space`.
    ///
    /// `WindowManager` keeps pinned windows last in its own stacking order,
    /// but the `Space` has an order of its own that decides what actually
    /// draws on top - so pinning is only real once it is pushed here.
    /// Called after anything that raises a window.
    pub(crate) fn raise_pinned(&mut self) {
        let pinned: Vec<WindowId> = self.wm.borrow().stacking_order().filter(|w| w.always_on_top).map(|w| w.id).collect();
        for id in pinned {
            if let Some(w) = self.id_to_window.get(&id).cloned() {
                self.space.raise_element(&w, false);
            }
        }
    }

    pub(crate) fn sync_geometry(&mut self, id: WindowId) {
        // A pending `anim_from` (set by `toggle_maximize`/`toggle_fullscreen`,
        // or by `new_managed_window` for the open-slide) means the target
        // geometry below is where this window is *headed*, not where it
        // should appear right now - register (or replace) a tween and use
        // `WindowAnim::current_rect` in its place for this call and every
        // `tick_animations` call afterward, until it completes. `take()`
        // both reads and clears it, so a later, non-animated `sync_geometry`
        // call for the same window (an ordinary drag/resize frame) goes
        // straight back to applying `geometry` immediately, as before.
        let anim_from = self.wm.borrow_mut().window_mut(id).and_then(|w| w.anim_from.take());
        let Some((target, decorated)) = self.wm.borrow().window(id).map(|w| (w.geometry, w.decorated)) else { return };
        if let Some(from) = anim_from {
            let duration_ms = self.wm.borrow().animation_duration_ms;
            if from != target && duration_ms > 0 {
                self.window_anims
                    .insert(id, WindowAnim { from, to: target, start: Instant::now(), duration: Duration::from_millis(duration_ms as u64) });
            }
        }
        let geom = self.window_anims.get(&id).map(WindowAnim::current_rect).unwrap_or(target);
        // The titlebar band is only actually reserved when there is one --
        // an undecorated window (client-side decoration, see
        // `set_decorated_from_mode`) gets the whole of `geom` as content,
        // not `geom` minus a band that's no longer being drawn. Without
        // this, a window that negotiated client-side decoration kept the
        // same 30px gap at its top anyway: our titlebar wasn't drawn there
        // (correctly), but the content was still offset down and told it
        // was 30px shorter than the window actually is, leaving a blank
        // strip and the frame sitting visibly wrong relative to what's
        // inside it.
        let band = if decorated { TITLEBAR_HEIGHT as i32 } else { 0 };
        // Position always moves with the pointer; only a size change needs a
        // client configure or a titlebar re-render (see `last_synced_size`'s
        // doc comment).
        let size = (geom.width as i32, geom.height as i32 - band);
        let size_changed = self.last_synced_size.insert(id, size) != Some(size);
        let mut moved = false;
        if let Some(w) = self.id_to_window.get(&id) {
            self.space.map_element(w.clone(), (geom.x, geom.y + band), false);
            moved = true;
            if let Some(top) = w.toplevel() {
                // xdg-shell position is a purely compositor-side concept --
                // the client is never told it - so only a size change
                // needs a configure here.
                if size_changed {
                    top.with_pending_state(|state| {
                        state.size = Some(size.into());
                    });
                    top.send_configure();
                }
            } else if let Some(x11) = w.x11_surface() {
                // Unlike xdg-shell, an X11 client's real on-screen position
                // is part of its own window state - it has to be told on
                // every move, not just every resize, the same way a real
                // X11 window manager sends continuous `ConfigureNotify`
                // during an interactive drag. Without this branch at all,
                // `sync_geometry` never reconfigured an XWayland window a
                // second time past its initial map: `space.map_element`
                // above still moved smithay's own tracked position (see
                // `resync_stacking_order`'s doc comment for the real
                // z-order side effect that has, since fixed below) and the
                // border/titlebar still redrew at the new `Window.geometry`
                // (both read it fresh every frame), but the real X11
                // client window was never told to move or resize - any
                // drag, resize, maximize, edge-snap, or tiling re-layout of
                // an XWayland-backed app left its actual content frozen at
                // its original position/size forever while srdwm's own
                // decoration moved freely around it.
                let _ = x11.configure(Rectangle::new((geom.x, geom.y + band).into(), size.into()));
            }
        }
        if size_changed && self.decorations.contains_key(&id) {
            self.redraw_decoration_buffer(id);
        }
        // See `resync_stacking_order`'s doc comment: `map_element` above
        // always re-stacks its target to the top of `Space`'s own order as
        // a side effect of updating position, `activate` or not - and
        // `sync_geometry` runs for reasons with nothing to do with raising
        // a window (a title changing, an ordinary resize frame), so left
        // uncorrected this silently, non-deterministically desynced
        // `Space`'s notion of "on top" from `WindowManager`'s.
        if moved {
            self.resync_stacking_order();
        }
    }

    /// Re-broadcasts any dock/panel-facing protocol state that changed by a
    /// path with no direct hook back into this module - specifically a
    /// compositor keybinding via `crates/config`'s `WindowAction`/
    /// `srd.workspace.*` API, which only ever touches `WindowManager` and
    /// has no way to reach `CompState`. See `foreign_toplevel::
    /// broadcast_dirty_state` and `workspace::broadcast_dirty_active`'s own
    /// doc comments for the full story; called once per frame alongside
    /// `tick_animations`, from both backends' poll loops.
    pub(crate) fn tick_dirty_broadcasts(&mut self) {
        foreign_toplevel::broadcast_dirty_state(self);
        workspace::broadcast_dirty_active(self);
        output_management::broadcast_dirty_outputs(self);
    }

    /// Advances every in-flight `WindowAnim` by one frame; called once per
    /// redraw from both backends' poll loops. A finished tween is dropped
    /// *before* its final `sync_geometry` call, so that call lands exactly
    /// on `Window.geometry` (the authoritative target) rather than on
    /// whatever the eased curve's last sub-pixel step happened to be.
    pub(crate) fn tick_animations(&mut self) {
        if self.window_anims.is_empty() {
            return;
        }
        let ids: Vec<WindowId> = self.window_anims.keys().copied().collect();
        for id in ids {
            if self.window_anims.get(&id).is_some_and(WindowAnim::is_done) {
                self.window_anims.remove(&id);
            }
            self.sync_geometry(id);
        }
    }

    /// Re-applies `WindowManager`'s own stacking order to `Space`, bottom
    /// to top.
    ///
    /// `Space::map_element` (smithay 0.7.0) always re-stacks its target to
    /// the top of `Space`'s internal order as an unconditional side effect
    /// of updating its tracked position - true regardless of the
    /// `activate` argument, and there is no "move without restacking" in
    /// this smithay version. `sync_geometry` calls `map_element` for
    /// reasons that have nothing to do with raising a window at all (a
    /// title/app_id changing, an ordinary resize frame, a workspace
    /// switch), so every one of those silently raised the window it
    /// touched to the top of `Space`'s order regardless of which window
    /// `WindowManager`/the user actually considered focused or on top.
    /// Real-world effect, confirmed live: two windows created moments
    /// apart, each independently going through their own startup
    /// title/app_id negotiation, would each trigger a handful of
    /// `sync_geometry` calls purely from that startup sequence - so
    /// whichever one happened to settle *last* silently won `Space`'s
    /// notion of "on top", a race with no relationship to which window was
    /// actually focused. Reported live as a background window's content
    /// and decoration randomly painting in front of the actually-focused
    /// window on top of it. Calling this right after any `map_element`
    /// restores `Space`'s order to exactly match `WindowManager.order`
    /// (which `restack_pinned` already keeps pinned windows at the tail
    /// of), so the two can never drift apart again.
    fn resync_stacking_order(&mut self) {
        let order: Vec<WindowId> = self.wm.borrow().stacking_order().map(|w| w.id).collect();
        for id in order {
            if let Some(w) = self.id_to_window.get(&id).cloned() {
                self.space.raise_element(&w, false);
            }
        }
    }
}

pub(crate) fn with_toplevel_title(surface: &WlSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(surface, |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap().title.clone().unwrap_or_default())
    })
}

/// Same pattern as `with_toplevel_title`, for `app_id` - the xdg-shell
/// equivalent of `WM_CLASS`, and what `srd.rule({ class = ... })` matches
/// against (`crates/core/src/rules.rs`).
///
/// Nothing read this before: `new_managed_window` populated `Window.title`
/// from `with_toplevel_title` but never touched `Window.app_id` at all, so
/// every native Wayland window had an *empty* app_id the entire time rules
/// are evaluated (`WindowManager::add_window` matches rules once, at
/// creation). Every `class`-based rule - including the Firefox
/// `decorated = false` one meant to stop srdwm drawing a second titlebar
/// on top of Firefox's own - could therefore never match a native Wayland
/// client, only an XWayland one (`map_window_request` in xwayland.rs does
/// set `app_id` correctly, from `X11Surface::class()`). Reported live as
/// Firefox showing two titlebars, "same for other applications" - exactly
/// what this predicts, since it silently breaks every class-matched rule
/// for every native Wayland app, not just Firefox's.
pub(crate) fn with_toplevel_app_id(surface: &WlSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(surface, |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap().app_id.clone().unwrap_or_default())
    })
}

/// Re-reads title/app_id from the surface's own xdg-shell state and updates
/// `Window`/foreign-toplevel listeners if either changed since last read.
///
/// Needed because a fresh toplevel's `new_toplevel` fires at
/// `xdg_surface.get_toplevel()` - role assignment - which for essentially
/// every real client happens *before* the `set_title`/`set_app_id`/first
/// `commit()` sequence that actually supplies them. `new_managed_window`
/// reading those fields at that moment (see its own comment) reliably got
/// nothing: not a race, a fixed ordering every client hits, confirmed live
/// by a peer session capturing the raw `zwlr_foreign_toplevel_handle_v1`
/// wire output and finding `app_id`/`title` empty on every window. Unlike
/// `Window.geometry`/state (double-buffered per xdg-shell semantics),
/// title and app_id are plain immediate-apply requests in smithay's own
/// `XdgToplevelSurfaceData`, so re-reading them on every commit - cheap,
/// and this is already a per-commit hook - keeps `Window.title`/`app_id`
/// (and anything, like `srd.rule`, that reads them) correct from the first
/// real commit onward instead of frozen at an empty initial snapshot.
pub(crate) fn sync_toplevel_metadata(state: &mut CompState, id: WindowId, surface: &WlSurface) {
    let title = with_toplevel_title(surface).unwrap_or_default();
    let app_id = with_toplevel_app_id(surface).unwrap_or_default();
    let changed = {
        let mut wm = state.wm.borrow_mut();
        let Some(w) = wm.window_mut(id) else { return };
        let changed = w.title != title || w.app_id != app_id;
        w.title = title;
        w.app_id = app_id;
        changed
    };
    if changed {
        // Now that `title`/`app_id` are real, give `add_window`'s rule
        // match (which ran before either was set - see `Window::
        // rules_applied`'s doc comment) a real chance. Only the *first*
        // successful evaluation (rule actually matched, `Some` returned)
        // warrants `redraw_decoration_buffer`/`sync_geometry` - unlike
        // `set_decorated_from_mode`, this fires on every subsequent title
        // change too (a page finishing loading, long after the window's
        // own creation), and `sync_geometry` re-stacks the window to the
        // top of smithay's `Space` as a side effect of `map_element`
        // (true regardless of its `activate` argument - there's no
        // "move without restacking" in this smithay version). Calling it
        // unconditionally here silently yanked an unrelated, unfocused
        // window back to the front any time its title happened to
        // update - reported live as an older window jumping in front of
        // a newer, focused one with no user action to explain it.
        if state.wm.borrow_mut().reapply_rules_if_pending(id) {
            state.redraw_decoration_buffer(id);
            state.sync_geometry(id);
        }
        crate::foreign_toplevel::send_state(state, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_window_keeps_its_configured_colour() {
        assert_eq!(effective_border_color((136, 192, 208), true), (136, 192, 208));
    }

    #[test]
    fn unfocused_window_is_dimmed_but_still_recognisably_that_colour() {
        let dimmed = effective_border_color((136, 192, 208), false);
        // Dimmer in every channel...
        assert!(dimmed.0 < 136 && dimmed.1 < 192 && dimmed.2 < 208);
        // ...but not black, and the channels' relative order is preserved
        // (still "bluish", not just "gray") so a per-window colour set via
        // a rule stays distinguishable from another window's even while
        // unfocused.
        assert!(dimmed.0 > 0 || dimmed.1 > 0 || dimmed.2 > 0);
        assert!(dimmed.2 >= dimmed.1 && dimmed.1 >= dimmed.0);
    }

    #[test]
    fn window_anim_starts_at_from_and_ends_at_to() {
        let anim = WindowAnim {
            from: srdwm_core::Rect::new(0, 100, 300, 200),
            to: srdwm_core::Rect::new(0, 0, 300, 200),
            start: Instant::now(),
            duration: Duration::from_millis(200),
        };
        assert_eq!(anim.current_rect(), anim.from);
        assert!(!anim.is_done());
    }

    #[test]
    fn window_anim_is_done_and_settles_exactly_on_to_once_duration_elapses() {
        let anim = WindowAnim {
            from: srdwm_core::Rect::new(0, 100, 300, 200),
            to: srdwm_core::Rect::new(50, 0, 600, 400),
            start: Instant::now() - Duration::from_millis(500),
            duration: Duration::from_millis(200),
        };
        assert!(anim.is_done());
        assert_eq!(anim.current_rect(), anim.to);
    }

    #[test]
    fn window_anim_midway_is_strictly_between_from_and_to_on_every_axis() {
        let anim = WindowAnim {
            from: srdwm_core::Rect::new(0, 200, 200, 100),
            to: srdwm_core::Rect::new(100, 0, 800, 600),
            start: Instant::now() - Duration::from_millis(100),
            duration: Duration::from_millis(200),
        };
        let r = anim.current_rect();
        assert!(r.x > 0 && r.x < 100);
        assert!(r.y > 0 && r.y < 200);
        assert!(r.width > 200 && r.width < 800);
        assert!(r.height > 100 && r.height < 600);
    }
}
