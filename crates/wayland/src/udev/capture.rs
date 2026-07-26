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
//! `render_udev_frame`: no borders, shadows, titlebars or cursor --
//! every consumer this was built for (a workspace-switcher tile) draws
//! those tiny, where that detail is imperceptible, and skipping them
//! keeps this from needing to duplicate that function's animation/
//! occlusion bookkeeping. The background/bottom layer-shell surfaces
//! (the wallpaper) *are* included, unlike the rest of that list - a
//! capture with no windows on it and no wallpaper either is
//! indistinguishable from broken, and was reported live as exactly that:
//! "why does current workspace show black background" once measured
//! against a real screenshot of the same moment (mean luminance ~0.5 vs.
//! this capture's own ~0.03, i.e. genuinely near-black, not just "looks
//! dark on this monitor"). An inactive workspace with literally no
//! windows placed on it rendered *exactly* black (mean and variance both
//! zero) for the same reason - there was nothing else in the frame at
//! all to show. Always renders at the target monitor's native resolution
//! and downscales afterward if a smaller size was requested, rather than
//! trying to get smithay's fractional-output-scale rendering path
//! exactly right for a target with no real `Output` behind it.

use super::*;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::utils::{Buffer as BufferCoord, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

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
        let Some(udev) = self.udev.as_mut() else { return Err("no udev backend".to_string()) };
        let mut elements: Vec<crate::elements::OverlayElement<PixmanRenderer>> = Vec::new();
        for id in ids {
            // Matches the render loops: a capture must not show a frame the
            // screen does not (see `window_has_content`).
            if !Self::has_content(&self.awaiting_first_buffer, id) {
                continue;
            }
            let Some(w) = self.id_to_window.get(&id) else { continue };
            let Some(surface) = crate::input::dwindow_wl_surface(w) else { continue };
            let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) else { continue };
            // Same `set_window_geometry` offset every other render path
            // subtracts (see `udev/render.rs`'s matching fix) - without
            // it, a CSD window's invisible shadow margin would show up as
            // a gap in the capture too.
            let content_offset = w.geometry().loc;
            let loc = (geom.x - origin.0 - content_offset.x, geom.y - origin.1 - content_offset.y);
            elements.extend(render_elements_from_surface_tree::<_, crate::elements::OverlayElement<PixmanRenderer>>(
                &mut udev.renderer,
                &surface,
                loc,
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }
        // Background/bottom layer-shell (the wallpaper) last - bottommost,
        // matching `render_udev_frame`'s own ordering convention (see that
        // function's matching comment). The real output behind whichever
        // monitor `origin`/`native` came from, matched by location; missing
        // entirely (an output that vanished between resolving `origin`
        // above and here, a narrow race) just means no wallpaper in this
        // one capture, not a hard failure - windows above still render.
        if let Some(head) = udev.heads.iter().find(|h| h.location == Point::from(origin)) {
            elements.extend(crate::elements::output_layer_elements(&mut udev.renderer, &head.output, |layer| {
                matches!(layer, Layer::Background | Layer::Bottom)
            }));
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

/// Encodes packed RGB to whatever the destination's extension asks for.
///
/// PPM was the only format this ever wrote, which made the capture
/// unreadable to its actual consumers: a shell drawing thumbnails decodes
/// PNG/JPEG/WebP and not PPM, so the file was written successfully,
/// returned successfully, and then silently not drawn. The render itself
/// was never the problem - only the container.
///
/// `.ppm` still produces PPM, so any existing caller keeps working;
/// anything else is chosen by extension, defaulting to PNG when the
/// extension is unfamiliar. PNG is the safe default: it is lossless and
/// universally decodable, and at thumbnail sizes the size difference
/// against JPEG is tens of kilobytes.
fn encode_capture(rgb: &[u8], width: u32, height: u32, path: &str) -> Result<Vec<u8>, String> {
    let extension = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if extension == "ppm" {
        let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
        out.extend_from_slice(rgb);
        return Ok(out);
    }
    let format = match extension.as_str() {
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        _ => image::ImageFormat::Png,
    };
    let buffer = image::RgbImage::from_raw(width, height, rgb.to_vec()).ok_or_else(|| format!("capture buffer is not {width}x{height} RGB"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(buffer).write_to(&mut out, format).map_err(|e| format!("encode {extension}: {e}"))?;
    Ok(out.into_inner())
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

    let out = encode_capture(&rgb, tw, th, path)?;
    // Written to a `.tmp` sibling and renamed into place: a reader (AGS's
    // wsPreview poller) racing a partial write is exactly the kind of
    // flicker/corruption a debounced, event-driven cache is supposed to
    // avoid - `rename` within the same directory is atomic, a plain
    // `write` never is.
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, &out).map_err(|e| format!("write {tmp}: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename to {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::encode_capture;

    fn rgb(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| [(i % 251) as u8, 0x40, 0x80]).collect()
    }

    #[test]
    fn the_extension_picks_the_container() {
        // Checked by magic bytes rather than by trusting the call: the whole
        // point is that the file a consumer opens is the format it expects.
        let px = rgb(8, 4);
        assert!(encode_capture(&px, 8, 4, "/tmp/x.ppm").unwrap().starts_with(b"P6"), "ppm");
        assert!(encode_capture(&px, 8, 4, "/tmp/x.png").unwrap().starts_with(&[0x89, b'P', b'N', b'G']), "png");
        assert!(encode_capture(&px, 8, 4, "/tmp/x.jpg").unwrap().starts_with(&[0xff, 0xd8]), "jpg");
        assert!(encode_capture(&px, 8, 4, "/tmp/x.jpeg").unwrap().starts_with(&[0xff, 0xd8]), "jpeg");
    }

    #[test]
    fn an_unfamiliar_extension_falls_back_to_png_rather_than_failing() {
        let px = rgb(4, 4);
        let out = encode_capture(&px, 4, 4, "/tmp/thumb.thumbnail").unwrap();
        assert!(out.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn a_buffer_that_does_not_match_the_size_is_an_error_not_a_panic() {
        assert!(encode_capture(&rgb(4, 4), 8, 8, "/tmp/x.png").is_err());
    }

    #[test]
    fn the_encoded_image_round_trips_at_the_requested_size() {
        let out = encode_capture(&rgb(9, 5), 9, 5, "/tmp/x.png").unwrap();
        let decoded = image::load_from_memory(&out).expect("our own png must decode");
        assert_eq!((decoded.width(), decoded.height()), (9, 5));
    }
}
