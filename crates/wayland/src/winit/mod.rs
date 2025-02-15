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
use smithay::backend::renderer::element::solid::SolidColorBuffer;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::backend::winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend};
use smithay::reexports::winit::dpi::LogicalSize as WinitLogicalSize;
use smithay::reexports::winit::window::Window as WinitWindow;
use smithay::desktop::{layer_map_for_output, PopupManager, Space};
use smithay::wayland::shell::wlr_layer::Layer;
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


mod capture;
mod connect;
mod events;
mod nested_platform;
mod render;
mod run;
