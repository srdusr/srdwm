//! Opt-in (`SRDWM_GPU=1`) capability probe for a real GBM+EGL GPU render
//! path on the udev backend - see [`probe`]'s own doc comment for exactly
//! what this does and does not do yet.

use std::os::fd::{AsFd, OwnedFd};

use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::reexports::gbm::Device as GbmDevice;
use smithay::reexports::rustix;

use super::Card;

/// Marker returned by a successful probe. Carries no data yet - see
/// [`probe`]'s own doc comment for why - this exists purely to answer,
/// definitively and safely, "does GBM+EGL actually initialize on this exact
/// machine's DRM device" (logged from inside `probe` itself) as a plain
/// `bool`-shaped `Option` a future caller can match on once there's
/// something to actually do with a `Some`.
pub(crate) struct GpuProbe;

/// Attempts GBM device creation plus an EGL display/device query against
/// `card`'s DRM fd, gated behind `SRDWM_GPU=1` (unset - the default --
/// skips this entirely: zero cost, zero risk, identical to every session
/// before this one). Returns `None` on *any* failure at any step, each
/// logged with which step failed and why, so a machine without working
/// KMS+3D driver support (or a VM with only dumb-buffer scanout - this
/// backend's own long-standing default target, see `udev/mod.rs`'s module
/// doc comment) gets one clear, harmless log line instead of anything
/// touching the actual rendering/modesetting this backend already does.
///
/// Deliberately does **not** yet create an `EGLContext`, a `GlesRenderer`,
/// or touch scanout at all. Reading smithay's own reference compositor
/// (`anvil/src/udev.rs`) confirmed it wires GBM+EGL rendering together with
/// *atomic*-KMS scanout as one unit via `DrmCompositor`, not as a renderer
/// swapped into the existing legacy `set_crtc`/`page_flip` flip loop this
/// backend uses today. Adopting `DrmCompositor` is real, separate,
/// larger-scoped work than "swap the renderer" - it replaces the same
/// `UdevHead` mode-set/flip machinery this session's own VT-switch fixes
/// (`register_session_notifier`'s `ActivateSession` arm, `copy_and_flip`'s
/// retry backoff) live in, and needs its own plan. This probe answers the
/// *first* question - does the hardware even support it at all - safely,
/// before that larger integration is scoped and attempted.
pub(crate) fn probe(card: &Card) -> Option<GpuProbe> {
    if std::env::var("SRDWM_GPU").as_deref() != Ok("1") {
        return None;
    }
    // GBM wants its own fd, not the one this backend's KMS control-plane
    // ioctls (`set_crtc`, `page_flip`, property gets/sets) already share
    // via `Rc<Card>` - duplicated here rather than reusing `card`'s own
    // fd directly, both because `gbm::Device`'s `EGLNativeDisplay` impl
    // needs `T: Send + 'static` (an `Rc`-shared `Card` is neither) and
    // because giving GBM a fd genuinely separate from the control-plane
    // one matches real compositor practice, not just this one's
    // convenience.
    let dup_fd: OwnedFd = match rustix::io::dup(card.as_fd()) {
        Ok(fd) => fd,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but failed to dup the DRM fd for GBM: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    let gbm = match GbmDevice::new(dup_fd) {
        Ok(gbm) => gbm,
        Err(e) => {
            log::warn!("udev: SRDWM_GPU=1 but GBM device creation failed: {e} - staying on the software (Pixman) render path");
            return None;
        }
    };
    // Safety: `gbm` owns its own fd (the dup above), stays alive for as
    // long as this function needs it, and is a real GBM device - exactly
    // what `EGLDisplay::new`'s own safety contract (a native display handle
    // that outlives the `EGLDisplay`) requires.
    let display = match unsafe { EGLDisplay::new(gbm) } {
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
    log::info!(
        "udev: SRDWM_GPU=1 - GBM+EGL capability confirmed on this hardware (render node: {render_node:?}). \
         Full GPU rendering isn't wired up yet (see gpu::probe's own doc comment); still rendering via Pixman for now."
    );
    Some(GpuProbe)
}
