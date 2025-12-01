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
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::xdg_activation::XdgActivationState;
use wayland_protocols_wlr::virtual_pointer::v1::server::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use srdwm_core::{Event as CoreEvent, SnapZoneKind, Window as CoreWindow, WindowId, WindowManager, TITLEBAR_HEIGHT};

use crate::lock::SessionLock;
use crate::{appmenu, decoration, foreign_toplevel, gamma_control, output_management, output_power, screencopy, udev, workspace, xwayland};

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

/// Every input `redraw_decoration_buffer` reads to decide what a window's
/// titlebar/border pixels look like - see `CompState::decoration_
/// signatures`'s own doc comment for why this exists. `title` is the one
/// field worth noting the cost of cloning: short in practice (a window
/// title), and only compared/cloned once per call to this already-more-
/// expensive-than-a-string-clone rasterization function, not once per
/// frame.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DecorationSignature {
    pub(crate) width: u32,
    /// The shadow bitmap's own inputs (`height`, `maximized`, `fullscreen`,
    /// and the global `shadows_enabled` setting) belong here too, even
    /// though nothing above the titlebar/border needs them - one
    /// signature covering every input this function reads, not one that
    /// only happens to match the titlebar/border's inputs and silently
    /// skips a shadow update a real state change needed.
    pub(crate) height: u32,
    pub(crate) decorated: bool,
    pub(crate) focused: bool,
    pub(crate) title: String,
    pub(crate) border_color: (u8, u8, u8),
    pub(crate) border_width: u32,
    pub(crate) corner_radius: u32,
    pub(crate) maximized: bool,
    pub(crate) fullscreen: bool,
    pub(crate) shadows_enabled: bool,
    /// Which of *this* window's own titlebar buttons (if any) is currently
    /// hovered, and the glyph-reveal animation's current progress (0..=255)
    /// - see `CompState::hovered_titlebar_button`'s own doc comment.
    /// Included here, progress and all, so hovering (or un-hovering) a
    /// button - and every intermediate frame of the reveal animating in
    /// between - is a real signature change, not silently absorbed by the
    /// cache this struct exists to drive; a signature that only recorded
    /// *which* button was hovered, not the animation's own progress, would
    /// cache the very first frame of the reveal and never rebuild again
    /// for the rest of it.
    pub(crate) hovered_button: Option<(srdwm_core::TitlebarHit, u8)>,
    /// `theme.title_centered` at the time this was rendered - a live
    /// `srd`-side theme change (there's no `srd set` for this yet, but
    /// nothing here assumes there never will be) must still invalidate the
    /// cache like every other themed input already does.
    pub(crate) title_centered: bool,
    /// `theme.buttons_left` at render time - same reasoning as `title_
    /// centered` above.
    pub(crate) buttons_left: bool,
    /// `theme.button_glyph_always`/`theme.button_order`/`theme.
    /// traffic_light_buttons` at render time - same reasoning as `title_
    /// centered`/`buttons_left` above (no `srd set` for any of the three
    /// yet either), and the same real gap those two fields were added to
    /// close: all three are passed straight into `render_titlebar`
    /// (`redraw_decoration_buffer`'s own call site) but were missing from
    /// this struct entirely until a full-pipeline audit found the mismatch
    /// - a live change to any of the three would have compared equal
    /// against a stale signature and silently never rebuilt the titlebar
    /// this window already has cached.
    pub(crate) button_glyph_always: bool,
    pub(crate) button_order: Option<srdwm_core::ButtonOrder>,
    pub(crate) traffic_light_buttons: bool,
    /// `Window::is_dialog` at render time - a client can call `xdg_
    /// toplevel.set_parent` well after its own initial map (a "Save As"
    /// dialog opened from an already-open main window, say), so this needs
    /// the same cache-invalidation treatment as every other live-
    /// changeable input here, not just a value read once at creation.
    pub(crate) is_dialog: bool,
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
    pub(crate) _virtual_pointer_state: crate::virtual_pointer::VirtualPointerState,
    /// Captures requested via `wlr-screencopy` but not yet serviced; drained
    /// inside the render pass (see `screencopy::service_pending`).
    pub(crate) screencopy_pending: Vec<screencopy::PendingCapture>,
    /// `org_kde_kwin_appmenu_manager` - not `Option`-gated, same reasoning
    /// as `_output_management_state` below: exporting a menu D-Bus address
    /// straight from a Wayland-native client has nothing GPU/DRM-specific
    /// about it, so both backends advertise it. See `appmenu.rs`'s module
    /// doc comment for why this exists alongside `xwayland.rs::read_global_
    /// menu` rather than instead of it - they cover disjoint sets of
    /// windows (XWayland-backed vs. Wayland-native), not the same one.
    pub(crate) _appmenu_state: appmenu::AppmenuManagerState,
    /// `zwp_virtual_keyboard_manager_v1` - lets a client (`wtype`, `ydotool
    /// type`, an accessibility tool, AGS's own global-menu shortcut items)
    /// inject synthetic key events through the exact same keyboard-focus/
    /// keymap pipeline a real key press already goes through, rather than
    /// needing a compositor-specific IPC of its own. Not `Option`-gated,
    /// same reasoning as `_appmenu_state` just above: injecting a key event
    /// has nothing GPU/DRM-specific about it either. Smithay's own
    /// `wayland::virtual_keyboard` module provides the full protocol
    /// implementation (`delegate_virtual_keyboard_manager!` in
    /// `protocols.rs` wires it up); this compositor only supplies the
    /// global itself. Absence of this was reported live as "most options in
    /// global menu don't work" - every keyboard-shortcut item there is
    /// delivered via `wtype`, which silently does nothing at all without
    /// this protocol (`wtype ""` exits 1 with "Compositor does not support
    /// the virtual keyboard protocol"), a failure the caller (AGS, fire-
    /// and-forget) never even saw.
    pub(crate) _virtual_keyboard_state: smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState,
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
    /// Whether `cursor_status`'s current value was last set by us (hovering
    /// our own decoration's resize edge/drag area) rather than by a client's
    /// own `wl_pointer.set_cursor` request - see `input.rs::update_cursor_
    /// shape`'s doc comment for the bug this exists to fix: without it, a
    /// resize icon forced while hovering a decoration edge stayed on screen
    /// indefinitely once the pointer moved onto plain client content,
    /// because nothing about moving onto content gives the client any
    /// reason to call `set_cursor` again itself.
    pub(crate) decoration_cursor_active: bool,
    /// Built-in cursor bitmaps (arrow, text, resize directions), built once
    /// at startup rather than per frame.
    pub(crate) cursor_buffers: crate::cursor::CursorBuffers,
    /// Last titlebar press, for double-click detection.
    pub(crate) last_titlebar_click: Option<(WindowId, u32)>,
    /// Finger count and accumulated horizontal offset of an in-progress
    /// touchpad swipe (`GestureSwipeBegin`..`GestureSwipeUpdate`*..
    /// `GestureSwipeEnd`) - `None` between gestures. Finger count is only
    /// ever reported on the `Begin` event, so it has to be carried forward
    /// to be checked at `End`. `GestureSwipeUpdateEvent::delta_x` is a
    /// per-update offset, not a running total (see the smithay struct's own
    /// doc comment: "relative to the previous event"), so the offset half
    /// has to sum across every update itself; only the total at `End`
    /// decides whether the swipe crossed the switch-workspace threshold.
    /// Never forwarded to a client - see `input::handle_gesture_swipe_end`'s
    /// doc comment for why 3+-finger swipe is claimed entirely by the
    /// compositor.
    pub(crate) gesture_swipe: Option<(u32, f64)>,
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
    /// The Snap-Layouts flyout, if one is currently open - see
    /// `snap_flyout.rs`. Same lifecycle as `context_menu` above (mutually
    /// exclusive in practice, since both close on any click elsewhere), just
    /// a separate field rather than an enum of the two: they render
    /// differently, are triggered by different clicks, and nothing needs to
    /// treat them uniformly.
    pub(crate) snap_flyout: Option<crate::snap_flyout::SnapFlyout>,
    /// Rasterised pixels for the currently-open `snap_flyout`, same
    /// build-once-on-open pattern as `context_menu_buffer`.
    pub(crate) snap_flyout_buffer: Option<MemoryRenderBuffer>,
    /// The real desktop icons (Home/Computer/Trash plus `~/Desktop`'s own
    /// contents) - see `desktop_icons.rs`. `None` until the first render
    /// pass populates it (lazily, once the primary monitor's own geometry
    /// is actually known - see `state/desktop_icons.rs::ensure_desktop_
    /// icons`), and permanently `None` when `general.desktop_icons` is off.
    pub(crate) desktop_icons: Option<crate::desktop_icons::DesktopIcons>,
    /// Rasterised pixels per icon, keyed by `DesktopIcon::id` - rebuilt
    /// only for the one icon whose selection/drag state actually changed,
    /// same cached-until-dirty convention as every other decoration
    /// buffer in this codebase.
    pub(crate) desktop_icon_buffers: HashMap<String, MemoryRenderBuffer>,
    /// An in-progress icon drag - see `desktop_icons::DesktopIconDrag`'s
    /// own doc comment for the full shape (it may carry more than one
    /// icon, when the drag started on an already-multi-selected icon).
    /// `None` whenever no drag is active.
    pub(crate) desktop_icon_drag: Option<crate::desktop_icons::DesktopIconDrag>,
    /// An active rubber-band/marquee selection drag on bare desktop --
    /// `(start, current)`, both global-space pointer positions. The one
    /// "click and drag" desktop interaction this compositor never had at
    /// all (only single-icon click-select existed) - reported live next
    /// to "missing click and drag stuff like from windows". `None`
    /// whenever no marquee is active. See `start_desktop_marquee`/
    /// `update_desktop_marquee`/`end_desktop_marquee`.
    pub(crate) desktop_marquee: Option<((i32, i32), (i32, i32))>,
    /// Four thin solid-color strips forming the marquee's own rectangle
    /// outline (top/bottom/left/right) - same "keep a persistent `Solid
    /// ColorBuffer` per strip, update it in place every frame" pattern
    /// `border_side_buffers` already uses for window borders, reused here
    /// rather than allocating a fresh buffer on every motion tick.
    pub(crate) marquee_buffers: [SolidColorBuffer; 4],
    /// The right-click desktop-icon/bare-desktop menu, if one is currently
    /// open - see `desktop_menu.rs`. Same lifecycle/mutual-exclusion
    /// story as `context_menu`/`snap_flyout` above.
    pub(crate) desktop_menu: Option<crate::desktop_menu::DesktopMenu>,
    /// Same build-once-on-open pattern as `context_menu_buffer`.
    pub(crate) desktop_menu_buffer: Option<MemoryRenderBuffer>,
    /// Same double-click bookkeeping as `last_titlebar_click`, keyed by
    /// `DesktopIcon::id` instead of `WindowId` since a desktop icon isn't
    /// a window - see `CompState::is_double_click`'s own doc comment.
    pub(crate) last_icon_click: Option<(String, u32)>,
    /// An in-progress inline rename: the icon's own id, and the live-
    /// edited name buffer - same shape as `NativeLock::password`
    /// (`native_lock.rs`), the existing precedent for redirecting real
    /// keyboard input into a plain string buffer instead of the normally-
    /// focused client. `None` whenever no rename is in progress; the
    /// keyboard handler checks this the same way it already checks
    /// `state.lock.locked`.
    pub(crate) renaming_icon: Option<(String, String)>,
    pub(crate) wm: Rc<RefCell<WindowManager>>,
    pub(crate) surface_to_id: HashMap<WlSurface, WindowId>,
    pub(crate) id_to_window: HashMap<WindowId, DWindow>,
    /// Every live `zwlr_virtual_pointer_v1` object, so `set_virtual_pointer_
    /// pin` (`virtual_pointer.rs`) can find every pointer a given client
    /// (identified by pid, via `Client::get_credentials`) owns without a
    /// second, redundant per-client map - see that module's own doc
    /// comment for why pid, not an opaque per-object id nothing outside
    /// this compositor could otherwise learn, is the pinning handle. Pruned
    /// lazily (a destroyed resource's own methods become no-ops, and dead
    /// entries are filtered out the next time this is walked) rather than
    /// on every single destroy - this list is only ever touched by an
    /// infrequent pin/unpin request, never a hot path.
    pub(crate) virtual_pointers: Vec<ZwlrVirtualPointerV1>,
    /// Surfaces whose `zwlr_layer_surface_v1` role has been destroyed --
    /// consulted by the pre-commit hook `CompositorHandler::new_surface`
    /// registers (see its doc comment) to work around a real smithay bug
    /// where a later commit of one of these surfaces gets spuriously
    /// rejected. Entries are added in `layer_destroyed`; there's
    /// deliberately no removal, since a `WlSurface` here is meaningless
    /// after the client destroys it too and Rust never reuses the id while
    /// any handle (including this one) still exists.
    pub(crate) dead_layer_surfaces: HashSet<WlSurface>,
    /// Layer surfaces this compositor has unmapped itself in response to a
    /// null-buffer commit - `wlr-layer-shell-unstable-v1.xml`'s own text:
    /// "Attaching a null buffer to a layer surface unmaps it", but nothing
    /// in smithay's `LayerMap` does that automatically (`arrange()` walks
    /// every layer in its list unconditionally, buffer or not; only an
    /// explicit `unmap_layer` call removes one). `layer_destroyed` already
    /// did this for the surface-destroyed case; `sync_layer_visibility`
    /// (state/layers.rs) does it for the hide-without-destroying one --
    /// AGS's dock, hiding for a fullscreen window, being the live case
    /// that surfaced this. Stores the output it was unmapped from plus the
    /// `LayerSurface` handle itself: `unmap_layer` removes it from
    /// `LayerMap`'s own list, so `layer_for_surface` can never find it
    /// again on its own - this is the only way `sync_layer_visibility`
    /// can re-map it once the client commits real content again.
    pub(crate) hidden_layer_surfaces: HashMap<WlSurface, (smithay::output::Output, smithay::desktop::LayerSurface)>,
    /// Layer surfaces `sync_layer_visibility` has seen commit an actual
    /// buffer at least once. A layer-shell client's realization sequence is
    /// (commit with no buffer -> receive configure -> commit with no buffer
    /// again to ack it -> *then* attach and commit real content), and that
    /// middle ack-commit is indistinguishable from a real "hide" (a null-
    /// buffer commit on an already-visible surface) by buffer-presence
    /// alone - both are "committed, no buffer". Without this, every
    /// layer-shell surface's very first realization spuriously unmapped and
    /// immediately remapped itself through `sync_layer_visibility`, doubling
    /// the number of `LayerMap::arrange()` passes on every single popup
    /// open (confirmed live: an AGS popup toggle logged unmapped/re-mapped
    /// within the same ~300ms window every time) and giving a second,
    /// needless remap for `arrange()`'s zone/size math to disagree with
    /// itself across - the leading suspect for a live-reproduced bug where
    /// a full-monitor click-catcher popup's hit-tested geometry came back
    /// wider than the real output after several open/close cycles. Real
    /// hides (a role kept alive, buffer later reattached) still work:
    /// `sync_layer_visibility`'s own `has_buffer` branch inserts here before
    /// this set is ever consulted, so a surface only reaches the unmap path
    /// once it has legitimately shown something.
    pub(crate) layer_surfaces_shown_once: HashSet<WlSurface>,
    pub(crate) decorations: HashMap<WindowId, MemoryRenderBuffer>,
    /// The top border strip's rounded-corner bitmap, cached the same way
    /// and at the same trigger points as `decorations` (built in
    /// `redraw_decoration_buffer`) - see that method's doc comment for why
    /// this has to be rebuilt at the same points a titlebar is, and
    /// `elements::border_side_render_element`'s doc comment for the damage-
    /// tracking reason a per-frame rebuild was wrong in the first place.
    pub(crate) border_top_decorations: HashMap<WindowId, MemoryRenderBuffer>,
    /// [`Self::border_top_decorations`]'s mirror for the bottom strip's own
    /// two corners - same cache, same trigger points, same reasoning.
    pub(crate) border_bottom_decorations: HashMap<WindowId, MemoryRenderBuffer>,
    /// What `redraw_decoration_buffer` last actually rendered for a window
    /// - every input its own rasterization reads (width, `decorated`,
    /// focus, title text, border colour/width) - so a call that would
    /// rebuild the exact same pixels can skip doing so instead.
    ///
    /// Exists because `main.rs`'s `sync()` calls `Platform::redraw_
    /// decoration` - which always reaches this - for *every visible
    /// window*, on *every* tick that has anything at all marked dirty, not
    /// only the window whose state actually changed: a resize drag alone
    /// fires this for every other open window too, once per pointer-motion
    /// event, each one re-rendering title text and re-rasterizing border
    /// strips into a freshly allocated buffer for no visible difference.
    /// The `decorations`/`border_*_decorations` doc comments already
    /// establish "only rebuild at real trigger points" as the intended
    /// contract; this closes the gap between that intent and `sync()`'s
    /// own blanket call, which never actually checked whether this
    /// specific window was one of the windows that triggered the tick.
    pub(crate) decoration_signatures: HashMap<WindowId, DecorationSignature>,
    /// When `handle_pointer_position` last called `redraw_decoration_buffer`
    /// for the window currently being interactively resized - throttles
    /// that call to once per `RESIZE_REDRAW_INTERVAL` (see `input::pointer`),
    /// since a pointer
    /// can emit motion events far faster than a titlebar's text and border
    /// bitmaps are worth re-rasterizing. Without this the decoration buffer
    /// only catches up with `effective_frame_of`'s now-live resize geometry
    /// (see that function's own doc comment) once the drag ends and the
    /// blanket `sync()` redraw runs - correct, but visibly laggy borders
    /// for the whole drag. `None` whenever no resize is in progress; reset
    /// there rather than left stale, so a *new* resize's first motion event
    /// always redraws immediately instead of inheriting a stale timestamp
    /// from a previous drag.
    pub(crate) resize_redraw_at: Option<std::time::Instant>,
    /// Which titlebar button (if any) the pointer is currently over, on
    /// which window, and *when that hover started* - set from `handle_
    /// pointer_position`'s own `hit_test` result, read by `redraw_
    /// decoration_buffer` to brighten that one button's dot and animate
    /// its glyph in (see `decoration::render_titlebar`'s `hovered`
    /// parameter). Explicitly requested background-highlight-on-hover
    /// behaviour for the titlebar buttons, previously unimplemented - see
    /// `docs/TODO.md`. A single `Option`, not a per-window map: only one
    /// button can plausibly be hovered at a time, across every window.
    /// The `Instant` is *only* updated when the hovered button itself
    /// changes (see the comparison at its own call site, which ignores
    /// this field) - it marks "hover started here", not "last motion
    /// event", so `tick_hover_glyph_animation` can measure real elapsed
    /// hover time instead of resetting every frame the pointer so much as
    /// twitches while still over the same button.
    pub(crate) hovered_titlebar_button: Option<(WindowId, srdwm_core::TitlebarHit, Instant)>,
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
    /// content (`rounded_corners_pixman::masked_content_buffer`), keyed by
    /// everything that can make a rebuilt-from-scratch copy necessary --
    /// see `elements::rounded_content_buffer`, which owns rebuilding this.
    /// In order: the `content_epoch` value it was built from (bumped once
    /// per real client commit); the `corner_radius` it was built from, in
    /// bit-cast `u32` form (`f32` has no `Eq`) - live-settable (`srd set
    /// corner_radius`/a rule) without any client commit, so `content_epoch`
    /// alone wouldn't notice a change; the tree-render `loc` it was built
    /// from (the negated `content_offset`, changes if a client alters its
    /// own declared shadow-margin geometry); and the off-screen buffer
    /// `size` it was built at (the window's own content dimensions --
    /// stale the moment those change, same reason `redraw_decoration_
    /// buffer`'s own signature check exists for the titlebar/border
    /// bitmaps). Always empty on the winit backend (GLES rounds via a
    /// shader instead, `rounded_corners_program`), but costs nothing to
    /// declare here unconditionally, the same call `rounded_corners_
    /// program` itself already makes.
    pub(crate) rounded_content_buffers: crate::elements::RoundedContentCache,
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
    /// Persistent solid-colour buffer backing the whole-output night-light/
    /// reading-mode overlay, one per output name - same "reuse the buffer
    /// so its `Id` stays stable across frames" reasoning as `border_side_
    /// buffers` above. See `color_filter::render_element`.
    pub(crate) color_filter_buffers: HashMap<String, SolidColorBuffer>,
    /// Client-visible size (`geometry` minus the titlebar band, converted
    /// to logical points for whichever monitor the window is currently on
    /// - see `sync_geometry`'s own doc comment) last sent to each window
    /// via `xdg_toplevel.configure`. `sync_geometry` runs on
    /// every pointer-motion tick while a window is being dragged or resized
    /// (see `input::handle_pointer_position`); a plain move changes only
    /// position, not size, so without this it was re-sending a configure
    /// and re-rasterizing the titlebar's text from scratch on every single
    /// motion event of every drag, which is what made moving a window
    /// stutter. Only a real size change now does either.
    pub(crate) last_synced_size: HashMap<WindowId, (i32, i32)>,
    /// Windows whose `Window::size_is_provisional` was `true` at creation
    /// and whose client hasn't sent a real, non-empty content commit yet --
    /// see that field's own doc comment. `sync_geometry` sends `size: None`
    /// (let the client pick) instead of forcing this guessed size for the
    /// one configure sent while a window is in this set; `commit()` removes
    /// it and adopts the client's own real first size into `Window::
    /// geometry` the moment one arrives.
    pub(crate) provisional_size: HashSet<WindowId>,
    /// A size-changing `xdg_toplevel.configure` that's been sent but not
    /// yet reflected in the client's own real committed content size --
    /// `(size requested, when it was sent)`. `sync_geometry` won't send
    /// *another* size-changing configure for the same window while an
    /// entry is still here (unless `CONFIGURE_THROTTLE_TIMEOUT` has
    /// elapsed - see that constant's own doc comment for why this can
    /// never wedge resize entirely).
    ///
    /// Niri throttles the same way (`window/mapped.rs`'s `ConfigureIntent::
    /// Throttled`, keyed on the configure serial rather than a size/time
    /// pair, but the same idea) - its own comment: "some clients do not
    /// batch size requests, leading to bad behavior with very fast input
    /// devices... this throttling also helps interactive resize
    /// transactions preserve visual consistency." srdwm had no equivalent
    /// at all: `sync_geometry` runs on every pointer-motion tick of an
    /// active resize and only ever compared the newly-requested size
    /// against the *previous request*, never against what the client had
    /// actually caught up to - a fast drag (a real high-poll-rate mouse,
    /// confirmed as niri's own stated motivation) could queue several
    /// configures before the client acknowledged the first, the exact
    /// backlog this field exists to prevent.
    pub(crate) pending_size_configure: HashMap<WindowId, ((i32, i32), Instant)>,
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
    /// `com.canonical.AppMenu.Registrar` - the classic Qt/`appmenu-qt5`
    /// global-menu source, see `srdwm_platform::appmenu_registrar`'s module
    /// doc comment for why it lives in the shared platform crate rather
    /// than here. `None` until XWayland is ready, same as `ewmh`/`xwm` --
    /// constructed alongside `ewmh` in `xwayland.rs::spawn`'s `XWaylandEvent
    /// ::Ready` handler, since a raw X11 window id is meaningless without it.
    pub(crate) appmenu_registrar: Option<srdwm_platform::AppmenuRegistrarState>,
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
    /// The Wayland-standard "implicit grab": once a pointer button goes
    /// down over a surface, every subsequent motion/button event - no
    /// matter where the pointer physically ends up - has to keep being
    /// delivered to that *same* surface until every held button is
    /// released, not whatever a fresh hit-test happens to land on next.
    /// Nothing implemented this before: every motion event re-ran the same
    /// popup/layer/content hit-test from scratch and called `pointer.
    /// motion()` with whatever it found *right now*, so the moment a real
    /// human's hand drifted even slightly outside the pressed surface's own
    /// bounds mid-gesture (trivially easy during a fast drag - a mouse
    /// does not move in a perfectly straight line), that client received an
    /// unrequested `leave` in the middle of its own gesture. GTK's drag
    /// recognizers (a `GtkHeaderBar`'s move-the-window gesture, concretely)
    /// treat a mid-gesture `leave` as "this isn't coherent, abort" - which
    /// reads as "dragging this window's title bar does nothing at all,"
    /// live-reproduced this session. `(surface, origin)` is captured once,
    /// from the same resolution `refresh_pointer_focus` already computes,
    /// the instant the held-button count goes from 0 to 1; `origin` is
    /// reused for every event under the grab so surface-local coordinates
    /// keep updating correctly even though the target surface no longer
    /// does.
    pub(crate) pointer_button_grab: Option<(WlSurface, Point<f64, Logical>)>,
    /// How many pointer buttons are currently held - what actually decides
    /// when [`Self::pointer_button_grab`] starts (0 -> 1) and ends (only
    /// once every button, not just one of several held at once, comes back
    /// up), matching the real Wayland implicit-grab rule instead of the
    /// single-button assumption that would break as soon as a drag and a
    /// second accidental button overlapped.
    pub(crate) pointer_buttons_held: u32,
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
pub(crate) fn effective_border_color(configured: (u8, u8, u8), focused: bool, dim: f32) -> (u8, u8, u8) {
    if focused {
        return configured;
    }
    let scale = |c: u8| (c as f32 * dim).round().clamp(0.0, 255.0) as u8;
    (scale(configured.0), scale(configured.1), scale(configured.2))
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


mod desktop_icons;
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
