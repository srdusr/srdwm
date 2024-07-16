//! DRM/udev backend: runs srdwm as the real compositor on a bare TTY (no
//! host session to nest under), unlike the `backend_winit`-based path in
//! `lib.rs`.
//!
//! Scope:
//! - Single primary GPU, but **every** connected connector on it: each
//!   becomes a [`UdevHead`] with its own scanout buffers, damage tracker
//!   and page-flip state, laid out left-to-right in the global coordinate
//!   space. Connectors are re-probed on hotplug (see `reprobe_outputs`);
//!   a second GPU is not supported.
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
use smithay::backend::renderer::{Bind, ImportDma};
use smithay::backend::session::{libseat::LibSeatSession, libseat::LibSeatSessionNotifier, Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::desktop::{layer_map_for_output, PopupManager, Space};
use smithay::backend::input::AxisSource;
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::input::pointer::AxisFrame;
use smithay::input::SeatState;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::{FdWrapper, Generic};
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode as CalloopMode, PostAction};
use smithay::reexports::drm::buffer::{Buffer as DrmBufferTrait, DrmFourcc};
use smithay::reexports::drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, Event as DrmEvent, Mode as DrmMode,
    ModeTypeFlags, PageFlipFlags,
};
use smithay::reexports::drm::Device as BasicDevice;
use smithay::reexports::input::Libinput;
use smithay::reexports::pixman::{FormatCode, Image};
use smithay::reexports::rustix;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, ListeningSocket};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::xdg_activation::XdgActivationState;

use srdwm_core::{Event as CoreEvent, WindowManager};
use srdwm_platform::{Platform, PlatformError, PlatformKind, Result as PlatformResult};

use crate::decoration;
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
    /// Which connector this head drives - the key hotplug diffs against.
    pub(crate) connector: connector::Handle,
    pub(crate) output: Output,
    /// The `wl_output` global, kept so it can be destroyed when the monitor
    /// is unplugged; leaving it advertised would show clients a screen that
    /// no longer exists.
    pub(crate) global: GlobalId,
    pub(crate) damage_tracker: OutputDamageTracker,
    pub(crate) buffers: [DrmBuffer; 2],
    pub(crate) front: usize,
    /// A flip is in flight; the next frame for this head waits for the DRM
    /// page-flip event (matched by `crtc`) before starting.
    pub(crate) flip_pending: bool,
    /// Per-buffer-slot age passed to `damage_tracker.render_output`: how
    /// many *damage-producing* renders ago that exact buffer was last
    /// brought fully up to date. 0 means "never rendered, contents
    /// undefined" and forces a full redraw. This used to be hardcoded to 0
    /// on every single call regardless - which, per
    /// `OutputDamageTracker::damage_output_internal`, forces the entire
    /// output geometry to be treated as damaged every time, so every frame
    /// was a full-screen software (pixman) recomposite plus a page-flip,
    /// nonstop, at whatever rate the event loop's 16ms dispatch timeout
    /// allowed - continuously, even with a fully idle desktop. That
    /// competes for the same single thread's CPU time as libinput event
    /// processing and is exactly what `client bug: event processing
    /// lagging behind` (logged for both the keyboard and the mouse) was
    /// reporting. With correct ages, a call that finds no real damage
    /// returns near-free (`damage_output_internal`'s own element/geometry
    /// comparison, no pixel work) and skips the flip entirely instead of
    /// always finding "damage".
    pub(crate) ages: [usize; 2],
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


impl UdevHead {
    /// Frees the DRM resources this head owns. Dropping the Rust structs
    /// alone would leak the kernel-side framebuffers and dumb buffers,
    /// which matters when a monitor is plugged and unplugged repeatedly.
    fn release(self, card: &Card) {
        for buffer in self.buffers {
            if let Err(e) = card.destroy_framebuffer(buffer.fb) {
                log::warn!("udev: destroy_framebuffer failed: {e}");
            }
            if let Err(e) = card.destroy_dumb_buffer(buffer.dumb) {
                log::warn!("udev: destroy_dumb_buffer failed: {e}");
            }
        }
    }

    /// Copies the just-rendered pixman image into buffer `back`'s dumb
    /// buffer (software rendering writes into its own owned image, not the
    /// scanout memory directly, to avoid tying that image's lifetime to an
    /// mmap - see this module's docs) and flips to it.
    fn copy_and_flip(&mut self, card: &Card, back: usize) -> std::io::Result<()> {
        let (src_stride, height) = (self.buffers[back].image.stride(), self.buffers[back].image.height());
        let byte_len = src_stride * height;
        // SAFETY: `image` owns this memory and outlives the byte slice we
        // construct from it here; we only read, and only for the duration
        // of this call.
        let src: &[u8] = unsafe { std::slice::from_raw_parts(self.buffers[back].image.data() as *const u8, byte_len) };
        // The dumb buffer's pitch is whatever the kernel driver actually
        // allocated, which the DRM API does not guarantee equals pixman's
        // own `src_stride` (drivers are free to pad each row for
        // alignment). This used to be a single flat `copy_from_slice` sized
        // off `src_stride` alone; on any driver that pads, that copies each
        // source row into the wrong offset in the destination, shearing the
        // image diagonally by one row per `dst_stride - src_stride` bytes of
        // padding. Copying row by row, each clamped to the narrower of the
        // two strides, is correct regardless of whether the strides happen
        // to match.
        let dst_stride = self.buffers[back].dumb.pitch() as usize;
        {
            let mut mapping = card.map_dumb_buffer(&mut self.buffers[back].dumb)?;
            let dst = mapping.as_mut();
            let row_len = src_stride.min(dst_stride);
            for row in 0..height {
                let s = row * src_stride;
                let d = row * dst_stride;
                if s + row_len > src.len() || d + row_len > dst.len() {
                    break;
                }
                dst[d..d + row_len].copy_from_slice(&src[s..s + row_len]);
            }
        }
        card.page_flip(self.crtc, self.buffers[back].fb, PageFlipFlags::EVENT, None)?;
        self.flip_pending = true;
        Ok(())
    }
}

mod drm;
mod outputs;
mod platform;
mod render;
mod session;

pub use platform::UdevPlatform;
