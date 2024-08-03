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
    /// Bumped once per real `commit()` of a mapped window's surface --
    /// see that handler in `protocols.rs`. The only signal `rounded_content_buffers`
    /// needs to know its cached masked copy is stale, since content changes
    /// (unlike geometry, which `redraw_decoration_buffer`'s trigger points
    /// already cover) can arrive on every single frame for a video or
    /// terminal, with nothing else in this struct tracking that.
    pub(crate) content_epoch: HashMap<WindowId, u64>,
    /// The udev/Pixman-backend rounded-corner masked copy of a window's own
    /// content (`rounded_corners_pixman::masked_content_buffer`), paired
    /// with the `content_epoch` value it was built from - see
    /// `elements::rounded_content_buffer`, which owns rebuilding this.
    /// Always empty on the winit backend (GLES rounds via a shader instead,
    /// `rounded_corners_program`), but costs nothing to declare here
    /// unconditionally, the same call `rounded_corners_program` itself
    /// already makes.
    pub(crate) rounded_content_buffers: HashMap<WindowId, (u64, MemoryRenderBuffer)>,
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
    /// `Some` only for the udev/DRM backend; see `udev/mod.rs` module docs for
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


mod focus;
mod geometry;
mod layers;
mod lifecycle;
mod menu;
mod tick;
mod toplevel;

pub(crate) use toplevel::{sync_toplevel_metadata, with_toplevel_app_id, with_toplevel_title};

#[cfg(test)]
mod tests;
