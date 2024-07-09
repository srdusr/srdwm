//! Real rounded corners on a window's own client content - udev/Pixman
//! backend. `rounded_corners.rs` covers the GLES/winit backend with a
//! fragment shader; `PixmanRenderer` is software-only and has no shader
//! stage to hook that into at all (`PixmanFrame::render_texture_from_to`'s
//! mask is a hardcoded flat alpha, and its destination image is private to
//! smithay's own module - no public hook for a custom mask picture).
//!
//! The technique here instead bakes the mask into a *copy* of the client's
//! own pixel data before it ever reaches the normal compositing path: read
//! the surface's committed `wl_shm` buffer, punch premultiplied-alpha holes
//! (this codebase's existing BGRA convention - see `decoration::
//! shadow_bitmap`'s doc comment) into the four corner regions, and hand the
//! result to `MemoryRenderBuffer` - the exact same type and render-element
//! path already used for the titlebar/border/shadow bitmaps. Rendering it
//! through the ordinary unmasked `render_texture_from_to` is what makes the
//! corners actually disappear: a premultiplied-zero source pixel there
//! contributes nothing, leaving whatever was already drawn underneath (the
//! desktop, or another window) showing through - a real cutout, not a
//! flat-colour patch.
//!
//! Deliberately narrow scope, same as the GLES version: only a window's
//! *main* surface (no subsurfaces), and only the two common `wl_shm`
//! formats this compositor's own bitmaps already use (`Argb8888`/
//! `Xrgb8888`) - anything else, a non-`wl_shm` buffer (dmabuf, a GL
//! client), or a non-identity buffer transform falls back to `None`, which
//! the caller treats as "render this window's content unrounded" rather
//! than an error.
//!
//! Cost, and why this stays default-off on this backend (`general.
//! rounded_corners`, see `WindowManager::rounded_corners_enabled`'s doc
//! comment): unlike the GLES shader, which the GPU evaluates once per pixel
//! at zero extra CPU cost, this masks a full copy of the surface's pixel
//! data on the CPU. The mask math itself only touches the four small
//! corner boxes, but producing a tightly-packed buffer `MemoryRenderBuffer::
//! from_slice` accepts (it asserts a `width * 4` stride; a client's own SHM
//! stride is often larger, padded for alignment) means copying the whole
//! buffer row by row regardless. The caller is expected to cache the
//! result and only call this again when the surface's content has actually
//! changed (see `CompState::content_epoch`), so the real per-frame cost for
//! idle/static windows is nothing - but a constantly-repainting client
//! (video, a terminal under heavy scrollback) pays this on every commit for
//! as long the feature stays on, which is exactly the untested-on-real-
//! hardware cost the opt-in default exists to avoid forcing on anyone.

use crate::rounded_corners::RoundedCorners;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::utils::{with_renderer_surface_state, RendererSurfaceState};
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Transform;
use smithay::wayland::shm::{with_buffer_contents, BufferData};

/// Builds a rounded-corner-masked copy of `surface`'s own committed content,
/// or `None` if that isn't possible right now - see this module's doc
/// comment for every case that falls back rather than erroring. `radius` is
/// in the same logical-pixel units as `decoration::CORNER_RADIUS`; scaled
/// up to buffer pixels internally using the surface's own buffer scale.
pub(crate) fn masked_content_buffer(surface: &WlSurface, radius: f32, corners: RoundedCorners) -> Option<MemoryRenderBuffer> {
    let (buffer, scale, transform) = with_renderer_surface_state(surface, |state: &mut RendererSurfaceState| {
        let buffer = state.buffer()?.clone();
        Some((buffer, state.buffer_scale(), state.buffer_transform()))
    })??;
    // A rotated/flipped buffer would need the mask rotated with it; not
    // worth the extra math for a cosmetic, already-narrow-scope pass.
    if transform != Transform::Normal {
        return None;
    }
    let radius_px = radius * scale as f32;

    let (data, w, h) = with_buffer_contents(&buffer, move |ptr, len, data: BufferData| -> Option<(Vec<u8>, i32, i32)> {
        if !matches!(data.format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) {
            return None;
        }
        let (w, h, stride, offset) = (data.width, data.height, data.stride, data.offset);
        if w <= 0 || h <= 0 || stride <= 0 || offset < 0 {
            return None;
        }
        let needed = offset as usize + stride as usize * h as usize;
        if needed > len {
            return None;
        }
        // SAFETY: `pool.with_data` (inside `with_buffer_contents`) already
        // validated `ptr`/`len` cover the whole pool; `needed` above
        // re-checks this buffer's own slice sits inside that before a
        // single byte is read.
        let src = unsafe { std::slice::from_raw_parts(ptr.add(offset as usize), stride as usize * h as usize) };

        // Repack into a tight `width * 4` stride: `MemoryRenderBuffer::
        // from_slice` computes its own stride from `width` alone and
        // asserts the data matches it, so the source's (often padded) SHM
        // stride can't be handed through as-is.
        let row_bytes = w as usize * 4;
        let mut out = vec![0u8; row_bytes * h as usize];
        for y in 0..h as usize {
            let src_row = &src[y * stride as usize..y * stride as usize + row_bytes];
            out[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(src_row);
        }

        let radius_px = radius_px.min(w as f32 / 2.0).min(h as f32 / 2.0);
        apply_corner_mask(&mut out, w, h, row_bytes as i32, radius_px, corners);
        Some((out, w, h))
    })
    .ok()
    .flatten()?;

    Some(MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (w, h), scale, Transform::Normal, None))
}

/// Zeroes (fading over ~2px, matching `rounded_corners::FRAGMENT_SHADER`'s
/// `smoothstep`) every premultiplied BGRA pixel in `buf` that falls outside
/// the rounded rect described by `radius` at each of the four corners
/// `corners` selects. Only walks the four `radius`-sized corner boxes, not
/// the whole image - everywhere else the mask is exactly `1.0`, a no-op.
///
/// Uses the same `clamp`-then-`distance` construction as the GLSL version
/// rather than a per-corner mirrored center, so one formula handles all
/// four boxes correctly regardless of which edges of the image they sit
/// against.
fn apply_corner_mask(buf: &mut [u8], w: i32, h: i32, stride: i32, radius: f32, corners: RoundedCorners) {
    if radius < 1.0 || w <= 0 || h <= 0 {
        return;
    }
    let r = radius.ceil() as i32;
    let (wf, hf) = (w as f32, h as f32);
    let boxes = [
        (corners.top_left, 0, 0, r.min(w), r.min(h)),
        (corners.top_right, (w - r).max(0), 0, w, r.min(h)),
        (corners.bottom_left, 0, (h - r).max(0), r.min(w), h),
        (corners.bottom_right, (w - r).max(0), (h - r).max(0), w, h),
    ];
    for (enabled, x0, y0, x1, y1) in boxes {
        if !enabled {
            continue;
        }
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            let cy = py.clamp(radius, hf - radius);
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let cx = px.clamp(radius, wf - radius);
                let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                if dist <= radius - 1.0 {
                    continue;
                }
                let mask = 1.0 - smoothstep(radius - 1.0, radius + 1.0, dist);
                let i = (y * stride + x * 4) as usize;
                if i + 4 > buf.len() {
                    continue;
                }
                for b in &mut buf[i..i + 4] {
                    *b = (*b as f32 * mask).round() as u8;
                }
            }
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_mask_leaves_the_interior_untouched() {
        let (w, h) = (20, 20);
        let stride = w * 4;
        let mut buf = vec![200u8; (stride * h) as usize];
        apply_corner_mask(&mut buf, w, h, stride, 6.0, RoundedCorners::ALL);
        let center = ((h / 2 * stride) + (w / 2) * 4) as usize;
        assert_eq!(&buf[center..center + 4], &[200, 200, 200, 200]);
    }

    #[test]
    fn corner_mask_zeroes_the_outermost_corner_pixel() {
        let (w, h) = (20, 20);
        let stride = w * 4;
        let mut buf = vec![200u8; (stride * h) as usize];
        apply_corner_mask(&mut buf, w, h, stride, 6.0, RoundedCorners::ALL);
        // Pixel (0, 0) is `sqrt(2) * 6 ≈ 8.49` px from the corner's arc
        // center at (6, 6) - well past `radius + 1`, so fully masked.
        assert_eq!(&buf[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn bottom_only_leaves_the_top_corners_alone() {
        let (w, h) = (20, 20);
        let stride = w * 4;
        let mut buf = vec![200u8; (stride * h) as usize];
        apply_corner_mask(&mut buf, w, h, stride, 6.0, RoundedCorners::BOTTOM_ONLY);
        assert_eq!(&buf[0..4], &[200, 200, 200, 200]);
        let bottom_left = ((h - 1) * stride) as usize;
        assert_eq!(&buf[bottom_left..bottom_left + 4], &[0, 0, 0, 0]);
    }
}
