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
use smithay::desktop::{layer_map_for_output, Space, Window as DWindow, WindowSurfaceType};
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
use smithay::wayland::shm::ShmState;

use srdwm_core::{Event as CoreEvent, Window as CoreWindow, WindowId, WindowManager, TITLEBAR_HEIGHT};

use crate::lock::SessionLock;
use crate::{decoration, screencopy, udev, xwayland};

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
    pub(crate) seat_state: SeatState<CompState>,
    pub(crate) seat: Seat<CompState>,
    pub(crate) space: Space<DWindow>,
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
    pub(crate) _screencopy_state: screencopy::ScreencopyState,
    /// Captures requested via `wlr-screencopy` but not yet serviced; drained
    /// inside the render pass (see `screencopy::service_pending`).
    pub(crate) screencopy_pending: Vec<screencopy::PendingCapture>,
    /// Session lock (`ext-session-lock-v1`). While `locked` is set, client
    /// content is never rendered and input never reaches normal clients --
    /// see `SessionLockHandler` below.
    pub(crate) lock: SessionLock,
    /// What the pointer should look like, as set by the focused client (or
    /// the default when no client has said). See `cursor.rs` for why this
    /// has to be drawn by us on the DRM backend.
    pub(crate) cursor_status: smithay::input::pointer::CursorImageStatus,
    /// Bitmap for the built-in arrow, built once at startup rather than
    /// per frame.
    pub(crate) cursor_buffer: MemoryRenderBuffer,
    /// Last titlebar press, for double-click detection.
    pub(crate) last_titlebar_click: Option<(WindowId, u32)>,
    pub(crate) wm: Rc<RefCell<WindowManager>>,
    pub(crate) surface_to_id: HashMap<WlSurface, WindowId>,
    pub(crate) id_to_window: HashMap<WindowId, DWindow>,
    pub(crate) decorations: HashMap<WindowId, MemoryRenderBuffer>,
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
}

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

/// Matches the seat's own repeat settings (`add_keyboard(.., 200, 25)`), so
/// held bindings feel the same as held keys in a text field.
const REPEAT_DELAY: Duration = Duration::from_millis(200);
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

/// Titlebar background is the same regardless of focus (matching the X11
/// backend); only the title text color changes.
const TITLEBAR_BG: (u8, u8, u8) = (0x2e, 0x34, 0x40);
const TITLEBAR_FG_FOCUSED: (u8, u8, u8) = (0x88, 0xc0, 0xd0);
const TITLEBAR_FG_UNFOCUSED: (u8, u8, u8) = (0x4c, 0x56, 0x6a);

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
        self.surface_to_id.insert(surface.clone(), id);
        self.id_to_window.insert(id, dwindow);
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
    }

    /// (Re)renders the titlebar band for `id` - background plus title text
    /// via `decoration::render_titlebar` - and replaces the buffer in
    /// `self.decorations`. Called on creation, geometry change (width
    /// affects layout), and focus change (text color).
    pub(crate) fn redraw_decoration_buffer(&mut self, id: WindowId) {
        let Some(w) = self.wm.borrow().window(id).cloned() else { return };
        if !w.decorated {
            self.decorations.remove(&id);
            return;
        }
        let focused = self.wm.borrow().focused_id() == Some(id);
        let fg = if focused { TITLEBAR_FG_FOCUSED } else { TITLEBAR_FG_UNFOCUSED };
        let width = w.geometry.width.max(1);
        let data = decoration::render_titlebar(width, TITLEBAR_HEIGHT, &w.title, TITLEBAR_BG, fg);
        let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (width as i32, TITLEBAR_HEIGHT as i32), 1, Transform::Normal, None);
        self.decorations.insert(id, buffer);
    }

    pub(crate) fn remove_window(&mut self, surface: &WlSurface) {
        let Some(id) = self.surface_to_id.remove(surface) else { return };
        if let Some(w) = self.id_to_window.remove(&id) {
            self.space.unmap_elem(&w);
        }
        self.decorations.remove(&id);
        self.wm.borrow_mut().remove_window(id);
        self.pending.borrow_mut().push(CoreEvent::WindowDestroyed(id));
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
        // A layer surface lives in exactly one output's `LayerMap` (whichever
        // one `new_layer_surface` mapped it into), so find that output rather
        // than assuming a single global one.
        let found = self.outputs().find_map(|output| {
            let layer = layer_map_for_output(output).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned();
            layer.map(|l| (output.clone(), l))
        });
        let Some((output, layer)) = found else { return };

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
        layer_map_for_output(&output).arrange();

        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|d| d.lock().unwrap().initial_configure_sent)
                .unwrap_or(false)
        });
        if !initial_configure_sent {
            layer.layer_surface().send_configure();
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
        if keyboard.current_focus() == surface {
            return;
        }
        let client = surface.as_ref().and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh.clone(), &self.seat.clone(), client.clone());
        set_primary_focus(&self.dh.clone(), &self.seat.clone(), client);
        let serial = SERIAL_COUNTER.next_serial();
        keyboard.set_focus(self, surface, serial);
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
        if self.decorations.contains_key(&id) {
            self.redraw_decoration_buffer(id);
        }
    }
}

pub(crate) fn with_toplevel_title(toplevel: &ToplevelSurface) -> Option<String> {
    smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|d| d.lock().unwrap().title.clone().unwrap_or_default())
    })
}
