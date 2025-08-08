//! Opt-in (`general.gpu` in config, or the lower-level `SRDWM_GPU=1` env
//! var) GBM+EGL+`DrmCompositor` GPU render path for the udev backend --
//! see [`probe`]'s own doc comment for exactly what this does and does
//! not do yet. Past the original Phase 2 (one output,
//! clear-color only - see the plan this was built from, `snappy-
//! percolating-boole.md`, for that phase's own scoping): every head
//! `initialize_output` succeeds for gets driven, not just the first
//! (`GpuContext::outputs`), VT-switch pause/activate is wired
//! (`udev/session.rs`'s `SessionEvent` handlers), and the real cursor and
//! real window content both render on top of the clear color
//! (`udev/render.rs`'s own GPU branch). Window content is plain
//! `surface_content_elements` - square corners, no border or titlebar --
//! not yet the masked/rounded path the Pixman branch uses (built against
//! `PixmanRenderer` specifically) or the GLES shader `winit/render.rs`
//! already has for its own single-output case; decorations (border,
//! titlebar) are the remaining real gap. Untested on real GPU-enabled
//! hardware as of this writing - `SRDWM_GPU`/`general.gpu` were both
//! unset on the machine this was built on, so this compiles, passes the
//! full test suite, and matches the existing Pixman path's own per-
//! window geometry logic by inspection, but has not been visually
//! confirmed against a real compositor session with the flag on.

use std::os::fd::{AsFd, OwnedFd};

use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier};
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
use smithay::reexports::drm::buffer::DrmFourcc;
use smithay::reexports::drm::control::{connector, crtc, Mode as DrmMode};
use smithay::reexports::rustix;
use smithay::utils::DeviceFd;

use super::Card;

/// The render-element type real frames on the GPU path push through --
/// `crate::elements::OverlayElement`, the same enum (Surface/Memory/Solid)
/// the Pixman path's own `custom_elements` already uses, just instantiated
/// for `GlesRenderer` instead of `PixmanRenderer`. Started out as a bare
/// `MemoryRenderBufferRenderElement<GlesRenderer>` back when this phase's
/// element list was always empty and the concrete choice genuinely didn't
/// matter - widened once `render_udev_frame`'s GPU branch started pushing
/// a real cursor (`cursor::render_elements`' own return type) through
/// `render_frame`.
type GpuElement = crate::elements::OverlayElement<GlesRenderer>;

/// One head successfully driven through `DrmOutputManager::initialize_output`
/// - see [`GpuContext::initialize_output`]'s own doc comment.
pub(crate) type GpuOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<smithay::desktop::utils::OutputPresentationFeedback>,
    DrmDeviceFd,
>;

/// A `DrmOutputManager` instantiated with this backend's concrete
/// allocator/exporter/fd types - spelled out once here so every later
/// field/signature that needs it doesn't have to repeat all four type
/// parameters.
pub(crate) type GpuOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<smithay::desktop::utils::OutputPresentationFeedback>,
    DrmDeviceFd,
>;

/// Everything a successful [`probe`] built: a real GLES renderer bound to
/// this hardware's GPU, and a `DrmOutputManager` ready for
/// `initialize_output` calls. Nothing here is wired into `render_udev_frame`
/// yet - that is the next step in the plan this was built from, done
/// separately once this compiles and the log confirms it actually
/// initializes on this hardware.
pub(crate) struct GpuContext {
    pub(crate) renderer: GlesRenderer,
    pub(crate) output_manager: GpuOutputManager,
    /// Every head [`GpuContext::initialize_output`] was successfully
    /// called for - Phase 2 of the plan this was built from only ever
    /// targeted a single head; `udev/platform.rs`'s startup path now calls
    /// `initialize_output` for every connected head instead of just the
    /// first, so a machine with several monitors gets all of them through
    /// this same GBM+EGL+`DrmCompositor` pipeline rather than only one.
    /// `DrmOutputManager` itself already supports driving multiple crtcs
    /// at once (`initialize_output` is a per-crtc call on the one shared
    /// manager, same as `anvil`'s own multi-output handling) - Phase 2
    /// simply never exercised that. A head this fails for individually
    /// (logged, not fatal) just has no entry here and falls back to the
    /// existing legacy Pixman path exactly as before, same as it always
    /// could when `SRDWM_GPU` was unset entirely. `render_udev_frame`
    /// (`udev/render.rs`) looks a head's own crtc up here to decide
    /// whether it renders through this path or through Pixman.
    pub(crate) outputs: Vec<(crtc::Handle, GpuOutput)>,
}

impl GpuContext {
    /// The `GpuOutput` driving `crtc`, if [`initialize_output`](Self::initialize_output)
    /// was called for it and succeeded. Only an `&self` lookup (`session.rs`'s
    /// VBlank handler doesn't need mutable access) - `render.rs`'s own
    /// render-frame call site needs `&mut gpu.outputs` *and* `&mut gpu.
    /// renderer` at once, which it does via direct field access instead of
    /// an equivalent `&mut self` method here, since Rust's disjoint-field-
    /// borrow analysis only sees through direct field access, not a method
    /// call that borrows all of `self` even when it only touches one field.
    pub(crate) fn output_for(&self, crtc: crtc::Handle) -> Option<&GpuOutput> {
        self.outputs.iter().find(|(c, _)| *c == crtc).map(|(_, o)| o)
    }
}

/// Reasonable, widely-supported scanout formats to try, most-preferred
/// first - the same two `anvil` tries before falling back further,
/// without that example's own optional 10-bit path (`ANVIL_DISABLE_10BIT`
/// is anvil-specific and this backend has no equivalent theme/config
/// concept for it yet).
const COLOR_FORMATS: [DrmFourcc; 2] = [DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

/// Attempts the full GBM+EGL+GLES+`DrmDevice`+`DrmOutputManager` chain
/// against `card`'s DRM fd, gated behind `enabled` (the caller's own
/// combined decision - `udev/platform.rs`'s call site attempts this
/// whenever *either* `general.gpu` in config or the lower-level
/// `SRDWM_GPU=1` env-var override says to; both unset/`false` is the
/// default, and skips this entirely: zero cost, zero risk, identical to
/// every session before this option existed). Returns `None` on *any*
/// failure at any step, each logged with which step failed and why, so a
/// machine without working KMS+3D driver support (or a VM with only
/// dumb-buffer scanout - this backend's own long-standing default
/// target, see `udev/mod.rs`'s module doc comment) gets one clear,
/// harmless log line instead of anything touching the actual rendering/
/// modesetting this backend already does, and falls straight back to
/// that same always-available software path with no further action
/// needed from whoever enabled this.
///
/// `DrmDevice::new`'s own `disable_connectors` parameter (passed `false`
/// here, matching the existing legacy path's own connector handling) is
/// *not* an atomic-vs-legacy switch, despite that being an easy assumption
/// - reading smithay's own source (`backend/drm/device/mod.rs`) directly
/// showed `DrmDevice::create_internal` tries atomic capability first and
/// falls back to a `Legacy` internal variant automatically if the driver
/// doesn't support it, both exposed through the one `DrmDevice` type via
/// its own `is_atomic()` query - logged here, not assumed, since Phase 1's
/// probe never got far enough to find this out empirically on this
/// specific machine's `i915` driver.
///
/// Deliberately does **not** yet call `initialize_output` for any specific
/// head, or touch `render_udev_frame` at all - this function's whole job
/// is building a ready-to-use `GpuContext`; which head (if any) actually
/// gets driven through it is a per-session, per-head decision the caller
/// (`udev/platform.rs`'s startup path) makes once this succeeds.
pub(crate) fn probe(card: &Card, enabled: bool) -> Option<(GpuContext, DrmDeviceNotifier)> {
    if !enabled {
        return None;
    }
    // One `DrmDeviceFd` used for everything below - GBM, EGL, and the
    // `DrmDevice` itself all share this single duped fd (cheaply cloned,
    // `DrmDeviceFd` is `Arc`-backed), matching `anvil`'s own construction
    // rather than Phase 1's separate raw-fd dup for GBM alone. Duped from
    // `card`'s own fd (not reused directly) both because `gbm::Device`'s
    // `EGLNativeDisplay` impl and `DrmDeviceFd` itself need `Send +
    // 'static` (an `Rc`-shared `Card` is neither) and because keeping this
    // whole GPU path on a fd genuinely separate from the existing legacy
    // control-plane one (`Card`, still driving every non-GPU head)
    // guarantees this experimental path can never contend with or disrupt
    // it at the fd level.
    let dup_fd: OwnedFd = match rustix::io::dup(card.as_fd()) {
        Ok(fd) => fd,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but failed to dup the DRM fd: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    let fd = DrmDeviceFd::new(DeviceFd::from(dup_fd));
    let gbm = match GbmDevice::new(fd.clone()) {
        Ok(gbm) => gbm,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but GBM device creation failed: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    // Safety: `gbm` owns its own fd (via the shared `DrmDeviceFd` above),
    // stays alive for as long as this function needs it, and is a real GBM
    // device - exactly what `EGLDisplay::new`'s own safety contract (a
    // native display handle that outlives the `EGLDisplay`) requires.
    let display = match unsafe { EGLDisplay::new(gbm.clone()) } {
        Ok(d) => d,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but EGL display creation failed: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    let egl_device = match EGLDevice::device_for_display(&display) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but no EGL device found for this display: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    if egl_device.is_software() {
        log::warn!("udev: SRDWM_GPU=1 but the only EGL device found is a software rasterizer - staying on the software (Pixman) render path");
        return None;
    }
    let render_node = egl_device.try_get_render_node().ok().flatten();
    let context = match EGLContext::new(&display) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but EGL context creation failed: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    // Safety: `context` was just created above, is not shared with
    // anything else, and this function does not touch it again after this
    // call - exactly what `GlesRenderer::new`'s own safety contract (sole,
    // current ownership of the context being wrapped) requires.
    let renderer = match unsafe { GlesRenderer::new(context) } {
        Ok(r) => r,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but GLES renderer creation failed: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    let (drm_device, notifier) = match DrmDevice::new(fd, false) {
        Ok(pair) => pair,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but DrmDevice creation failed: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    log::info!(
        "udev: SRDWM_GPU=1 - DrmDevice created (render node: {render_node:?}, atomic modesetting: {}).",
        drm_device.is_atomic()
    );
    let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);
    let render_formats = renderer.egl_context().dmabuf_render_formats().iter().copied();
    let output_manager = DrmOutputManager::new(drm_device, allocator, exporter, Some(gbm), COLOR_FORMATS, render_formats);
    log::info!(
        "udev: SRDWM_GPU=1 - GBM+EGL+GLES+DrmOutputManager all initialized successfully on this hardware. \
         No output is being driven through it yet (see gpu::probe's own doc comment); every head still renders via Pixman for now."
    );
    Some((GpuContext { renderer, output_manager, outputs: Vec::new() }, notifier))
}

impl GpuContext {
    /// Drives one head (`crtc`/`mode`/`connector`/`output`, the same
    /// values `udev/drm.rs`'s `bring_up_head` already resolved for this
    /// head's *existing* legacy `UdevHead`) through this context's shared
    /// `DrmOutputManager`, appending the resulting `GpuOutput` to
    /// `self.outputs` on success - call once per connected head from
    /// `udev/platform.rs`'s startup path, same loop that already brings up
    /// each head's legacy `UdevHead`.
    ///
    /// `elements` is always empty here - initial output setup, not a real
    /// frame - so the concrete choice of `E` ([`GpuElement`], a plain
    /// `MemoryRenderBufferRenderElement`) is arbitrary.
    ///
    /// Returns `false` and leaves `self.outputs` unchanged on failure
    /// (logged): that one head simply never gets an entry, so it renders
    /// through the existing legacy Pixman path exactly as if `SRDWM_GPU`
    /// had never been set for it, while every other head this succeeded
    /// for is unaffected.
    pub(crate) fn initialize_output(&mut self, crtc: crtc::Handle, mode: DrmMode, connector: connector::Handle, output: &Output) -> bool {
        let elements: DrmOutputRenderElements<GlesRenderer, GpuElement> = DrmOutputRenderElements::default();
        match self.output_manager.initialize_output::<GlesRenderer, GpuElement>(crtc, mode, &[connector], output, None, &mut self.renderer, &elements) {
            Ok(gpu_output) => {
                log::info!("udev: SRDWM_GPU=1 - output initialized through DrmOutputManager for crtc {crtc:?}");
                self.outputs.push((crtc, gpu_output));
                true
            }
            Err(e) => {
                log::warn!("udev: SRDWM_GPU=1 but initialize_output failed for crtc {crtc:?}: {e:?} - this head stays on the software (Pixman) render path");
                false
            }
        }
    }
}
