//! CPU box blur for the native lock screen's captured background
//! (`native_lock.rs`). Not a true Gaussian blur - no blur primitive is
//! available without a GPU shader (the udev backend's `PixmanRenderer` is
//! software-only), the same "approximate falloff over true blur" tradeoff
//! `decoration::shadow_bitmap` already accepts for drop shadows. Three
//! box-blur passes approximate a Gaussian closely enough to read as a
//! real blur rather than an obviously-boxy one, a standard trick (Adobe's
//! own CSS `filter: blur()` polyfills use the same three-pass
//! approximation).
//!
//! Runs once, at lock time, on the just-captured screen content - not
//! per frame - so a straightforward `O(pixels)` sliding-window
//! implementation (not `O(pixels * radius)`, which would make a large
//! radius on a real screen resolution noticeably slow even as a one-time
//! cost) is what actually matters here, not raw simplicity.

/// Blurs `buf` (a BGRA8/XRGB8888 pixel buffer, 4 bytes per pixel, alpha
/// byte untouched either way) in place. `radius` of `0` is a deliberate
/// no-op, not clamped up to some minimum - see `LockConfig::blur_radius`'s
/// own doc comment.
pub(crate) fn box_blur(buf: &mut [u8], width: usize, height: usize, radius: u32) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let radius = radius as usize;
    // Three passes, alternating axis, approximates a Gaussian kernel.
    for _ in 0..3 {
        blur_horizontal(buf, width, height, radius);
        blur_vertical(buf, width, height, radius);
    }
}

/// Sliding-window box blur along each row: the window's running sum is
/// updated by removing the pixel that just left it and adding the one
/// that just entered, rather than re-summing `2 * radius + 1` pixels at
/// every single output pixel.
fn blur_horizontal(buf: &mut [u8], width: usize, height: usize, radius: usize) {
    let mut row = vec![0u8; width * 4];
    for y in 0..height {
        let row_start = y * width * 4;
        row.copy_from_slice(&buf[row_start..row_start + width * 4]);
        let mut sum = [0i64; 3];
        let mut count = 0i64;
        for x in 0..=radius.min(width.saturating_sub(1)) {
            add_pixel(&row, x, &mut sum, &mut count);
        }
        for x in 0..width {
            write_average(buf, row_start + x * 4, &sum, count);
            let leaving = x as isize - radius as isize;
            if leaving >= 0 {
                remove_pixel(&row, leaving as usize, &mut sum, &mut count);
            }
            let entering = x + radius + 1;
            if entering < width {
                add_pixel(&row, entering, &mut sum, &mut count);
            }
        }
    }
}

/// Same sliding-window technique as [`blur_horizontal`], transposed --
/// copies each column into a contiguous scratch buffer first so the
/// window updates are still sequential memory access, not a
/// `width * 4`-strided walk on every add/remove.
fn blur_vertical(buf: &mut [u8], width: usize, height: usize, radius: usize) {
    let stride = width * 4;
    let mut col = vec![0u8; height * 4];
    for x in 0..width {
        for y in 0..height {
            let idx = y * stride + x * 4;
            col[y * 4..y * 4 + 4].copy_from_slice(&buf[idx..idx + 4]);
        }
        let mut sum = [0i64; 3];
        let mut count = 0i64;
        for y in 0..=radius.min(height.saturating_sub(1)) {
            add_pixel(&col, y, &mut sum, &mut count);
        }
        for y in 0..height {
            write_average(buf, y * stride + x * 4, &sum, count);
            let leaving = y as isize - radius as isize;
            if leaving >= 0 {
                remove_pixel(&col, leaving as usize, &mut sum, &mut count);
            }
            let entering = y + radius + 1;
            if entering < height {
                add_pixel(&col, entering, &mut sum, &mut count);
            }
        }
    }
}

fn add_pixel(buf: &[u8], index: usize, sum: &mut [i64; 3], count: &mut i64) {
    let i = index * 4;
    sum[0] += buf[i] as i64;
    sum[1] += buf[i + 1] as i64;
    sum[2] += buf[i + 2] as i64;
    *count += 1;
}

fn remove_pixel(buf: &[u8], index: usize, sum: &mut [i64; 3], count: &mut i64) {
    let i = index * 4;
    sum[0] -= buf[i] as i64;
    sum[1] -= buf[i + 1] as i64;
    sum[2] -= buf[i + 2] as i64;
    *count -= 1;
}

/// Writes the window's current average into `buf` at byte offset `at`,
/// leaving the alpha byte (`at + 3`) untouched - the captured background
/// is always fully opaque, so there is nothing meaningful to blur there.
fn write_average(buf: &mut [u8], at: usize, sum: &[i64; 3], count: i64) {
    buf[at] = (sum[0] / count) as u8;
    buf[at + 1] = (sum[1] / count) as u8;
    buf[at + 2] = (sum[2] / count) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_radius_is_a_no_op() {
        let mut buf = vec![10, 20, 30, 255, 200, 100, 50, 255];
        let original = buf.clone();
        box_blur(&mut buf, 2, 1, 0);
        assert_eq!(buf, original);
    }

    #[test]
    fn a_uniform_image_is_unchanged_by_blurring() {
        // Blurring a flat color must not shift it - the clearest possible
        // regression check for an off-by-one in the sliding window (a
        // wrong window size would still average to *something*, but a
        // biased one, not the same flat color back).
        let (w, h) = (10, 10);
        let mut buf = vec![0u8; w * h * 4];
        for px in buf.chunks_exact_mut(4) {
            px.copy_from_slice(&[40, 80, 120, 255]);
        }
        box_blur(&mut buf, w, h, 3);
        for px in buf.chunks_exact(4) {
            assert_eq!(px, [40, 80, 120, 255]);
        }
    }

    #[test]
    fn alpha_is_never_touched() {
        let (w, h) = (4, 4);
        let mut buf = vec![0u8; w * h * 4];
        for (i, px) in buf.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[i as u8, i as u8, i as u8, (i * 17) as u8]);
        }
        box_blur(&mut buf, w, h, 2);
        for (i, px) in buf.chunks_exact(4).enumerate() {
            assert_eq!(px[3], (i * 17) as u8, "alpha byte at pixel {i} must survive unchanged");
        }
    }

    #[test]
    fn a_bright_spot_spreads_into_its_dark_neighbours_with_falloff() {
        // Large enough that a genuinely-far corner exists even after three
        // cascaded passes (each pass spreads influence roughly `radius`
        // further, so three passes of radius 2 reach noticeably past 2
        // pixels out - a smaller canvas made this assert something the
        // algorithm was never expected to guarantee).
        let (w, h) = (41, 41);
        let mut buf = vec![0u8; w * h * 4];
        for px in buf.chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 0, 0, 255]);
        }
        let (cx, cy) = (20, 20);
        let center = (cy * w + cx) * 4;
        buf[center..center + 3].copy_from_slice(&[255, 255, 255]);

        box_blur(&mut buf, w, h, 2);

        let near = ((cy * w) + cx + 1) * 4; // immediately right of center
        let far = ((cy * w) + cx + 10) * 4; // well outside the blur's reach
        let corner = 0; // top-left, far from the bright spot on both axes
        assert!(buf[near] > 0, "a pixel next to the bright spot must pick up some of its brightness after blurring");
        assert!(buf[near] > buf[far], "brightness must fall off with distance, not spread flatly to the whole image");
        assert_eq!(buf[corner], 0, "a pixel far outside the blur's reach must stay untouched");
    }

    #[test]
    fn zero_sized_buffer_does_not_panic() {
        let mut buf: Vec<u8> = Vec::new();
        box_blur(&mut buf, 0, 0, 5);
    }
}
