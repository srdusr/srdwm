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
    AbsolutePositionEvent, Axis, ButtonState as BackendButtonState, Event as InputEventTrait, GestureBeginEvent as BackendGestureBeginEvent,
    GestureEndEvent as BackendGestureEndEvent, GesturePinchUpdateEvent as BackendGesturePinchUpdateEvent, InputEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
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
use smithay::input::pointer::{
    AxisFrame, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent,
};
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
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform, SERIAL_COUNTER};
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
use crate::input::{
    handle_gesture_swipe_begin, handle_gesture_swipe_end, handle_gesture_swipe_update, handle_keyboard_key_event,
    handle_pointer_button, handle_pointer_position,
};
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

/// How long a secondary-cursor entry (`UdevState::secondary_cursors`) is
/// trusted after its own device's last real motion event before it's
/// treated as stale and pruned/skipped - see that field's own doc
/// comment for the frozen-ghost-cursor bug this exists to close. Short
/// enough that a genuinely idle second device's sprite actually
/// disappears at a human-noticeable timescale (not "eventually, whenever
/// something else happens to touch this map"), generous enough that
/// briefly pausing mid-gesture with a real second device doesn't flicker
/// its own cursor away and back.
pub(crate) const SECONDARY_CURSOR_TIMEOUT: Duration = Duration::from_millis(1500);

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
    /// When the current `flip_pending` was set - lets `render_udev_frame`
    /// notice a page-flip event that never arrived (or arrived but matched
    /// no head - see `FLIP_TIMEOUT`'s own doc comment) instead of waiting
    /// on it forever. Meaningless while `flip_pending` is `false`.
    pub(crate) flip_pending_since: Instant,
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
    /// The DRM mode this head was actually brought up with - kept so a VT-
    /// switch resume can reassert the CRTC with its real connector and
    /// mode (see `register_session_notifier`'s own `ActivateSession` arm),
    /// rather than the empty connector list and `None` mode that call used
    /// to pass, which does not reassert a CRTC at all - it is DRM/KMS's
    /// own shape for *disabling* one. Confirmed live: switching back to
    /// srdwm's VT after switching away left the screen black, with no
    /// further VT switch (either direction) able to recover it, matching a
    /// CRTC left disabled rather than restored.
    pub(crate) mode: DrmMode,
    /// Set when [`UdevHead::copy_and_flip`] fails; no new flip is attempted
    /// for this head again until this deadline passes.
    ///
    /// Without this, a failed `page_flip` (real and reproduced live: the
    /// kernel returns `EBUSY`/"device or resource busy" for a brief window
    /// right after a VT-switch resume's `set_crtc` reasserts the mode,
    /// before that commit has actually settled) left `flip_pending` still
    /// `false` - `copy_and_flip`'s early-return `?` on the failing
    /// `page_flip` call skips the line just after it that would have set
    /// `flip_pending = true`, so nothing ever marked this head "busy". The
    /// next call to `render_udev_frame` (every ~16ms, or sooner --
    /// `event_loop.dispatch`'s timeout is only an upper bound) saw the
    /// exact same head still "ready" and every prior damage still pending,
    /// tried the exact same flip again, failed the exact same way, forever
    /// - a true busy loop with no backoff at all, not merely a missed
    /// optimization. Confirmed live from a real session log: tens of
    /// thousands of consecutive `page flip failed: Device or resource
    /// busy` lines a few *microseconds* apart, the compositor's one thread
    /// spinning flat out on nothing else, which is what actually explains
    /// the user's report of losing pointer input and the ability to
    /// switch VTs at all after switching away and back once - not a
    /// separate input bug, this loop simply never yielded the CPU back to
    /// anything else, libinput's own event processing included.
    pub(crate) flip_retry_after: Option<Instant>,
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
    /// Fully virtual "fake" monitors - no DRM connector/CRTC, never
    /// scanned out. See `virtual_heads.rs`'s own module doc comment for
    /// the full design and scope.
    pub(crate) virtual_heads: Vec<VirtualHead>,
    pub(crate) active: bool,
    /// Pointer position in the *global* space, so it can cross between
    /// monitors; clamped to the union of all head rectangles.
    pub(crate) pointer_pos: Point<f64, Logical>,
    /// Multi-cursor mode, Phase 1: every physical pointer/trackpad's own
    /// last-known position *and when it was last actually recorded*, keyed
    /// by its real libinput device identity (`smithay::backend::input::
    /// Event::device()`, confirmed `Device: PartialEq + Eq + Hash` by
    /// reading smithay's own trait definition). Purely a *visual*
    /// addition - `pointer_pos` above is still the one position that
    /// actually drives clicks/drags/hit-testing, updated by whichever
    /// device moved most recently exactly as before, so nothing about
    /// existing interactive behaviour changes.
    ///
    /// Gated behind `WindowManager::multi_cursor_enabled` (off by
    /// default) and the timestamp both exist for the same real, reported
    /// bug: real hardware routinely reports what is genuinely one mouse
    /// as more than one distinct libinput device (a side-button/scroll
    /// cluster on its own HID path, concretely) - the first motion event
    /// from that phantom device seeded a permanent entry here, rendered
    /// every frame forever after at wherever the pointer happened to be
    /// at that one moment, since nothing ever moved that specific device
    /// identity again. Reported live as "I see two cursors and can't even
    /// control the other one" - a frozen, uncontrollable ghost, exactly
    /// what an unpruned entry here looks like. `render_udev_frame` now
    /// skips (and `handle_libinput_event` now prunes) any entry older
    /// than `SECONDARY_CURSOR_TIMEOUT`, so only a device that has *itself*
    /// moved recently ever shows a sprite.
    pub(crate) secondary_cursors: HashMap<smithay::reexports::input::Device, (Point<f64, Logical>, Instant)>,
    /// A clone of the same `LibSeatSession` `platform.rs` opened the DRM
    /// device with (`LibSeatSession` is cheaply `Clone` - see its own
    /// derive - all clones share the same underlying seat connection).
    /// Kept here, reachable from `input.rs`'s keyboard handler, purely so
    /// `Ctrl+Alt+F<n>` can call `change_vt` on it - nothing else in this
    /// backend needed the session handle after startup, so it was never
    /// retained anywhere before this.
    pub(crate) session: LibSeatSession,
    /// Connector names administratively disabled via `srd dispatch set
    /// output enabled <name> false` - still physically connected (DRM
    /// still reports/probes them), just deliberately not driven. Checked
    /// by `reprobe_outputs`'s own "added" loop so an unrelated hotplug
    /// event doesn't resurrect one of these the next time anything else
    /// plugs or unplugs - without this, the very next `Changed` uevent
    /// (any connector, not just this one) would see a disabled-but-still-
    /// present connector as newly "added" (present in a fresh probe,
    /// absent from `heads`, exactly the condition that branch already
    /// uses to detect a real hotplug) and bring it straight back up.
    pub(crate) disabled_connectors: std::collections::HashSet<String>,
    /// `WorkspaceId` this backend last built `custom_elements` for --
    /// compared against `WindowManager::current_workspace()` at the top of
    /// every `render_udev_frame` call so a switch can force every head's
    /// `ages` back to `[0, 0]` (see that call site's own comment for why).
    /// `None` before the very first frame, which already renders fully
    /// regardless (every head starts with `ages: [0, 0]` - see
    /// `UdevHead`'s own field).
    pub(crate) last_rendered_workspace: Option<srdwm_core::WorkspaceId>,
    /// Order-sensitive hash of every visible window's id and rect, compared
    /// each frame in `render_udev_frame` to force `ages` back to `[0, 0]`
    /// on any move/resize/open/close/restack - see that comparison's own
    /// doc comment for the live-reproduced ghost-content bug this catches.
    /// `None` before the first frame, same reasoning as `last_rendered_
    /// workspace` above (renders fully regardless).
    pub(crate) last_rendered_layout: Option<u64>,
    /// Index into `heads` of whichever head the pointer was actually drawn
    /// on last frame (`None` before the first frame, or if it was on none
    /// of them). Compared each frame in `render_udev_frame`, same pattern
    /// as `last_rendered_workspace`/`last_rendered_layout` above, so that
    /// when the pointer crosses from one monitor to another the head it
    /// just *left* gets its own `ages` forced back to `[0, 0]` too.
    ///
    /// Needed because neither of those two other resets notices this
    /// transition at all: no window moved, and the workspace didn't
    /// change, so both stay silent while the cursor sprite simply drops
    /// out of that head's `custom_elements` list from one frame to the
    /// next. That leaves the departing head's vacated cursor-sized region
    /// resting entirely on `OutputDamageTracker`'s own element diffing --
    /// already documented, for the same "an element disappeared" shape of
    /// bug on a window vacating part of the screen, as not reliable on its
    /// own (see `layout_signature`'s own doc comment above `render_udev_
    /// frame`). Reported live as an intermittent cursor "ghost" briefly
    /// left behind right after moving the pointer between monitors --
    /// intermittent because it depends on whatever else that head's own
    /// diffing already had queued that frame, exactly like the window
    /// case did.
    pub(crate) last_cursor_head: Option<usize>,
    /// Set only when `SRDWM_GPU=1` and `gpu::probe` succeeds on this
    /// hardware - see that function's own doc comment for exactly what
    /// it does and does not do yet. `None` (the default, every session
    /// that doesn't set the env var, and every one where the probe fails)
    /// means every head renders through `renderer`/`PixmanRenderer` above,
    /// completely unaffected by this field's existence.
    pub(crate) gpu: Option<gpu::GpuContext>,
}

impl UdevState {
    /// Bounding box of every head, used to clamp pointer motion --
    /// `(min_x, min_y, max_x, max_y)`, not just a `(width, height)`
    /// implicitly anchored at `(0, 0)` (what this used to return, and what
    /// every call site clamped into with a hardcoded `0.0` floor). That
    /// was only ever correct while every head's `location.x`/`location.y`
    /// stayed `>= 0`, true for `reprobe_outputs`' own left-to-right hotplug
    /// layout but not guaranteed once `set_output_position` exists: an
    /// "extend left"/"extend above" arrangement (a real one, requested and
    /// applied live by an AGS peer session's monitor-layout panel) places
    /// the newly-added head at a *negative* `x`/`y` relative to whichever
    /// one stayed at the origin. With the old `(0, w)` clamp, the pointer
    /// could never actually cross into that negative-origin region at
    /// all - reported live as "clicked it now I can't go to other
    /// monitor at all" once such an arrangement was applied. The AGS
    /// side has since started normalising every arrangement it sends so
    /// the leftmost/topmost edge lands at `0` again, which works around
    /// this from outside, but srdwm's own pointer clamp assuming an origin
    /// no other part of this backend actually enforces is the real bug --
    /// fixed here instead of just left for every future caller to avoid.
    pub(crate) fn bounds(&self) -> (f64, f64, f64, f64) {
        bounds_of(self.heads.iter().map(|h| (h.location.x, h.location.y, h.size.0, h.size.1)))
    }
}

/// The actual arithmetic behind [`UdevState::bounds`], over plain
/// `(x, y, width, height)` tuples rather than real `UdevHead`s - pulled
/// out so it's testable without a real DRM/`Card` handle, which every
/// `UdevHead` in this module otherwise needs to even construct.
pub(crate) fn bounds_of(heads: impl Iterator<Item = (i32, i32, i32, i32)>) -> (f64, f64, f64, f64) {
    let mut min_x = 0;
    let mut min_y = 0;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut any = false;
    for (x, y, w, h) in heads {
        if !any {
            min_x = x;
            min_y = y;
            any = true;
        } else {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
        }
        max_x = max_x.max(x + w);
        max_y = max_y.max(y + h);
    }
    (min_x as f64, min_y as f64, max_x as f64, max_y as f64)
}

#[cfg(test)]
mod bounds_tests {
    use super::bounds_of;

    #[test]
    fn single_head_at_origin_matches_the_old_zero_anchored_behaviour() {
        assert_eq!(bounds_of([(0, 0, 1920, 1080)].into_iter()), (0.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    fn two_heads_left_to_right_from_origin() {
        assert_eq!(bounds_of([(0, 0, 1920, 1080), (1920, 0, 1920, 1080)].into_iter()), (0.0, 0.0, 3840.0, 1080.0));
    }

    #[test]
    fn negative_origin_head_is_reflected_in_min_not_clamped_to_zero() {
        // The actual regression this exists for: an "extend left"
        // arrangement places the new head at a negative x, and the old
        // `(width, height)`-only version of this function (implicitly
        // anchored at 0) made that head's own region completely
        // unreachable by pointer motion - reported live as "clicked it
        // now I can't go to other monitor at all".
        let (min_x, min_y, max_x, max_y) = bounds_of([(0, 0, 1920, 1080), (-1920, 0, 1920, 1080)].into_iter());
        assert_eq!((min_x, min_y, max_x, max_y), (-1920.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    fn negative_origin_above_is_reflected_in_min_y() {
        let (min_x, min_y, max_x, max_y) = bounds_of([(0, 0, 1920, 1080), (0, -1080, 1920, 1080)].into_iter());
        assert_eq!((min_x, min_y, max_x, max_y), (0.0, -1080.0, 1920.0, 1080.0));
    }

    #[test]
    fn no_heads_at_all_is_a_degenerate_zero_sized_box_not_a_panic() {
        assert_eq!(bounds_of(std::iter::empty()), (0.0, 0.0, 0.0, 0.0));
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
    /// `damage` is the exact set of rects `render_output` just re-rendered
    /// into `self.buffers[back].image` - an empty slice means "copy
    /// everything" (the locked/lock-UI render paths don't bother computing
    /// per-rect damage, so this is also the safe fallback for any caller
    /// that can't cheaply produce real rects), otherwise only those rows'
    /// column ranges are copied.
    ///
    /// Used to be an unconditional full-buffer copy regardless of how
    /// little of the frame actually changed - `render_output`'s own
    /// age-based damage tracking already leaves everything outside
    /// `damage` untouched in `image` (correct: that buffer's untouched
    /// pixels still match what was on screen `ages[back]` frames ago), so
    /// `dumb` - this same buffer's DRM-mapped twin, previously brought up
    /// to date by this exact function on that same past frame - is
    /// already correct everywhere outside `damage` too. Copying the whole
    /// buffer anyway meant a full `stride * height` memcpy on every single
    /// presented frame, for content as small as a moved cursor or a
    /// blinking terminal caret - confirmed as the largest per-frame CPU
    /// cost on this software `PixmanRenderer` backend by a direct
    /// comparison against niri's DRM-composited present path (which has no
    /// equivalent copy step at all) and mutter's native backend (which
    /// explicitly restricts its own swap to damaged regions,
    /// `swap_buffers_with_damage`) - this is the same technique, adapted
    /// to a raw byte copy instead of a GL/EGL damage extension.
    fn copy_and_flip(&mut self, card: &Card, back: usize, damage: &[Rectangle<i32, Physical>]) -> std::io::Result<()> {
        let (src_stride, height, width) =
            (self.buffers[back].image.stride(), self.buffers[back].image.height(), self.buffers[back].image.width());
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
            copy_damaged_rows(src, mapping.as_mut(), src_stride, dst_stride, width, height, damage);
        }
        card.page_flip(self.crtc, self.buffers[back].fb, PageFlipFlags::EVENT, None)?;
        self.flip_pending = true;
        self.flip_pending_since = Instant::now();
        Ok(())
    }
}

/// The row/column copy math behind [`DrmHead::copy_and_flip`], pulled out
/// as a free function over plain slices so it's testable without a real
/// `Card`/dumb buffer - everything else in that method needs live DRM
/// state, this doesn't. `damage` empty means "copy every row in full"
/// (`width`/`height` are pixels, `src_stride`/`dst_stride` bytes); a
/// non-empty `damage` copies only each rect's row/column span, clamped to
/// the narrower of the two strides and to `width`/`height` the same way
/// the full-copy path always has.
fn copy_damaged_rows(src: &[u8], dst: &mut [u8], src_stride: usize, dst_stride: usize, width: usize, height: usize, damage: &[Rectangle<i32, Physical>]) {
    let full_row_len = src_stride.min(dst_stride);
    let copy_row = |dst: &mut [u8], row: usize, col_start_bytes: usize, col_len: usize| {
        let s = row * src_stride + col_start_bytes;
        let d = row * dst_stride + col_start_bytes;
        let len = col_len.min(full_row_len.saturating_sub(col_start_bytes));
        if len == 0 || s + len > src.len() || d + len > dst.len() {
            return;
        }
        dst[d..d + len].copy_from_slice(&src[s..s + len]);
    };
    if damage.is_empty() {
        for row in 0..height {
            copy_row(dst, row, 0, full_row_len);
        }
        return;
    }
    const BPP: usize = 4; // Argb8888/Xrgb8888, same assumption every other raw-buffer path in this codebase makes.
    for rect in damage {
        let y0 = rect.loc.y.max(0) as usize;
        let y1 = (rect.loc.y.saturating_add(rect.size.h).max(0) as usize).min(height);
        let x0 = rect.loc.x.max(0) as usize;
        let x1 = (rect.loc.x.saturating_add(rect.size.w).max(0) as usize).min(width);
        if x1 <= x0 {
            continue;
        }
        let (col_start_bytes, col_len) = (x0 * BPP, (x1 - x0) * BPP);
        for row in y0..y1 {
            copy_row(dst, row, col_start_bytes, col_len);
        }
    }
}

#[cfg(test)]
mod copy_damaged_rows_tests {
    use super::copy_damaged_rows;
    use smithay::utils::{Physical, Point, Rectangle, Size};

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    /// A tiny 4x3 BGRA canvas, one distinct byte value per pixel's blue
    /// channel (row * width + col) so a wrong offset or a skipped pixel
    /// shows up as the wrong number, not just "still zero".
    fn make_src(width: usize, height: usize) -> Vec<u8> {
        let mut buf = vec![0u8; width * height * 4];
        for (i, px) in buf.chunks_exact_mut(4).enumerate() {
            px[0] = i as u8;
            px[3] = 255;
        }
        buf
    }

    #[test]
    fn empty_damage_copies_every_row_in_full() {
        let (w, h) = (4, 3);
        let src = make_src(w, h);
        let mut dst = vec![0u8; w * h * 4];
        copy_damaged_rows(&src, &mut dst, w * 4, w * 4, w, h, &[]);
        assert_eq!(dst, src);
    }

    #[test]
    fn a_damage_rect_updates_only_its_own_pixels() {
        let (w, h) = (4, 3);
        let src = make_src(w, h);
        let mut dst = vec![0u8; w * h * 4];
        // Only the single pixel at (1, 1).
        copy_damaged_rows(&src, &mut dst, w * 4, w * 4, w, h, &[rect(1, 1, 1, 1)]);
        let idx = (w + 1) * 4;
        assert_eq!(dst[idx], src[idx], "the damaged pixel must be copied");
        assert_eq!(dst[0], 0, "a pixel outside the damage rect must stay untouched");
        assert_eq!(dst[dst.len() - 4], 0, "the last row's pixel is also outside the rect and must stay untouched");
    }

    #[test]
    fn a_full_width_row_rect_copies_that_row_only() {
        let (w, h) = (4, 3);
        let src = make_src(w, h);
        let mut dst = vec![0u8; w * h * 4];
        copy_damaged_rows(&src, &mut dst, w * 4, w * 4, w, h, &[rect(0, 1, w as i32, 1)]);
        let row1 = w * 4..w * 4 * 2;
        assert_eq!(dst[row1.clone()], src[row1], "row 1 must be fully copied");
        assert_eq!(&dst[..w * 4], &vec![0u8; w * 4][..], "row 0 must stay untouched");
        assert_eq!(&dst[w * 4 * 2..], &vec![0u8; w * 4][..], "row 2 must stay untouched");
    }

    #[test]
    fn a_rect_extending_past_the_buffer_is_clamped_not_panicking() {
        let (w, h) = (4, 3);
        let src = make_src(w, h);
        let mut dst = vec![0u8; w * h * 4];
        // Starts inside the buffer but both extends past its right/bottom
        // edge and would run off a naive unclamped copy.
        copy_damaged_rows(&src, &mut dst, w * 4, w * 4, w, h, &[rect(2, 2, 100, 100)]);
        let idx = (2 * w + 2) * 4;
        assert_eq!(dst[idx], src[idx], "the in-bounds corner of an oversized rect must still be copied");
    }

    #[test]
    fn a_wider_destination_stride_does_not_shear_rows() {
        // Destination row padded 4 extra bytes past the source's own
        // stride - the same "driver-padded dumb buffer pitch" case the
        // full-copy path was already written to handle; damage-restricted
        // copying must preserve that, not just the empty-damage fallback.
        let (w, h) = (4, 3);
        let src = make_src(w, h);
        let dst_stride = w * 4 + 4;
        let mut dst = vec![0u8; dst_stride * h];
        copy_damaged_rows(&src, &mut dst, w * 4, dst_stride, w, h, &[rect(0, 0, w as i32, h as i32)]);
        for row in 0..h {
            let s = row * w * 4..row * w * 4 + w * 4;
            let d = row * dst_stride..row * dst_stride + w * 4;
            assert_eq!(dst[d], src[s], "row {row} must land at the destination's own stride, not the source's");
        }
    }
}

mod capture;
mod drm;
pub(crate) mod gpu;
mod outputs;
mod platform;
mod render;
mod session;
mod virtual_heads;

pub use platform::UdevPlatform;
pub(crate) use virtual_heads::VirtualHead;
