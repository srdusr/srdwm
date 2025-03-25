//! Opt-in (`SRDWM_GPU=1`) GBM+EGL+`DrmCompositor` GPU render path for the
//! udev backend - see [`probe`]'s own doc comment for exactly what this
//! does and does not do yet (Phase 2: one output, clear-color only, no
//! window content/decorations, no VT-switch support - see the plan this
//! was built from, `snappy-percolating-boole.md`, for the full scoping).

use std::os::fd::{AsFd, OwnedFd};

use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier};
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
use smithay::reexports::drm::buffer::DrmFourcc;
use smithay::reexports::drm::control::{connector, crtc, Mode as DrmMode};
use smithay::reexports::rustix;
use smithay::utils::DeviceFd;

use super::Card;

/// The concrete render-element type this phase's (empty) element list uses
/// - see [`GpuContext::initialize_output`]'s own doc comment for why the
/// actual choice of `E` doesn't matter yet, since no real elements are
/// passed through it.
type GpuElement = MemoryRenderBufferRenderElement<GlesRenderer>;

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
    /// The one head [`GpuContext::initialize_output`] was successfully
    /// called for, if any - Phase 2 of the plan this was built from only
    /// ever targets a single head, see that call site (`udev/platform.rs`)
    /// for which one and why. `render_udev_frame` (`udev/render.rs`)
    /// checks this to decide whether a given head renders through here or
    /// through the existing legacy Pixman path.
    pub(crate) output: Option<(crtc::Handle, GpuOutput)>,
}

/// Reasonable, widely-supported scanout formats to try, most-preferred
/// first - the same two `anvil` tries before falling back further,
/// without that example's own optional 10-bit path (`ANVIL_DISABLE_10BIT`
/// is anvil-specific and this backend has no equivalent theme/config
/// concept for it yet).
const COLOR_FORMATS: [DrmFourcc; 2] = [DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

/// Attempts the full GBM+EGL+GLES+`DrmDevice`+`DrmOutputManager` chain
/// against `card`'s DRM fd, gated behind `SRDWM_GPU=1` (unset - the
/// default - skips this entirely: zero cost, zero risk, identical to
/// every session before this one). Returns `None` on *any* failure at any
/// step, each logged with which step failed and why, so a machine without
/// working KMS+3D driver support (or a VM with only dumb-buffer scanout --
/// this backend's own long-standing default target, see `udev/mod.rs`'s
/// module doc comment) gets one clear, harmless log line instead of
/// anything touching the actual rendering/modesetting this backend already
/// does.
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
pub(crate) fn probe(card: &Card) -> Option<(GpuContext, DrmDeviceNotifier)> {
    if std::env::var("SRDWM_GPU").as_deref() != Ok("1") {
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
    Some((GpuContext { renderer, output_manager, output: None }, notifier))
}

impl GpuContext {
    /// Drives exactly one head (`crtc`/`mode`/`connector`/`output`, the
    /// same values `udev/drm.rs`'s `bring_up_head` already resolved for
    /// this head's *existing* legacy `UdevHead`) through this context's
    /// `DrmOutputManager`, returning the resulting `GpuOutput` on success.
    ///
    /// `elements` is always empty for this phase (Phase 2 of the plan this
    /// was built from renders a plain clear color only, no window content/
    /// decorations/cursor yet) - so the concrete choice of `E` here
    /// ([`GpuElement`], a plain `MemoryRenderBufferRenderElement`) is
    /// arbitrary; nothing about it is load-bearing until a later phase
    /// actually pushes real elements through this same call shape.
    ///
    /// Returns `false` and leaves `self.output` untouched on failure
    /// (logged) - same fallback contract as [`probe`] itself: a head this
    /// fails for simply never gets an entry in `self.output_manager`'s
    /// internal map, so it can still be driven through the existing
    /// legacy Pixman path exactly as if `SRDWM_GPU` had never been set for
    /// that particular head.
    pub(crate) fn initialize_output(&mut self, crtc: crtc::Handle, mode: DrmMode, connector: connector::Handle, output: &Output) -> bool {
        let elements: DrmOutputRenderElements<GlesRenderer, GpuElement> = DrmOutputRenderElements::default();
        match self.output_manager.initialize_output::<GlesRenderer, GpuElement>(crtc, mode, &[connector], output, None, &mut self.renderer, &elements) {
            Ok(gpu_output) => {
                log::info!("udev: SRDWM_GPU=1 - output initialized through DrmOutputManager for crtc {crtc:?}");
                self.output = Some((crtc, gpu_output));
                true
            }
            Err(e) => {
                log::warn!("udev: SRDWM_GPU=1 but initialize_output failed for crtc {crtc:?}: {e:?} - this head stays on the software (Pixman) render path");
                false
            }
        }
    }
}
