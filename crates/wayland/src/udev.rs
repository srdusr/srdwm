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

impl CompState {
    /// Renders and (if there was damage) page-flips a new frame on every
    /// head that is ready for one. A head with a flip still in flight is
    /// skipped this pass and picked up when its page-flip event arrives, so
    /// monitors on different refresh rates each run at their own pace
    /// instead of the slowest one gating the rest.
    pub(crate) fn render_udev_frame(&mut self) {
        self.tick_animations();
        self.tick_dirty_broadcasts();
        let locked = self.lock.locked;
        let elapsed = self.start_time.elapsed();
        // Drained before the `&mut self.udev` borrow below, so screencopy can
        // be serviced with the renderer that borrow owns.
        let mut captures = std::mem::take(&mut self.screencopy_pending);
        // Same reason: the cursor needs the renderer that borrow owns.
        let cursor_status = self.cursor_status.clone();
        let cursor_buffers = self.cursor_buffers.clone();

        // Border geometry is in global space, independent of which head
        // renders it, so it's gathered once here rather than per head.
        // Buffers are pre-built for the same reason as `cursor_buffers`:
        // Rendered per window, front-to-back (topmost first), each window's
        // content immediately followed by its decoration and border --
        // fixes the same cross-window ordering bug documented in
        // `winit.rs`'s render loop: a background window's titlebar could
        // otherwise show through in front of the actually-focused window on
        // top of it, since decorations/borders used to be a single flat
        // layer drawn unconditionally above *every* window's content
        // regardless of real stacking order. `visible_windows_front_to_back`
        // is `WindowManager.order` reversed - not `visible_windows`, which
        // iterates the `windows` HashMap with no ordering guarantee - the
        // same "topmost first" convention `hit_test`/`window_at` use.
        // Fetched once here (`&mut self` fields, id_to_window/space lookups
        // happen per head below without needing `self` itself mutably) --
        // see the per-head loop for why decoration/border buffers still get
        // looked up fresh per head (head-local `origin` translation).
        let ids: Vec<srdwm_core::WindowId> = if locked { Vec::new() } else { self.wm.borrow().visible_windows_front_to_back().map(|w| w.id).collect() };
        let focused = self.wm.borrow().focused_id();
        let popup_targets = if locked { Vec::new() } else { crate::elements::popup_targets(self) };

        // Which heads are eligible, and what each needs, gathered before the
        // mutable borrow of `self.udev`. Both early-outs below give the
        // `captures` taken above nowhere to go this pass - put them back
        // rather than silently dropping a client's pending screenshot
        // because a VT switch happened to be in progress at that instant.
        let Some(udev) = self.udev.as_ref() else {
            self.screencopy_pending.extend(captures);
            return;
        };
        if !udev.active {
            self.screencopy_pending.extend(captures);
            return;
        }
        let ready: Vec<(usize, Output)> = udev
            .heads
            .iter()
            .enumerate()
            .filter(|(_, h)| !h.flip_pending)
            .map(|(i, h)| (i, h.output.clone()))
            .collect();
        // Kept separately from `presented` below: layer-shell surfaces
        // (bars, docks) get their frame callback every pass regardless of
        // `has_damage`, unlike toplevel windows - see the callback loop at
        // the end of this function for why the two can't share one gate.
        let ready_outputs: Vec<Output> = ready.iter().map(|(_, o)| o.clone()).collect();

        // Damage rects travel alongside each presented output so the
        // frame-callback loop below (after `udev` is no longer borrowed)
        // can notify only the windows that damage actually overlapped --
        // see `windows_touched_by_damage`'s doc comment in elements.rs.
        let mut presented: Vec<(Output, Vec<Rectangle<i32, Physical>>)> = Vec::new();
        for (index, output) in ready {
            let lock_surface = self.lock_surface_for(&output).cloned();

            // Content/decoration elements are built per head: both need the
            // renderer, and geometry is translated into head-local space.
            let origin = self.udev.as_ref().map(|u| u.heads[index].location).unwrap_or_default();

            let Some(udev) = self.udev.as_mut() else { return };
            let head = &mut udev.heads[index];
            let back = 1 - head.front;

            let mut custom_elements: Vec<crate::elements::OverlayElement<PixmanRenderer>> = Vec::new();
            if !locked {
                // Cursor first: `render_output` draws custom elements
                // front-to-back, so the earliest element is topmost. On a
                // bare TTY nothing else draws a pointer - see `cursor.rs`.
                let pointer_pos = udev.pointer_pos;
                let hsize = udev.heads[index].size;
                custom_elements.extend(crate::cursor::render_elements(
                    &cursor_status,
                    &cursor_buffers,
                    &mut udev.renderer,
                    pointer_pos,
                    origin,
                    hsize,
                ));
                // The right-click titlebar menu, if open - pushed right
                // after the cursor so it's still topmost over every window
                // but never hides the pointer itself (you need to see what
                // you're about to click).
                if let (Some(menu), Some(buffer)) = (self.context_menu.as_ref(), self.context_menu_buffer.as_ref()) {
                    let pos = ((menu.pos.0 - origin.x) as f64, (menu.pos.1 - origin.y) as f64);
                    match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                        Err(e) => log::warn!("udev: failed to import context menu buffer: {e}"),
                    }
                }
                // Popups next: always above every window's own content,
                // matching this codebase's long-standing behavior from
                // before content moved into this same `custom_elements`
                // list (see below) - pushing them here, ahead of every
                // window and every layer-shell surface, is what keeps that
                // true now that "above everything in `self.space`" is no
                // longer a free property of a separate tier.
                custom_elements.extend(crate::elements::popup_render_elements(&popup_targets, &mut udev.renderer, (origin.x, origin.y)));

                // The bar/dock/launcher (`Layer::Top`/`Overlay`): rendered
                // ourselves via `output_layer_elements`, not through
                // `render_output`'s automatic inclusion of `self.space` +
                // `layer_map_for_output` - see this function's own call
                // site further down for why content had to stop flowing
                // through that convenience wrapper at all (per-window
                // opacity), which took layer-shell inclusion down with it as
                // a side effect. Skipped entirely - not just covered - for
                // a fullscreen window: `we should not see the bar at all`,
                // and unmapping it (`gtk_shell`) or covering it are two
                // different guarantees. `ids` is already front-to-back, so
                // checking every id for `fullscreen` here (rather than just
                // the frontmost) covers a fullscreen window stacked behind
                // an always-on-top one too.
                let hide_top_layers = ids.iter().any(|&id| self.wm.borrow().window(id).is_some_and(|w| w.fullscreen));
                if !hide_top_layers {
                    custom_elements.extend(crate::elements::output_layer_elements(
                        &mut udev.renderer,
                        &output,
                        (origin.x, origin.y),
                        |layer| matches!(layer, Layer::Top | Layer::Overlay),
                    ));
                }

                // Windows stacked in front of whichever one border/
                // decoration is being built right now - `ids` is already
                // front-to-back, so this only ever needs appending to, not
                // recomputing. A window's own *content*, pushed inside this
                // same loop below, needs no separate occlusion test: it
                // draws in the same front-to-back push order as everything
                // else here, so ordinary painter's-algorithm draw order
                // already occludes it correctly (this is exactly why content
                // used to occlude correctly via `self.space`'s own order,
                // before it had to move into this list for per-window
                // opacity to be possible at all). The border strips and
                // titlebar bitmap are different: outside `geometry`, drawn
                // via a bitmap that isn't itself window-shaped, so they
                // still need `occluders`' explicit clip against whichever
                // window is stacked in front.
                let mut occluders: Vec<srdwm_core::Rect> = Vec::with_capacity(ids.len());
                for &id in &ids {
                    let Some(w) = self.wm.borrow().window(id).cloned() else { continue };
                    // `w.geometry` is the animation's *target*, not
                    // necessarily where the window is actually drawn this
                    // frame - during a maximize/fullscreen/open-slide tween,
                    // `sync_geometry` already renders the window's own
                    // content at `window_anims`' interpolated rect (see its
                    // doc comment), but this loop used to read `w.geometry`
                    // straight from the model regardless, so the border and
                    // titlebar sat at the final rect while the content they
                    // were supposed to outline slid past underneath them --
                    // reported live as the border "not flush" with the
                    // window during any animated transition. Every use of
                    // this window's geometry below (titlebar/border
                    // placement *and* the occlusion test against later
                    // windows) has to agree with what `sync_geometry` mapped
                    // the content to, or they drift apart again.
                    let geom = self.window_anims.get(&id).map(crate::state::WindowAnim::current_rect).unwrap_or(w.geometry);
                    // Drawn first among this window's own decoration, and
                    // positioned from the same animated `geom` as everything
                    // else here - not `w.geometry` - for the identical
                    // reason: a shadow that stayed at the pre-tween rect
                    // while the window slid past it would look exactly as
                    // detached as the border did before that fix. Not
                    // fragment-clipped against `occluders` like the titlebar/
                    // border below: at `SHADOW_MAX_ALPHA`'s low opacity, a
                    // shadow bleeding slightly onto a window stacked in front
                    // of this one reads as a soft edge, not the hard-line
                    // bleed-through that made the titlebar/border need it.
                    if let Some(shadow) = self.shadow_buffers.get(&id) {
                        let rect = decoration::shadow_rect(geom);
                        let pos = ((rect.x - origin.x) as f64, (rect.y - origin.y) as f64);
                        match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, shadow, None, None, None, Kind::Unspecified) {
                            Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                            Err(e) => log::warn!("udev: failed to import shadow buffer: {e}"),
                        }
                    }
                    if let Some(deco) = self.decorations.get(&id) {
                        // Fragment-clipped, same as the three solid border
                        // strips below - an *all-or-nothing* version of
                        // this (skip only once fully covered) was tried
                        // first and reported live as still showing "the
                        // behind window's bar": a titlebar only *partially*
                        // covered - the common case for cascaded/
                        // overlapping windows - drew in full regardless,
                        // bleeding through the covered part. `from_buffer`'s
                        // `src` parameter crops the bitmap itself, so each
                        // visible fragment can come from the matching
                        // sub-rect of the source image rather than the
                        // whole thing.
                        let titlebar_rect = srdwm_core::Rect::new(geom.x, geom.y, geom.width, srdwm_core::TITLEBAR_HEIGHT);
                        for fragment in crate::elements::visible_border_fragments(titlebar_rect, &occluders) {
                            let pos = ((fragment.x - origin.x) as f64, (fragment.y - origin.y) as f64);
                            let src = Rectangle::new(
                                Point::from(((fragment.x - titlebar_rect.x) as f64, (fragment.y - titlebar_rect.y) as f64)),
                                Size::from((fragment.width as f64, fragment.height as f64)),
                            );
                            match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, deco, None, Some(src), None, Kind::Unspecified) {
                                Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                Err(e) => log::warn!("udev: failed to import titlebar buffer: {e}"),
                            }
                        }
                    }
                    // Border strips sit entirely outside this window's own
                    // `geometry` (see `decoration::border_strips`), so they
                    // never overlap its own decoration/content - draw
                    // order against those doesn't matter here, only against
                    // other windows', which iterating `ids` in stacking
                    // order already gets right *for windows also drawn via
                    // this same custom_elements loop* - but not against
                    // any window's own *content*, which is why `occluders`
                    // below is still needed even with that ordering.
                    if w.border_width > 0 {
                        let color = crate::state::effective_border_color(w.border_color, focused == Some(id));
                        let strips = decoration::border_strips(geom, w.border_width);
                        // Strip 0 (top) rounded to match the titlebar under
                        // it - see `render_border_top`'s doc comment - so
                        // it's a cached bitmap (rebuilt only in
                        // `redraw_decoration_buffer`, same as the titlebar
                        // itself), not rasterized fresh here every frame.
                        // Not fragment-clipped like the other three strips
                        // below - cropping a bitmap's source rect per
                        // fragment is real extra work for a strip that's
                        // only `border_width` pixels tall to begin with, so
                        // this only handles the all-or-nothing case: skip
                        // entirely once *fully* covered, accept a small
                        // residual bleed while only partially covered.
                        if strips[0].width > 0 && strips[0].height > 0 && !strips[0].subtract_all(&occluders).is_empty() {
                            if let Some(buffer) = self.border_top_decorations.get(&id) {
                                let pos = ((strips[0].x - origin.x) as f64, (strips[0].y - origin.y) as f64);
                                match MemoryRenderBufferRenderElement::from_buffer(&mut udev.renderer, pos, buffer, None, None, None, Kind::Unspecified) {
                                    Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                                    Err(e) => log::warn!("udev: failed to import top border buffer: {e}"),
                                }
                            }
                        }
                        // The other three strips are persistent
                        // `SolidColorBuffer`s updated in place, not rebuilt
                        // with a fresh `Id` every frame - see
                        // `elements::border_side_render_element`'s doc
                        // comment for why that distinction is load-bearing
                        // for damage tracking, not cosmetic. Each strip is
                        // further split into whatever fragments remain
                        // visible after subtracting `occluders`, since a
                        // whole unclipped strip is exactly the bug fixed
                        // here.
                        let pool = self.border_side_buffers.entry(id).or_default();
                        let mut buf_index = 0;
                        for strip in &strips[1..] {
                            if strip.width == 0 || strip.height == 0 {
                                continue;
                            }
                            for fragment in crate::elements::visible_border_fragments(*strip, &occluders) {
                                let buf = crate::elements::border_fragment_buffer(pool, buf_index);
                                buf_index += 1;
                                custom_elements.push(crate::elements::OverlayElement::Solid(crate::elements::border_side_render_element(buf, fragment, color, (origin.x, origin.y))));
                            }
                        }
                    }
                    // The window's own content, at its own `opacity` --
                    // this, not decoration, is the entire reason content
                    // moved into this loop at all (see the doc comment on
                    // the popup push above). Positioned the same way
                    // `sync_geometry` maps it into `self.space` (band added
                    // for a decorated window's titlebar reservation), so
                    // switching rendering paths doesn't also shift content
                    // relative to where clicks still land (hit-testing is
                    // untouched, still `w.geometry`/`self.space`-based).
                    if let Some(dwindow) = self.id_to_window.get(&id) {
                        if let Some(surface) = crate::elements::window_wl_surface(dwindow) {
                            let band = if w.decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
                            let pos = (geom.x - origin.x, geom.y + band - origin.y);
                            custom_elements.extend(crate::elements::surface_content_elements(&mut udev.renderer, &surface, pos, w.opacity));
                        }
                    }
                    occluders.push(geom);
                }
                // Background/bottom layer-shell (wallpaper engines) last --
                // bottommost, matching smithay's own `space_render_elements`
                // ordering, which this whole custom loop now replaces.
                custom_elements.extend(crate::elements::output_layer_elements(
                    &mut udev.renderer,
                    &output,
                    (origin.x, origin.y),
                    |layer| matches!(layer, Layer::Background | Layer::Bottom),
                ));
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
                    .map(|r| (r.damage.is_some(), Vec::new()))
                    .map_err(|e| e.to_string())
            } else {
                // Not `smithay::desktop::space::render_output`: that
                // convenience wrapper draws `self.space`'s window content at
                // one `alpha` for the whole frame and pulls every
                // layer-shell surface in unconditionally, neither of which
                // leaves room for per-window opacity or hiding the bar/dock
                // during fullscreen. `custom_elements` above already carries
                // everything that wrapper would have built - window
                // content (`surface_content_elements`, one call per window,
                // each with its own `w.opacity`) and layer-shell surfaces
                // (`output_layer_elements`, split Top/Overlay above content
                // and Background/Bottom below it) - assembled by hand in
                // the correct front-to-back order instead. `self.space`
                // itself is untouched and still authoritative for
                // hit-testing/stacking bookkeeping (`sync_geometry`'s
                // `map_element` calls); only the *render* path stopped
                // reading from it.
                head.damage_tracker
                    .render_output(&mut udev.renderer, &mut framebuffer, head.ages[back], &custom_elements, [0.05, 0.05, 0.08, 1.0])
                    .map(|r| (r.damage.is_some(), r.damage.cloned().unwrap_or_default()))
                // Both arms reduce to "was there damage" plus the damage
                // rects themselves; the two error types differ, so they are
                // flattened to a message here.
                .map_err(|e| e.to_string())
            };
            if !locked {
                // Only this head's own captures: `captures` holds requests
                // for every output, and each must be read back from the
                // framebuffer it was actually requested against, not
                // whichever head happens to render first in this loop (a
                // multi-monitor capture would otherwise silently read the
                // wrong screen). Whatever doesn't match `output` stays in
                // `captures` for a later head this same pass.
                let (mine, rest): (Vec<_>, Vec<_>) = captures.into_iter().partition(|c| c.output == output);
                captures = rest;
                crate::screencopy::service_pending(mine, &mut udev.renderer, &framebuffer);
            }
            drop(framebuffer);

            let (has_damage, damage_rects) = match result {
                Ok(v) => v,
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
                // This buffer is now fully up to date. It won't be rendered
                // into again until the *other* slot has also been presented
                // once (strict two-buffer alternation), so by then it will
                // be exactly 2 damage-producing renders stale - matching
                // `damage_tracker`'s own history, which only advances on
                // calls that actually found damage (see `ages`' doc
                // comment).
                head.ages[back] = 2;
                // Only a head that actually presented a new frame should
                // tell its windows they may render their next one - this
                // used to run unconditionally for every "ready" head (i.e.
                // every head not already mid-flip) on every single call to
                // this function, which is every ~16ms regardless of
                // activity. A client that renders on the standard
                // wait-for-frame-callback pattern (which is most of
                // them - confirmed live: wezterm-gui pinned at 140%+ CPU
                // sitting on a fully idle, unchanged terminal) had no
                // reason not to redraw at whatever rate this loop cycled,
                // forever, since it kept getting told a new frame was
                // wanted whether or not the screen had changed at all.
                presented.push((output, damage_rects));
            }
        }

        // Frame callbacks + lock confirmation, once the `udev` borrow is done.
        for (output, damage_rects) in presented {
            if locked {
                let surface = self.lock_surface_for(&output).cloned();
                crate::lock::send_lock_frame(surface.as_ref(), &output, elapsed);
                self.confirm_lock_if_presented(&output);
            } else {
                let out = output.clone();
                let scale = Scale::from(out.current_scale().fractional_scale());
                for w in crate::elements::windows_touched_by_damage(&self.space, &damage_rects, scale) {
                    w.send_frame(&out, elapsed, None, |_, _| Some(out.clone()));
                }
            }
        }
        // Deliberately unconditional - not folded into the `presented`
        // loop above, and not gated on any head having had damage this
        // tick at all. The whole point of `always_notify` is covering the
        // case where the output has *no* damage whatsoever (a fully idle
        // desktop, cursor not moving) but the focused/hovered window still
        // has a pending callback it needs answered to unblock an input-
        // driven redraw - GTK's frame-clock model (Firefox's Wayland
        // vsync source included) paces every repaint through that
        // callback, even the first one after being idle, with no "just
        // commit immediately" fallback. Nesting this inside the `presented`
        // loop (the first version of this fix) meant it only ever ran on a
        // tick that already had damage from something else happening --
        // i.e. never in the exact scenario it exists for. Reported live as
        // clicks in Firefox still doing nothing at all, not just
        // intermittently, after the first version of this fix.
        if !locked {
            let pointer_pos = self.udev.as_ref().map(|u| u.pointer_pos).unwrap_or_default();
            let wm = self.wm.borrow();
            let always_notify = [wm.focused_id(), wm.window_at(pointer_pos.x as i32, pointer_pos.y as i32)];
            drop(wm);
            let outputs: Vec<Output> = self.outputs.iter().map(|e| e.output.clone()).collect();
            for w in always_notify.into_iter().flatten().filter_map(|id| self.id_to_window.get(&id)) {
                for out in &outputs {
                    w.send_frame(out, elapsed, None, |_, _| Some(out.clone()));
                }
            }
        }
        // Layer-shell surfaces (bars, docks, launchers) get their frame
        // callback on every pass, unconditionally - NOT folded into the
        // `presented`/`has_damage` gate above.
        //
        // That gate exists because most toplevel clients redraw on the
        // standard wait-for-callback loop regardless of whether their own
        // content changed (confirmed live: wezterm-gui pinned at 140%+ CPU
        // on a fully idle terminal when it got a callback every ~16ms
        // whether or not the screen had changed). Gating toplevel callbacks
        // on real output damage fixed that.
        //
        // Applying the same gate to layer surfaces creates a real deadlock
        // instead: many (GTK4/AGS among them) drive their *entire* repaint
        // loop off frame callbacks with no independent timer fallback --
        // paint once, request a callback, wait. If nothing ELSE on the
        // desktop ever produces damage again (a static terminal, no other
        // animation), that callback never arrives, so the surface can never
        // draw its next frame, which means it can never produce damage,
        // which means it never gets a callback - permanently frozen after
        // exactly one frame. Confirmed live: AGS and waybar both hung this
        // way, one frame in, with `wl_surface.frame` requests that were
        // never answered (see docs/PANEL_SUPPORT_TODO.md).
        //
        // Splitting the gate is safe rather than reintroducing the wezterm
        // bug: there are at most a handful of layer surfaces on a real
        // desktop (a bar, maybe a dock/launcher), their content is cheap to
        // redraw even when done needlessly, and periodic UI chrome (a
        // clock, a resource graph) is exactly the case frame callbacks
        // exist to pace - unlike a full toplevel window, whose redraw cost
        // is what made the unconditional case expensive in the first place.
        if !locked {
            for output in &ready_outputs {
                for layer in layer_map_for_output(output).layers() {
                    layer.send_frame(output, elapsed, None, |_, _| Some(output.clone()));
                }
            }
        }
        if locked {
            crate::screencopy::fail_pending(captures);
        } else if !captures.is_empty() {
            // Left over because their target head wasn't in `ready` this
            // pass (e.g. mid-page-flip). Put back rather than dropped: this
            // function runs again on the next poll tick (or the page-flip
            // completion that made the head ready), so the capture gets
            // another chance instead of silently vanishing - which is what
            // made `grim` hang waiting on a `ready`/`failed` that would
            // otherwise never come (see docs/PANEL_SUPPORT_TODO.md, P1).
            self.screencopy_pending.extend(captures);
        }
    }

    /// Sets a connector's DPMS mode via the generic KMS "DPMS" property --
    /// there is no dedicated legacy-API call for this in `drm-rs`, only the
    /// same `get_properties`/`set_property` pair every other connector
    /// property goes through, so the property has to be found by name each
    /// time rather than through some `Dpms` -specific method. `None` if the
    /// `wl_output` doesn't resolve to a live head, or the connector has no
    /// "DPMS" property at all (rare on real hardware, but virtual/headless
    /// outputs may not expose one) - either way maps to `zwlr_output_power_v1`'s
    /// `failed` event, matching what the protocol asks for when the mode
    /// can't be honoured.
    pub(crate) fn set_output_power(&self, wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput, on: bool) -> Option<()> {
        // Raw KMS UAPI values for the "DPMS" connector property
        // (`DRM_MODE_DPMS_ON`/`_OFF` in `drm_sys`/the kernel's
        // `drm_mode.h`) - not worth a whole extra dependency on `drm-sys`
        // just for two constants that have been stable since DPMS was
        // added to the DRM UAPI.
        const DRM_MODE_DPMS_ON: u64 = 0;
        const DRM_MODE_DPMS_OFF: u64 = 3;

        let target = self.output_for_wl(wl_output)?.output.clone();
        let udev = self.udev.as_ref()?;
        let head = udev.heads.iter().find(|h| h.output == target)?;
        let props = udev.card.get_properties(head.connector).ok()?;
        let dpms_prop = props.as_props_and_values().0.iter().copied().find(|&handle| udev.card.get_property(handle).is_ok_and(|info| info.name().to_str() == Ok("DPMS")))?;
        let mode = if on { DRM_MODE_DPMS_ON } else { DRM_MODE_DPMS_OFF };
        udev.card.set_property(head.connector, dpms_prop, mode).ok()
    }

    /// The CRTC's gamma ramp length, in elements per channel - what
    /// `zwlr_gamma_control_v1.gamma_size` reports so a client knows how
    /// large a table `set_gamma` expects. `None` if the output doesn't
    /// resolve to a live head, or the CRTC reports a zero-length ramp
    /// (no gamma hardware, common on virtual/headless outputs).
    pub(crate) fn gamma_ramp_size(&self, wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput) -> Option<u32> {
        let target = self.output_for_wl(wl_output)?.output.clone();
        let udev = self.udev.as_ref()?;
        let head = udev.heads.iter().find(|h| h.output == target)?;
        let len = udev.card.get_crtc(head.crtc).ok()?.gamma_length();
        (len > 0).then_some(len)
    }

    /// Reads a client-supplied gamma table (`zwlr_gamma_control_v1.
    /// set_gamma`'s `fd`: a memory-mapped blob of `gamma_size` `u16`s per
    /// channel, red then green then blue, per the protocol) and applies it
    /// to the CRTC. `None` on any failure - output/head not found, the
    /// blob is the wrong size, or the DRM `set_gamma` call itself fails --
    /// which the caller maps to `zwlr_gamma_control_v1.failed`, exactly
    /// what the protocol specifies for "setting the gamma tables failed".
    pub(crate) fn set_gamma_ramp(&self, wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput, fd: std::os::fd::OwnedFd) -> Option<()> {
        let target = self.output_for_wl(wl_output)?.output.clone();
        let udev = self.udev.as_ref()?;
        let head = udev.heads.iter().find(|h| h.output == target)?;
        let size = udev.card.get_crtc(head.crtc).ok()?.gamma_length() as usize;
        if size == 0 {
            return None;
        }
        // Three channels, two bytes (one native-endian u16) per element --
        // client and compositor are always the same machine, so there is
        // no cross-endianness concern to handle here, unlike an over-the-
        // wire protocol value.
        let expected_bytes = size * 3 * 2;
        // SAFETY: the fd is a client-supplied shared-memory blob, mapped
        // read-only for the duration of this call and never touched again
        // afterwards - the same trust boundary `wl_shm` buffers already
        // cross for every window's actual pixel content elsewhere in this
        // codebase.
        let map = unsafe { memmap2::MmapOptions::new().map(&fd) }.ok()?;
        if map.len() < expected_bytes {
            return None;
        }
        let read_channel = |offset: usize| -> Vec<u16> { map[offset..offset + size * 2].chunks_exact(2).map(|b| u16::from_ne_bytes([b[0], b[1]])).collect() };
        let red = read_channel(0);
        let green = read_channel(size * 2);
        let blue = read_channel(size * 4);
        udev.card.set_gamma(head.crtc, &red, &green, &blue).ok()
    }
}

impl CompState {
    /// Re-probes connectors after a hotplug and reconciles the head list.
    ///
    /// Connectors that vanished have their head torn down (global removed,
    /// output unmapped, DRM buffers freed); newly connected ones are brought
    /// up exactly as they would have been at startup. Every head is then
    /// repositioned left-to-right, because removing a monitor shifts the
    /// ones after it.
    pub(crate) fn reprobe_outputs(&mut self) {
        let Some(udev) = self.udev.as_ref() else { return };
        let card = udev.card.clone();

        let probes = match probe_connected(&card) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("udev: hotplug re-probe failed: {e}");
                return;
            }
        };
        let present: Vec<connector::Handle> = probes.iter().map(|p| p.connector).collect();
        let existing: Vec<connector::Handle> = udev.heads.iter().map(|h| h.connector).collect();

        let gone: Vec<connector::Handle> = existing.iter().copied().filter(|c| !present.contains(c)).collect();
        let added: Vec<usize> = probes
            .iter()
            .enumerate()
            .filter(|(_, p)| !existing.contains(&p.connector))
            .map(|(i, _)| i)
            .collect();
        if gone.is_empty() && added.is_empty() {
            return; // a "changed" event that didn't change the connector set
        }
        log::info!("udev: hotplug - {} output(s) removed, {} added", gone.len(), added.len());

        // ---- removals ----
        for connector in &gone {
            let Some(udev) = self.udev.as_mut() else { return };
            let Some(index) = udev.heads.iter().position(|h| h.connector == *connector) else { continue };
            let head = udev.heads.remove(index);
            log::info!("udev: output {} disconnected", head.output.name());
            self.dh.remove_global::<CompState>(head.global.clone());
            self.space.unmap_output(&head.output);
            self.outputs.retain(|e| e.output != head.output);
            // A lock surface for a monitor that no longer exists would keep
            // `confirm_lock_if_presented` waiting forever otherwise.
            self.lock.surfaces.remove(&head.output.name());
            self.lock.presented.remove(&head.output.name());
            head.release(&card);
            self.pending.borrow_mut().push(CoreEvent::MonitorRemoved(index as u32));
        }

        // ---- additions ----
        for i in added {
            let probe = &probes[i];
            let used: Vec<crtc::Handle> =
                self.udev.as_ref().map(|u| u.heads.iter().map(|h| h.crtc).collect()).unwrap_or_default();
            let Some(crtc) = pick_crtc(&card, probe, &used) else {
                log::warn!("udev: no free CRTC for newly connected {}; not driving it", probe.name);
                continue;
            };
            // Placed at 0 for now; the re-layout below assigns real offsets.
            match bring_up_head(&card, &self.dh.clone(), probe, crtc, 0) {
                Ok((head, entry)) => {
                    log::info!("udev: output {} connected ({}x{})", probe.name, head.size.0, head.size.1);
                    let monitor_id = self.outputs.len() as u32;
                    let geometry = srdwm_core::Rect::new(0, 0, head.size.0 as u32, head.size.1 as u32);
                    if let Some(udev) = self.udev.as_mut() {
                        udev.heads.push(head);
                    }
                    self.outputs.push(entry);
                    self.pending
                        .borrow_mut()
                        .push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(monitor_id, probe.name.clone(), geometry)));
                }
                Err(e) => log::warn!("udev: failed to bring up {}: {e}", probe.name),
            }
        }

        self.relayout_outputs();
    }

    /// Repositions every head left-to-right and republishes the new
    /// positions to the output globals, the `Space`, and the layer maps.
    fn relayout_outputs(&mut self) {
        let Some(udev) = self.udev.as_mut() else { return };
        let mut x = 0;
        let mut placed: Vec<(Output, Point<i32, Logical>)> = Vec::new();
        for head in &mut udev.heads {
            head.location = (x, 0).into();
            head.output.change_current_state(None, None, None, Some((x, 0).into()));
            placed.push((head.output.clone(), head.location));
            x += head.size.0;
        }
        for (output, location) in placed {
            if let Some(entry) = self.outputs.iter_mut().find(|e| e.output == output) {
                entry.location = location;
            }
            self.space.map_output(&output, (location.x, location.y));
            // Bars are anchored to their output, so their geometry has to be
            // recomputed against the moved output rectangle.
            layer_map_for_output(&output).arrange();
        }
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

pub struct UdevPlatform {
    event_loop: EventLoop<'static, CompState>,
    display: Display<CompState>,
    state: CompState,
    listener: ListeningSocket,
    clients: Vec<Client>,
    pending: Rc<RefCell<Vec<CoreEvent>>>,
    ipc: Option<srdwm_platform::IpcServer>,
}

impl UdevPlatform {
    pub fn connect(wm: Rc<RefCell<WindowManager>>, bound_keys: &[String], repeat_keys: &[String]) -> PlatformResult<Self> {
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
        let connected = probe_connected(&card)?;
        log::info!("udev: {} connected output(s)", connected.len());

        let renderer = PixmanRenderer::new().map_err(err)?;
        let dh = Display::<CompState>::new().map_err(err)?;
        let display_handle = dh.handle();
        // `zwp_linux_dmabuf_v1` - see `protocols.rs`'s `DmabufHandler` for
        // why `PixmanRenderer`, a pure software renderer, can still import
        // these (mmap, not GPU). `create_global` (v3) rather than the v4
        // `..._with_default_feedback` variant: the latter needs a
        // `main_device` `dev_t` to steer multi-GPU clients toward the
        // right render node, which is a real gap worth closing later but
        // not required for a single-GPU client to allocate and hand over a
        // Linear-modifier buffer, which is all this backend can use anyway.
        let mut dmabuf_state = DmabufState::new();
        dmabuf_state.create_global::<CompState>(&display_handle, renderer.dmabuf_formats());

        let mut heads: Vec<UdevHead> = Vec::new();
        let mut output_entries: Vec<crate::state::OutputEntry> = Vec::new();
        let mut used_crtcs: Vec<crtc::Handle> = Vec::new();
        let mut x_offset = 0;
        for probe in &connected {
            let Some(crtc) = pick_crtc(&card, probe, &used_crtcs) else {
                log::warn!("udev: no free CRTC left for connector {}; not driving it", probe.name);
                continue;
            };
            let (head, entry) = bring_up_head(&card, &display_handle, probe, crtc, x_offset)?;
            log::info!("udev: head {}: {} {}x{} at x={x_offset}", heads.len(), probe.name, head.size.0, head.size.1);
            used_crtcs.push(crtc);
            x_offset += head.size.0;
            heads.push(head);
            output_entries.push(entry);
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
            dmabuf_state,
            xdg_activation_state: XdgActivationState::new::<CompState>(&display_handle),
            _text_input_manager_state: smithay::wayland::text_input::TextInputManagerState::new::<CompState>(&display_handle),
            _input_method_manager_state: smithay::wayland::input_method::InputMethodManagerState::new::<CompState, _>(&display_handle, |_client| true),
            _gtk_shell_state: crate::gtk_shell::GtkShellState::new::<CompState>(&display_handle),
            seat_state,
            seat,
            space,
            popups: PopupManager::default(),
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
            _foreign_toplevel_state: crate::foreign_toplevel::ForeignToplevelState::new::<CompState>(&display_handle),
            foreign_toplevel_managers: Vec::new(),
            foreign_toplevel_handles: HashMap::new(),
            _workspace_state: crate::workspace::WorkspaceManagerState::new::<CompState>(&display_handle),
            _output_power_state: Some(crate::output_power::OutputPowerManagerState::new::<CompState>(&display_handle)),
            _gamma_control_state: Some(crate::gamma_control::GammaControlManagerState::new::<CompState>(&display_handle)),
            _output_management_state: crate::output_management::OutputManagementState::new::<CompState>(&display_handle),
            output_managers: Vec::new(),
            output_heads: HashMap::new(),
            output_modes: HashMap::new(),
            output_serial: 0,
            last_broadcast_outputs: Vec::new(),
            workspace_managers: Vec::new(),
            workspace_groups: Vec::new(),
            workspace_handles: HashMap::new(),
            _viewporter_state: smithay::wayland::viewporter::ViewporterState::new::<CompState>(&display_handle),
            _fractional_scale_state: smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<CompState>(&display_handle),
            _cursor_shape_state: smithay::wayland::cursor_shape::CursorShapeManagerState::new::<CompState>(&display_handle),
            idle_notifier_state: smithay::wayland::idle_notify::IdleNotifierState::new(&display_handle, event_loop.handle()),
            _idle_inhibit_manager_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<CompState>(&display_handle),
            idle_inhibiting_surfaces: Vec::new(),
            last_idle_notify: None,
            window_anims: HashMap::new(),
            last_broadcast_flags: HashMap::new(),
            last_broadcast_workspace: None,
            lock: Default::default(),
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
            bound_keys: Rc::new(bound_keys.iter().cloned().collect::<HashSet<_>>()),
            repeat_keys: Rc::new(repeat_keys.iter().cloned().collect::<HashSet<_>>()),
            repeat: None,
            start_time: Instant::now(),
            udev: Some(udev_state),
            xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState::new::<CompState>(&display_handle),
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
        // Otherwise this is whatever the session inherited - typically
        // stale from a *previous* login's compositor (a shell's exported
        // `XDG_CURRENT_DESKTOP=Hyprland` surviving into this one), since
        // nothing else ever sets it. `xdg-desktop-portal` and any client
        // that sniffs this value to pick a desktop-specific integration
        // (screenshot/file-picker backends, etc.) get actively misrouted by
        // the stale value rather than just seeing "unknown". Only affects
        // processes spawned from here on (autostart, `srd.spawn`) - an
        // env var set mid-process doesn't retroactively reach anything
        // already running.
        std::env::set_var("XDG_CURRENT_DESKTOP", "srdwm");

        let ipc = match listener.socket_name().map(|n| n.to_string_lossy().into_owned()) {
            Some(name) => match srdwm_platform::IpcServer::bind(&name) {
                Ok(ipc) => Some(ipc),
                Err(e) => {
                    log::warn!("control socket unavailable ({e}); srd and scripts that use it won't work");
                    None
                }
            },
            None => None,
        };

        let handle = event_loop.handle();
        register_drm_fd(&handle, &card)?;
        register_libinput(&handle, &session, &seat_name)?;
        register_session_notifier(&handle, notifier)?;
        if let Err(e) = register_udev_monitor(&handle, &seat_name) {
            log::warn!("udev: connector hotplug unavailable ({e}); monitors are fixed at startup");
        }
        if let Err(e) = crate::xwayland::spawn(&handle, &display_handle) {
            log::warn!("XWayland unavailable ({e}); X11-only clients will not run");
        }

        Ok(Self { event_loop, display: dh, state, listener, clients: Vec::new(), pending, ipc })
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

/// Brings one connector up: allocates its scanout buffers, sets the mode,
/// and creates the `wl_output` global. Shared by startup and hotplug so a
/// monitor plugged in later is set up exactly like one present at boot.
fn bring_up_head(
    card: &Card,
    dh: &DisplayHandle,
    probe: &ConnectorProbe,
    crtc: crtc::Handle,
    x_offset: i32,
) -> PlatformResult<(UdevHead, crate::state::OutputEntry)> {
    let (width, height) = probe.mode.size();
    let (width, height) = (width as i32, height as i32);

    let buffers = [make_drm_buffer(card, width, height)?, make_drm_buffer(card, width, height)?];
    card.set_crtc(crtc, Some(buffers[0].fb), (0, 0), &[probe.connector], Some(probe.mode)).map_err(err)?;

    // Named after the real connector (eDP-1, HDMI-A-1, ...) so clients and
    // the user can tell monitors apart; `wl_output.name` is what a bar's
    // per-monitor config keys off.
    //
    // Physical size in millimeters comes straight from EDID via the
    // connector, not the hardcoded (0, 0) this used to be - some clients
    // compute their own effective DPI from it (independently of the
    // compositor's own scale factor, which srdwm always reports as 1), so
    // reporting "no physical size at all" was live, wrong data reaching
    // every client, not just an unfilled-in placeholder.
    let (phys_w, phys_h) = probe.info.size().unwrap_or((0, 0));
    let physical_mm = (phys_w as i32, phys_h as i32);
    let output = Output::new(
        probe.name.clone(),
        PhysicalProperties { size: physical_mm.into(), subpixel: Subpixel::Unknown, make: "srdwm".into(), model: "drm".into() },
    );
    let mode = OutputMode { size: (width, height).into(), refresh: mode_refresh_mhz(&probe.mode) };
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((x_offset, 0).into()));
    output.set_preferred(mode);
    let global = output.create_global::<CompState>(dh);

    let location: Point<i32, Logical> = (x_offset, 0).into();
    let head = UdevHead {
        crtc,
        connector: probe.connector,
        output: output.clone(),
        global,
        damage_tracker: OutputDamageTracker::from_output(&output),
        buffers,
        front: 0,
        flip_pending: false,
        ages: [0, 0],
        location,
        size: (width, height),
    };
    Ok((head, crate::state::OutputEntry { output, location }))
}

/// A connected connector and the mode we intend to drive it at. CRTC
/// assignment is deliberately separate ([`pick_crtc`]) so a hotplug re-probe
/// can leave surviving heads on the CRTCs they already hold.
struct ConnectorProbe {
    connector: connector::Handle,
    info: connector::Info,
    mode: DrmMode,
    /// Connector name as the kernel reports it (`eDP-1`, `HDMI-A-1`, ...).
    name: String,
}

/// Every connector currently reporting `Connected`, with its preferred mode.
///
/// Forces a fresh probe (`get_connector(.., true)`) rather than trusting
/// cached state - on a hotplug the cached status is exactly what has gone
/// stale.
fn probe_connected(card: &Card) -> PlatformResult<Vec<ConnectorProbe>> {
    let res = card.resource_handles().map_err(err)?;
    let mut probes = Vec::new();
    for handle in res.connectors() {
        let Ok(info) = card.get_connector(*handle, true) else { continue };
        if info.state() != connector::State::Connected {
            continue;
        }
        let name = format!("{:?}-{}", info.interface(), info.interface_id());
        // Prefer the mode the display advertises as PREFERRED (its native
        // resolution) rather than whatever happens to be listed first --
        // the list order is not guaranteed, and picking wrong means running
        // a monitor at the wrong resolution.
        let Some(&mode) = info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| info.modes().first())
        else {
            log::warn!("udev: connector {name} is connected but reports no modes; skipping");
            continue;
        };
        probes.push(ConnectorProbe { connector: *handle, info, mode, name });
    }
    Ok(probes)
}

/// Picks a CRTC for `probe` that is not in `used`.
///
/// CRTCs are a finite hardware resource and cannot be shared, so a machine
/// with more connected monitors than CRTCs drives as many as the hardware
/// allows and logs the rest rather than failing outright.
fn pick_crtc(card: &Card, probe: &ConnectorProbe, used: &[crtc::Handle]) -> Option<crtc::Handle> {
    let res = card.resource_handles().ok()?;
    // Prefer the CRTC already driving this connector, else any free one the
    // encoder can reach, else anything free at all.
    probe
        .info
        .current_encoder()
        .and_then(|enc| card.get_encoder(enc).ok())
        .map(|enc| res.filter_crtcs(enc.possible_crtcs()))
        .unwrap_or_default()
        .into_iter()
        .chain(res.crtcs().iter().copied())
        .find(|c| !used.contains(c))
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
                        // the VT switch (another VT's session may have
                        // scanned out something else entirely in between).
                        head.flip_pending = false;
                        head.ages = [0, 0];
                    }
                    data.render_udev_frame();
                }
            }
        })
        .map_err(|e| PlatformError::Other(format!("failed to register session notifier: {e}")))?;
    Ok(())
}

/// Watches udev for DRM device changes. The kernel emits a `change` uevent
/// on the card when a connector is plugged or unplugged, which smithay
/// surfaces as [`UdevEvent::Changed`] - that is the hotplug signal.
///
/// `Added`/`Removed` refer to whole GPUs appearing or disappearing, which
/// this backend does not support (it binds one primary GPU at startup), so
/// they are logged and ignored rather than silently dropped.
fn register_udev_monitor(handle: &LoopHandle<'static, CompState>, seat_name: &str) -> PlatformResult<()> {
    let backend = UdevBackend::new(seat_name).map_err(err)?;
    handle
        .insert_source(backend, move |event, _, data: &mut CompState| match event {
            UdevEvent::Changed { .. } => {
                data.reprobe_outputs();
                data.render_udev_frame();
            }
            UdevEvent::Added { path, .. } => {
                log::info!("udev: new GPU {} appeared; multi-GPU is not supported, ignoring", path.display())
            }
            UdevEvent::Removed { .. } => log::info!("udev: a GPU was removed; multi-GPU is not supported, ignoring"),
        })
        .map_err(|e| PlatformError::Other(format!("failed to register udev monitor: {e}")))?;
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
        // Laptop lid. libinput reports this as a switch toggle; without
        // handling it, closing the lid does nothing at all - no lock, no
        // suspend - which is a genuine problem on a laptop rather than a
        // missing nicety.
        InputEvent::SwitchToggle { event } => {
            // Fully qualified: libinput's own `Switch` is also in scope here.
            use smithay::backend::input::{SwitchState, SwitchToggleEvent};
            if matches!(event.switch(), Some(smithay::reexports::input::event::switch::Switch::Lid)) {
                let closed = event.state() == SwitchState::On;
                log::info!("lid {}", if closed { "closed" } else { "opened" });
                state.pending.borrow_mut().push(CoreEvent::LidSwitch { closed });
            }
        }
        InputEvent::PointerAxis { event } => {
            // Modifier+scroll switches workspace instead of reaching the
            // client - the `bind = SUPER, mouse_down/up, workspace, e+1/e-1`
            // gesture. Checked first so the client never sees these events;
            // forwarding them too would scroll the window under the cursor
            // as a side effect of changing workspace.
            if crate::input::handle_workspace_scroll(state, &event) {
                return;
            }
            // Otherwise: forwarded to the focused client via the pointer axis
            // frame, no WM-level handling.
            let Some(pointer) = state.seat.get_pointer() else { return };
            let source = event.source();
            let mut frame = AxisFrame::new(event.time_msec()).source(source);
            for axis in [Axis::Horizontal, Axis::Vertical] {
                match event.amount(axis) {
                    Some(value) => frame = frame.value(axis, value),
                    // `AxisSource::Finger` (a touchpad) *requires* a stop
                    // event on the frame where the finger lifts and the
                    // axis genuinely has no more motion - see `AxisFrame::
                    // source`'s own doc comment ("Using AxisSource::Finger
                    // requires a stop event to be sent, when the user lifts
                    // off the finger"). Never sending it left every
                    // two-finger scroll gesture with no way to tell Firefox/
                    // GTK it had actually ended, which is exactly the kind
                    // of thing that reads as "scrolling doesn't work" --
                    // not "no events arrive" (discrete wheel scrolling,
                    // which needs no stop event, was never affected) but
                    // kinetic/momentum scrolling and starting a fresh
                    // gesture right after a previous one never settling.
                    None if source == AxisSource::Finger => frame = frame.stop(axis),
                    None => {}
                }
                // Discrete wheel steps, additional to the pixel `value`
                // above - optional (`value` is the only event a client
                // strictly needs), but some clients use it to distinguish
                // "one physical click" from a smooth/high-resolution
                // scroll, so provide it whenever the device actually
                // reports one (real scroll wheels; never touchpads, which
                // have no discrete steps to report - `amount_v120` is
                // `None` for those, same guarantee `amount` gives the
                // other way around).
                if let Some(v120) = event.amount_v120(axis) {
                    frame = frame.v120(axis, v120 as i32);
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
        // Held bindings that repeat - see `CompState::tick_repeat`.
        self.state.tick_repeat();
        self.display.dispatch_clients(&mut self.state).map_err(err)?;
        self.display.flush_clients().map_err(err)?;
        if let Some(ipc) = self.ipc.as_mut() {
            if ipc.poll(&self.state.wm) {
                self.pending.borrow_mut().push(CoreEvent::WorkspaceChanged);
            }
        }
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
                // Shrunk by whatever a layer-shell surface (bar, dock) has
                // reserved via `set_exclusive_zone` - reporting the full
                // head size here otherwise means core's placement/tiling
                // treats that strip as ordinary free space, so a new
                // window's titlebar lands right where the bar renders on
                // top of it, unreachable to drag. `non_exclusive_zone()` is
                // output-local, so it's translated into this head's
                // position in the shared global space the same way
                // `head.location` already is.
                let zone = layer_map_for_output(&head.output).non_exclusive_zone();
                let rect = srdwm_core::Rect::new(
                    head.location.x + zone.loc.x,
                    head.location.y + zone.loc.y,
                    zone.size.w as u32,
                    zone.size.h as u32,
                );
                let mut m = srdwm_core::Monitor::new(i as u32, head.output.name(), rect);
                // `Monitor::new` defaults `full_geometry` to whatever
                // `geometry` was constructed with - correct for a monitor
                // with no layer-shell client at all, wrong the moment one
                // exists, since `rect` above is already zone-shrunk. Without
                // this, `full_geometry` was silently identical to `geometry`
                // for every real monitor this backend ever reported, which
                // made `toggle_fullscreen`'s whole "ignore the reserved
                // zone" design a no-op in practice: fullscreen still
                // stopped at the bar/dock exactly like maximize does.
                // Reported live as "fullscreen isn't actually going
                // fullscreen" - confirmed by triggering it and reading
                // the resulting geometry back over IPC, not just from
                // reading this code.
                m.full_geometry = srdwm_core::Rect::new(head.location.x, head.location.y, head.size.0 as u32, head.size.1 as u32);
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

    /// Was `wm.focus_window(window)` alone - core-only, so a caller that
    /// only has `Platform` to go through (`crates/platform`'s `IpcServer`,
    /// which can't reach `CompState`/real Wayland focus at all) could make
    /// a window *look* focused (rendering already reads live core state
    /// for the highlighted-border/titlebar-text colour) without it ever
    /// actually receiving a keystroke - confirmed live: `srd dispatch
    /// focus <xwayland-window-id>` changed core's own focused-window
    /// bookkeeping but left `_NET_ACTIVE_WINDOW` at `0x0` and real
    /// keyboard input going nowhere. `crate::input::focus_window` is the
    /// same full path a real mouse click already goes through.
    fn focus(&mut self, window: srdwm_core::WindowId) -> PlatformResult<()> {
        crate::input::focus_window(&mut self.state, window);
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
