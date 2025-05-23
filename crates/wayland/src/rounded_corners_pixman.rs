//! Real rounded corners on a window's own client content - udev/Pixman
//! backend. `rounded_corners.rs` covers the GLES/winit backend with a
//! fragment shader; `PixmanRenderer` is software-only and has no shader
//! stage to hook that into at all (`PixmanFrame::render_texture_from_to`'s
//! mask is a hardcoded flat alpha, and its destination image is private to
//! smithay's own module - no public hook for a custom mask picture).
//!
//! The technique here: render the window's *entire* surface tree (root
//! plus every subsurface, exactly what the ordinary unmasked path already
//! draws) into a private off-screen buffer, read that back as plain BGRA8
//! bytes, and punch the four corner holes into *those* - the composited
//! result, not any one client buffer - before handing it to
//! `MemoryRenderBuffer`, the same type and render-element path already
//! used for the titlebar/border/shadow bitmaps.
//!
//! A previous version of this instead tried to identify *which one*
//! surface in the tree held "the real content" (a root-plus-one-child
//! GTK4/WebRender pattern, confirmed live against Firefox and Chrome) and
//! masked that single client buffer directly, skipping everything else in
//! the tree. That was cheaper - no extra render pass - but structurally
//! fragile: it assumed the *rest* of the tree (whatever the chosen surface
//! didn't cover) was always invisible padding, true for Chrome's own
//! shadow-margin inset but false for Firefox, whose tab strip/title row is
//! painted on the *root* surface, outside its own content child. The
//! moment that surface-picking heuristic got permissive enough to actually
//! mask Firefox's real, common case, it started *silently deleting
//! Firefox's own tab strip* - reported live as "Firefox's titlebar turned
//! invisible", confirmed by toggling `general.rounded_corners` off, which
//! brought it straight back. Rendering the whole tree and masking the
//! *output* instead of guessing which *input* is real sidesteps the whole
//! question - the same reason a GPU shader-based compositor (niri,
//! cosmic-comp) never has this class of bug at all: by the time its own
//! shader runs, the subsurface tree is already flattened into one texture,
//! so there is nothing left to misidentify.
//!
//! Only `wl_shm`/dmabuf-agnostic now - unlike the old per-buffer read,
//! this never touches a client's own buffer format at all, only the
//! renderer's own composited output, so the format/transform restrictions
//! the previous version needed (`Argb8888`/`Xrgb8888` only, no dmabuf
//! without a dedicated read path, `Transform::Normal` only) no longer
//! apply - whatever the renderer can already draw (which is everything it
//! draws for the ordinary unmasked path too), this can mask.
//!
//! Cost, and why this stays default-off on this backend (`general.
//! rounded_corners`, see `WindowManager::rounded_corners_enabled`'s doc
//! comment): a full extra off-screen render pass (allocate a buffer, draw
//! the tree into it, read the result back) on every real content change,
//! not just a raw memory copy the old approach needed - strictly more
//! expensive per rebuild than before, though still gated by the same
//! `CompState::content_epoch` cache as before, so an idle/static window
//! still costs nothing per frame, only per genuine repaint.

use crate::rounded_corners::RoundedCorners;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer as BufferCoord, Rectangle, Transform};

/// Renders `surface`'s whole subsurface tree into a private `size`-sized
/// off-screen buffer - `loc` is the tree's own root-surface-relative
/// origin to render at, exactly like `render_elements_from_surface_tree`'s
/// own `location` parameter elsewhere in this codebase (`udev/capture.rs`);
/// the caller passes the *negated* `content_offset`
/// (`dwindow.geometry().loc`, the client's own declared shadow-margin
/// inset) so the buffer's own `(0, 0)` lands exactly on the window's real
/// visible content top-left, the same correction every other render path
/// in this compositor already applies (see `udev/render.rs`'s own `pos`
/// computation) - then punches the four rounded-corner holes into the
/// result. Returns tightly-packed BGRA8 bytes (`size.0 * size.1 * 4`),
/// ready for `MemoryRenderBuffer::from_slice`, or `None` if the off-screen
/// render itself failed (a genuine renderer error, not "this window isn't
/// shaped right for masking" - there is no such restriction anymore).
///
/// `radius` is already in the same units as `size` (this compositor's
/// outputs are always scale `1.0`, per `WindowManager::rounded_corners_
/// enabled`'s own doc comment, so there is no separate buffer-scale
/// factor to fold in here the way the old per-client-buffer read needed).
pub(crate) fn masked_content_buffer(renderer: &mut PixmanRenderer, surface: &WlSurface, loc: (i32, i32), size: (i32, i32), radius: f32, corners: RoundedCorners) -> Option<Vec<u8>> {
    let (w, h) = size;
    if w <= 0 || h <= 0 {
        return None;
    }
    // Transparent clear (not opaque black, unlike `udev/capture.rs`'s own
    // off-screen render): this buffer holds only the window's own content,
    // with nothing behind it to composite against yet - any area the
    // surface tree doesn't actually draw into (a subsurface smaller than
    // its own declared geometry, say) needs to stay real, punch-through
    // transparency so the border/desktop already drawn underneath on the
    // real output shows through there, not a solid black patch.
    let elements = render_elements_from_surface_tree::<_, crate::elements::OverlayElement<PixmanRenderer>>(renderer, surface, loc, 1.0, 1.0, Kind::Unspecified);
    let mut target = match renderer.create_buffer(Fourcc::Argb8888, (w, h).into()) {
        Ok(t) => t,
        Err(e) => {
            log::debug!("rounded_corners_pixman: masked_content_buffer: create_buffer failed ({e:?}) - giving up unmasked");
            return None;
        }
    };
    let mut framebuffer = match renderer.bind(&mut target) {
        Ok(fb) => fb,
        Err(e) => {
            log::debug!("rounded_corners_pixman: masked_content_buffer: bind failed ({e:?}) - giving up unmasked");
            return None;
        }
    };
    let mut tracker = OutputDamageTracker::new((w, h), 1.0, Transform::Normal);
    if let Err(e) = tracker.render_output(renderer, &mut framebuffer, 0, &elements, [0.0, 0.0, 0.0, 0.0]) {
        log::debug!("rounded_corners_pixman: masked_content_buffer: render_output failed ({e:?}) - giving up unmasked");
        return None;
    }
    let region: Rectangle<i32, BufferCoord> = Rectangle::new((0, 0).into(), (w, h).into());
    let mapping = match renderer.copy_framebuffer(&framebuffer, region, Fourcc::Argb8888) {
        Ok(m) => m,
        Err(e) => {
            log::debug!("rounded_corners_pixman: masked_content_buffer: copy_framebuffer failed ({e:?}) - giving up unmasked");
            return None;
        }
    };
    let pixels = match renderer.map_texture(&mapping) {
        Ok(p) => p,
        Err(e) => {
            log::debug!("rounded_corners_pixman: masked_content_buffer: map_texture failed ({e:?}) - giving up unmasked");
            return None;
        }
    };
    let mut out = pixels.to_vec();
    let radius_px = radius.min(w as f32 / 2.0).min(h as f32 / 2.0);
    apply_corner_mask(&mut out, w, h, w * 4, radius_px, corners);
    Some(out)
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
pub(crate) fn apply_corner_mask(buf: &mut [u8], w: i32, h: i32, stride: i32, radius: f32, corners: RoundedCorners) {
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
