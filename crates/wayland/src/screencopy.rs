//! `wlr-screencopy-unstable-v1`: lets a client ask the compositor for the
//! contents of an output (or a region of one). This is what `grim` uses, and
//! therefore what the user's `Print`/`Alt+Print` screenshot binds
//! (`grim`, `slurp | grim -g -`) and `wf-recorder` need.
//!
//! Unlike every other protocol this backend speaks, smithay 0.7 ships no
//! helper for this one - there is no `ScreencopyState`/`ScreencopyHandler`
//! to delegate to - so the `GlobalDispatch`/`Dispatch` plumbing below is
//! written out by hand against the raw `wayland-protocols-wlr` server
//! bindings.
//!
//! Capture is deferred, not immediate: a `copy` request only *queues* the
//! frame (`CompState::screencopy_pending`), and the actual pixels are read
//! back inside the render pass, from the framebuffer that was just drawn
//! (see `service_pending`). Doing it at request time would either capture
//! the previous frame or require an extra off-screen render of the whole
//! scene; reading back the real framebuffer is both cheaper and matches what
//! is actually on screen.

use std::time::UNIX_EPOCH;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::{ExportMem, Renderer};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Buffer as BufferCoord, Physical, Rectangle, Size};
use smithay::wayland::shm::with_buffer_contents_mut;
use wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};

use crate::state::CompState;

/// Every pixel we hand out is 4 bytes; the protocol needs a stride and the
/// shm format that matches what `copy_framebuffer` is asked to produce.
const BYTES_PER_PIXEL: u32 = 4;
const CAPTURE_FOURCC: Fourcc = Fourcc::Xrgb8888;
const CAPTURE_SHM_FORMAT: wl_shm::Format = wl_shm::Format::Xrgb8888;

/// The screencopy manager global. Held by `CompState` purely to keep the
/// global alive for the compositor's lifetime.
#[derive(Debug)]
pub struct ScreencopyState {
    _global: smithay::reexports::wayland_server::backend::GlobalId,
}

impl ScreencopyState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ()> + 'static,
    {
        // Version 3 advertises `linux_dmabuf`/`buffer_done`, which this
        // software-readback implementation does not support, so cap at 2 --
        // that still covers `copy_with_damage`, which `wf-recorder` uses.
        Self { _global: dh.create_global::<D, ZwlrScreencopyManagerV1, _>(2, ()) }
    }
}

/// State attached to each `zwlr_screencopy_frame_v1`.
#[derive(Debug, Clone)]
pub struct FrameData {
    /// Region of the output to capture, in physical pixels.
    pub region: Rectangle<i32, Physical>,
    /// The output this frame captures. `None` if the `wl_output` the client
    /// named at request time doesn't resolve to a live output (e.g.
    /// unplugged between bind and capture) - such a frame is failed
    /// immediately and never queued, so this is only read on that path.
    pub output: Option<Output>,
    /// Set once `copy`/`copy_with_damage` has been handled, so a second one
    /// can be rejected with the protocol's `already_used` error.
    pub used: bool,
}

/// A capture that has been requested but not yet serviced. Drained by
/// `service_pending` during the next render pass.
#[derive(Debug)]
pub struct PendingCapture {
    pub frame: ZwlrScreencopyFrameV1,
    pub buffer: WlBuffer,
    pub region: Rectangle<i32, Physical>,
    /// Which head this capture is bound to - the udev backend renders each
    /// head into its own framebuffer, so a capture must be serviced against
    /// the framebuffer for *this* output, not whichever head happens to
    /// render first (see `render_udev_frame`'s per-output split).
    pub output: Output,
    /// `copy_with_damage` clients expect a `damage` event before `ready`.
    pub with_damage: bool,
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for CompState {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        manager: New<ZwlrScreencopyManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(manager, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_manager_v1::Request;
        let (frame, region, output) = match request {
            Request::CaptureOutput { frame, overlay_cursor: _, output } => {
                (frame, state.output_capture_region(&output), state.output_for_wl(&output).map(|e| e.output.clone()))
            }
            Request::CaptureOutputRegion { frame, overlay_cursor: _, output, x, y, width, height } => {
                // Clamp to the output: a client is free to ask for a region
                // hanging off the edge (slurp will, at a screen border), and
                // `copy_framebuffer` errors out on out-of-bounds reads.
                let full = state.output_capture_region(&output);
                let requested = Rectangle::new((x, y).into(), (width.max(0), height.max(0)).into());
                let region = full.intersection(requested).unwrap_or_default();
                (frame, region, state.output_for_wl(&output).map(|e| e.output.clone()))
            }
            Request::Destroy => return,
            _ => return,
        };

        let frame = data_init.init(frame, FrameData { region, output: output.clone(), used: false });
        if region.size.w <= 0 || region.size.h <= 0 || output.is_none() {
            // Nothing to capture: an empty/off-screen region, or a
            // `wl_output` that no longer resolves to a live head.
            frame.failed();
            return;
        }
        frame.buffer(
            CAPTURE_SHM_FORMAT,
            region.size.w as u32,
            region.size.h as u32,
            region.size.w as u32 * BYTES_PER_PIXEL,
        );
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_frame_v1::Request;
        let (buffer, with_damage) = match request {
            Request::Copy { buffer } => (buffer, false),
            Request::CopyWithDamage { buffer } => (buffer, true),
            Request::Destroy => {
                state.screencopy_pending.retain(|p| &p.frame != frame);
                return;
            }
            _ => return,
        };

        if data.used {
            frame.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "frame was already copied");
            return;
        }

        // Validate the buffer really can hold the frame before promising a
        // capture - a mismatch here would otherwise be a silent short write.
        let expected_stride = data.region.size.w as u32 * BYTES_PER_PIXEL;
        let ok = with_buffer_contents_mut(&buffer, |_ptr, len, spec| {
            spec.format == CAPTURE_SHM_FORMAT
                && spec.width == data.region.size.w
                && spec.height == data.region.size.h
                && spec.stride as u32 == expected_stride
                && len >= (expected_stride * data.region.size.h as u32) as usize
        })
        .unwrap_or(false);
        if !ok {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "buffer does not match the advertised format/size/stride",
            );
            return;
        }

        // A frame with no resolved output was already failed at request
        // time (see the manager's `request` handler) and should never reach
        // `Copy`/`CopyWithDamage` from a well-behaved client; guarded rather
        // than unwrapped so a misbehaving one can't panic the compositor.
        let Some(output) = data.output.clone() else {
            frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "frame has no output to capture");
            return;
        };

        state.screencopy_pending.push(PendingCapture {
            frame: frame.clone(),
            buffer,
            region: data.region,
            output,
            with_damage,
        });
    }

    fn destroyed(state: &mut Self, _client: smithay::reexports::wayland_server::backend::ClientId, frame: &ZwlrScreencopyFrameV1, _data: &FrameData) {
        state.screencopy_pending.retain(|p| &p.frame != frame);
    }
}

impl CompState {
    /// Full-output capture region for the output the client named, in that
    /// output's own physical pixels (captures read back a single output's
    /// framebuffer, so coordinates are output-local, not global).
    fn output_capture_region(&self, output: &WlOutput) -> Rectangle<i32, Physical> {
        let size: Size<i32, Physical> = self
            .output_for_wl(output)
            .and_then(|e| e.output.current_mode())
            .map(|m| m.size)
            .unwrap_or_default();
        Rectangle::from_size(size)
    }
}

/// Services every queued capture against the framebuffer that was just
/// rendered, then answers each client with `flags` + `ready` (or `failed`).
///
/// Called from inside both backends' render passes, while `framebuffer` is
/// still bound and holds the current frame.
/// Rejects queued captures outright. Used while the session is locked: the
/// render pass draws only the lock surface, so there is no frame a capture
/// could legitimately be served from. Failing immediately is both correct
/// (the client gets an answer instead of hanging) and safer than leaving
/// requests queued, which would otherwise all fire against the first
/// *unlocked* frame after the screen is unlocked.
pub fn fail_pending(pending: Vec<PendingCapture>) {
    for capture in pending {
        if capture.frame.is_alive() {
            capture.frame.failed();
        }
    }
}

/// Takes the queue by value rather than `&mut CompState` so the udev
/// backend can call it while already holding a `&mut` borrow of the
/// `UdevOutput` that owns its renderer. Callers drain
/// `CompState::screencopy_pending` before binding the renderer.
pub fn service_pending<R>(pending: Vec<PendingCapture>, renderer: &mut R, framebuffer: &R::Framebuffer<'_>)
where
    R: Renderer + ExportMem,
{
    for pending in pending {
        if !pending.frame.is_alive() {
            continue;
        }
        match copy_region(renderer, framebuffer, pending.region, &pending.buffer) {
            Ok(()) => {
                // No `y_invert`: `copy_framebuffer` hands back rows already
                // in top-down order for both renderers used here (verified
                // against a real `grim` capture - an inverted image is the
                // immediately visible symptom if this is ever wrong).
                pending.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
                if pending.with_damage {
                    pending.frame.damage(0, 0, pending.region.size.w as u32, pending.region.size.h as u32);
                }
                let now = UNIX_EPOCH.elapsed().unwrap_or_default();
                let secs = now.as_secs();
                pending.frame.ready((secs >> 32) as u32, secs as u32, now.subsec_nanos());
            }
            Err(e) => {
                log::warn!("screencopy: capture failed: {e}");
                pending.frame.failed();
            }
        }
    }
}

fn copy_region<R>(
    renderer: &mut R,
    framebuffer: &R::Framebuffer<'_>,
    region: Rectangle<i32, Physical>,
    buffer: &WlBuffer,
) -> Result<(), String>
where
    R: Renderer + ExportMem,
{
    // `copy_framebuffer` works in buffer coordinates; with no output
    // transform or fractional scale in play (see this backend's single
    // `Output`), those are the same numbers as the physical ones.
    let src: Rectangle<i32, BufferCoord> = Rectangle::new((region.loc.x, region.loc.y).into(), (region.size.w, region.size.h).into());
    let mapping = renderer
        .copy_framebuffer(framebuffer, src, CAPTURE_FOURCC)
        .map_err(|e| format!("copy_framebuffer: {e}"))?;
    let pixels = renderer.map_texture(&mapping).map_err(|e| format!("map_texture: {e}"))?;

    let stride = region.size.w as usize * BYTES_PER_PIXEL as usize;
    let needed = stride * region.size.h as usize;
    if pixels.len() < needed {
        return Err(format!("readback produced {} bytes, need {}", pixels.len(), needed));
    }

    with_buffer_contents_mut(buffer, |ptr, len, _spec| {
        if len < needed {
            return Err(format!("client buffer holds {len} bytes, need {needed}"));
        }
        // SAFETY: `with_buffer_contents_mut` guarantees `ptr` is valid for
        // `len` bytes for the duration of this closure, and `needed <= len`
        // was just checked. Source and destination are distinct mappings.
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr, needed);
        }
        Ok(())
    })
    .map_err(|e| format!("client buffer not accessible: {e}"))?
}
