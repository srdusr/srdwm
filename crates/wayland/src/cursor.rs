//! Mouse cursor rendering.
//!
//! The nested (winit) backend gets a cursor for free - the *host*
//! compositor draws one over srdwm's window - which is exactly why this was
//! missing for so long without being noticed. On a bare TTY nothing else is
//! drawing anything, so without this the pointer is simply invisible: you
//! can move it, click with it, and drag windows with it, but you cannot see
//! where it is. That makes the udev backend unusable as a real session.
//!
//! **Scope, stated plainly:** this draws one built-in arrow, always. It
//! honours `CursorImageStatus::Hidden` (so a client that hides the pointer
//! still gets its way), but it does *not* yet render a client's own cursor
//! surface or a named shape - an app asking for an I-beam or a resize arrow
//! still sees this arrow. That is a real limitation, and a visible one over
//! text fields; it is also strictly better than the previous behaviour of
//! drawing nothing at all.
//!
//! The built-in arrow is deliberate rather than loading an XCursor theme:
//! theme loading pulls in a dependency, needs a theme to actually be
//! installed, and has a search-path fallback story of its own. A cursor that
//! is always present beats a prettier one that sometimes isn't there - the
//! same reasoning as `decoration.rs`'s font fallback.

use smithay::utils::{Logical, Point};

/// Side length of the built-in cursor bitmap, in pixels.
pub(crate) const CURSOR_SIZE: i32 = 24;

/// A classic left-pointing arrow: white fill, black outline, with the
/// hotspot at (0, 0) - the tip.
///
/// Encoded as a small bitmap rather than drawn with geometry so the shape is
/// obvious and reviewable: `.` transparent, `#` black outline, `*` white
/// fill. 24 rows of 24 columns.
const ARROW: [&str; CURSOR_SIZE as usize] = [
    "#.......................",
    "##......................",
    "#*#.....................",
    "#**#....................",
    "#***#...................",
    "#****#..................",
    "#*****#.................",
    "#******#................",
    "#*******#...............",
    "#********#..............",
    "#*********#.............",
    "#**********#............",
    "#***********#...........",
    "#************#..........",
    "#*************#.........",
    "#******####### .........",
    "#***#**#................",
    "#**#.#**#...............",
    "#*#..#**#...............",
    "##....#**#..............",
    "#.....#**#..............",
    ".......###..............",
    "........................",
    "........................",
];

/// Rasterises the built-in arrow as premultiplied ARGB8888, the format
/// `MemoryRenderBuffer` expects.
pub(crate) fn arrow_bitmap() -> Vec<u8> {
    let mut buf = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    for (y, row) in ARROW.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if x >= CURSOR_SIZE as usize || y >= CURSOR_SIZE as usize {
                break;
            }
            // Premultiplied: opaque pixels only, so colour == colour * 1.
            let (b, g, r, a) = match ch {
                '#' => (0x00, 0x00, 0x00, 0xff),
                '*' => (0xff, 0xff, 0xff, 0xff),
                _ => continue,
            };
            let i = (y * CURSOR_SIZE as usize + x) * 4;
            buf[i] = b;
            buf[i + 1] = g;
            buf[i + 2] = r;
            buf[i + 3] = a;
        }
    }
    buf
}


/// Render elements for the pointer, to be drawn above everything else.
///
/// `pos` is in the global space and `origin` is the output's origin, since
/// each head renders in its own coordinate space.
///
/// Returns nothing when the cursor is hidden, or when the pointer is not
/// over this output - otherwise every monitor would draw its own copy.
pub(crate) fn render_elements<R>(
    status: &smithay::input::pointer::CursorImageStatus,
    buffer: &smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    renderer: &mut R,
    pos: Point<f64, Logical>,
    origin: Point<i32, Logical>,
    size: (i32, i32),
) -> Vec<smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>>
where
    R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
    use smithay::backend::renderer::element::Kind;
    use smithay::input::pointer::CursorImageStatus;

    if matches!(status, CursorImageStatus::Hidden) {
        return Vec::new();
    }
    // Only the output the pointer is actually on draws it.
    let local = (pos.x as i32 - origin.x, pos.y as i32 - origin.y);
    if local.0 < 0 || local.1 < 0 || local.0 >= size.0 || local.1 >= size.1 {
        return Vec::new();
    }

    // Built-in arrow: hotspot is the tip, so no offset.
    let at = (local.0 as f64, local.1 as f64);
    match MemoryRenderBufferRenderElement::from_buffer(renderer, at, buffer, None, None, None, Kind::Cursor) {
        Ok(e) => vec![e],
        Err(e) => {
            log::warn!("cursor: failed to import bitmap: {e}");
            Vec::new()
        }
    }
}

/// The built-in arrow as an uploadable buffer. Built once at startup rather
/// than per frame - the bitmap never changes.
pub(crate) fn make_buffer() -> smithay::backend::renderer::element::memory::MemoryRenderBuffer {
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
    use smithay::utils::Transform;
    MemoryRenderBuffer::from_slice(&arrow_bitmap(), Fourcc::Argb8888, (CURSOR_SIZE, CURSOR_SIZE), 1, Transform::Normal, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_is_the_expected_size_and_has_an_opaque_tip() {
        let buf = arrow_bitmap();
        assert_eq!(buf.len(), (CURSOR_SIZE * CURSOR_SIZE * 4) as usize);
        // The hotspot pixel (0,0) is the arrow's tip and must be visible,
        // otherwise the cursor appears offset from where clicks land.
        assert_eq!(buf[3], 0xff, "tip pixel must be opaque");
    }

    #[test]
    fn arrow_has_both_outline_and_fill() {
        let buf = arrow_bitmap();
        let mut black = 0;
        let mut white = 0;
        for px in buf.chunks_exact(4) {
            if px[3] == 0 {
                continue;
            }
            if px[0] == 0 && px[1] == 0 && px[2] == 0 {
                black += 1;
            } else {
                white += 1;
            }
        }
        assert!(black > 20, "expected a black outline, got {black} px");
        assert!(white > 40, "expected a white fill, got {white} px");
    }
}
