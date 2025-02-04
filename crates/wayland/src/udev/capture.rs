//! Off-screen render of a workspace that isn't necessarily the one
//! currently on screen - `srd capture workspace <id> <path>`, drained
//! from `WindowManager::drain_capture_requests` on every poll. See
//! `srdwm_core::CaptureRequest`'s own doc comment for why this exists at
//! all: `wlr-screencopy` (`crates/wayland/src/screencopy.rs`, and `grim`)
//! can only ever see what an output is actually presenting, and a
//! workspace switcher's thumbnail needs exactly the opposite - a
//! workspace that, most of the time, is *not* the one presented.
//!
//! Deliberately simple, not a small reimplementation of
//! `render_udev_frame`: only window content is drawn, no borders,
//! shadows, titlebars, cursor or layer-shell surfaces - every consumer
//! this was built for (a workspace-switcher tile) draws those tiny, where
//! that detail is imperceptible, and skipping them keeps this from needing
//! to duplicate that function's animation/occlusion bookkeeping. Always
//! renders at the target monitor's native resolution and downscales
//! afterward if a smaller size was requested, rather than trying to get
//! smithay's fractional-output-scale rendering path exactly right for a
//! target with no real `Output` behind it.

use super::*;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::utils::{Buffer as BufferCoord, Transform};

impl CompState {
    /// Services every capture request queued since the last poll. Takes
    /// the `Vec` by value for the same reason `screencopy::service_pending`
    /// does: the renderer this needs lives behind `self.udev`'s own
    /// mutable borrow, so the request list has to be lifted out of
    /// `self.wm` before that borrow starts.
    pub(crate) fn service_capture_requests(&mut self, requests: Vec<srdwm_core::CaptureRequest>) {
        for req in requests {
            if let Err(e) = self.capture_workspace(&req) {
                log::warn!("capture: workspace {} -> {}: {e}", req.workspace, req.path);
            }
        }
    }

    fn capture_workspace(&mut self, req: &srdwm_core::CaptureRequest) -> Result<(), String> {
        // The monitor a freshly-placed window on this workspace would land
        // on: workspaces aren't per-monitor in this compositor (a single
        // `current_workspace` is shared by every screen - see
        // `WindowManager`'s own field doc comment), so there's no single
        // "this workspace's monitor" to ask for; the primary one is the
        // same reasonable default `arrange_workspace` itself falls back to.
        let (origin, native): ((i32, i32), (u32, u32)) = {
            let wm = self.wm.borrow();
            let monitor = wm.monitors().iter().find(|m| m.primary).or_else(|| wm.monitors().first()).ok_or("no monitor to capture from")?;
            ((monitor.full_geometry.x, monitor.full_geometry.y), (monitor.full_geometry.width, monitor.full_geometry.height))
        };
        if native.0 == 0 || native.1 == 0 {
            return Err("monitor has zero size".to_string());
        }

        let ids = self.wm.borrow().window_ids_on_workspace_front_to_back(req.workspace);
        let mut elements: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();
        let Some(udev) = self.udev.as_mut() else { return Err("no udev backend".to_string()) };
        for id in ids {
            let Some(w) = self.id_to_window.get(&id) else { continue };
            let Some(surface) = crate::input::dwindow_wl_surface(w) else { continue };
            let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) else { continue };
            // Same `set_window_geometry` offset every other render path
            // subtracts (see `udev/render.rs`'s matching fix) - without
            // it, a CSD window's invisible shadow margin would show up as
            // a gap in the capture too.
            let content_offset = w.geometry().loc;
            let loc = (geom.x - origin.0 - content_offset.x, geom.y - origin.1 - content_offset.y);
            elements.extend(render_elements_from_surface_tree(&mut udev.renderer, &surface, loc, 1.0, 1.0, Kind::Unspecified));
        }

        let (nw, nh) = (native.0 as i32, native.1 as i32);
        let mut target = udev.renderer.create_buffer(Fourcc::Xrgb8888, (nw, nh).into()).map_err(|e| format!("create_buffer: {e}"))?;
        let mut framebuffer = udev.renderer.bind(&mut target).map_err(|e| format!("bind: {e}"))?;
        let mut tracker = OutputDamageTracker::new((nw, nh), 1.0, Transform::Normal);
        tracker
            .render_output(&mut udev.renderer, &mut framebuffer, 0, &elements, [0.0, 0.0, 0.0, 1.0])
            .map_err(|e| format!("render_output: {e:?}"))?;

        let region: Rectangle<i32, BufferCoord> = Rectangle::new((0, 0).into(), (nw, nh).into());
        let mapping = udev.renderer.copy_framebuffer(&framebuffer, region, Fourcc::Xrgb8888).map_err(|e| format!("copy_framebuffer: {e}"))?;
        let pixels = udev.renderer.map_texture(&mapping).map_err(|e| format!("map_texture: {e}"))?;

        write_ppm(pixels, native, req.size, &req.path)
    }
}

/// `pixels` is `Xrgb8888` - 4 bytes per pixel, little-endian, so byte
/// order in memory is B, G, R, X. PPM (`P6`) wants tightly-packed R, G, B
/// with no pad byte, hence the reorder rather than a straight `memcpy`.
/// Downscales with plain nearest-neighbor sampling when `target` is
/// smaller than `native` - a thumbnail has no need for anything more
/// expensive, and this avoids pulling in an image-scaling crate for one
/// call site.
fn write_ppm(pixels: &[u8], native: (u32, u32), target: Option<(u32, u32)>, path: &str) -> Result<(), String> {
    let (nw, nh) = native;
    let (tw, th) = target.unwrap_or(native);
    if tw == 0 || th == 0 {
        return Err("requested capture size is zero".to_string());
    }
    let src_stride = nw as usize * 4;
    let needed = src_stride * nh as usize;
    if pixels.len() < needed {
        return Err(format!("readback produced {} bytes, need {needed}", pixels.len()));
    }

    let mut rgb = Vec::with_capacity(tw as usize * th as usize * 3);
    for ty in 0..th {
        // `.min(nh - 1)`/`.min(nw - 1)`: guards the last row/column of a
        // downscale from ever reading one pixel past the source when an
        // integer ratio rounds up, not a real expectation of overflow.
        let sy = (ty as u64 * nh as u64 / th as u64).min(nh as u64 - 1) as usize;
        for tx in 0..tw {
            let sx = (tx as u64 * nw as u64 / tw as u64).min(nw as u64 - 1) as usize;
            let i = sy * src_stride + sx * 4;
            rgb.push(pixels[i + 2]); // R
            rgb.push(pixels[i + 1]); // G
            rgb.push(pixels[i]); // B
        }
    }

    let mut out = format!("P6\n{tw} {th}\n255\n").into_bytes();
    out.extend_from_slice(&rgb);
    // Written to a `.tmp` sibling and renamed into place: a reader (AGS's
    // wsPreview poller) racing a partial write is exactly the kind of
    // flicker/corruption a debounced, event-driven cache is supposed to
    // avoid - `rename` within the same directory is atomic, a plain
    // `write` never is.
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, &out).map_err(|e| format!("write {tmp}: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename to {path}: {e}"))
}
