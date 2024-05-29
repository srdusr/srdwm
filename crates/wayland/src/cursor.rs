//! Mouse cursor rendering.
//!
//! The nested (winit) backend gets a cursor for free - the *host*
//! compositor draws one over srdwm's window - which is exactly why this was
//! missing for so long without being noticed. On a bare TTY nothing else is
//! drawing anything, so without this the pointer is simply invisible: you
//! can move it, click with it, and drag windows with it, but you cannot see
//! where it is. That makes the udev backend unusable as a real session.
//!
//! Two sources, in priority order:
//!
//! 1. **The client's own cursor surface** (`CursorImageStatus::Surface`) --
//!    a terminal's I-beam, a browser's hand, an app's resize arrows. Drawn
//!    from its surface tree, offset by the hotspot the client declared.
//! 2. **A built-in arrow**, for when no client has set an image (over
//!    srdwm's own decorations and the desktop) or asked for a named shape
//!    we have no art for.
//!
//! `CursorImageStatus::Hidden` is honoured, so a client that hides the
//! pointer still gets its way. Named shapes (`CursorIcon::Text` etc.) fall
//! back to the arrow rather than being drawn as the requested shape - most
//! toolkits set a surface rather than a name, so this is rarely visible.
//!
//! The built-in arrow is deliberate rather than loading an XCursor theme:
//! theme loading pulls in a dependency, needs a theme to actually be
//! installed, and has a search-path fallback story of its own. A cursor that
//! is always present beats a prettier one that sometimes isn't there - the
//! same reasoning as `decoration.rs`'s font fallback.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::utils::{Logical, Point};

use crate::elements::OverlayElement;

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


/// One cursor render element, whatever the source.
///
/// Client cursor surfaces and the built-in bitmap are different element
/// types, so they're unified here rather than forcing both through one.
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
) -> Vec<OverlayElement<R>>
where
    R: smithay::backend::renderer::Renderer
        + smithay::backend::renderer::ImportAll
        + smithay::backend::renderer::ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
    use smithay::backend::renderer::element::Kind;
    use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
    use smithay::wayland::compositor::with_states;

    if matches!(status, CursorImageStatus::Hidden) {
        return Vec::new();
    }
    // Only the output the pointer is actually on draws it.
    let local = (pos.x as i32 - origin.x, pos.y as i32 - origin.y);
    if local.0 < 0 || local.1 < 0 || local.0 >= size.0 || local.1 >= size.1 {
        return Vec::new();
    }

    if let CursorImageStatus::Surface(surface) = status {
        // The client picked an image. Its hotspot is the point *inside* that
        // image which tracks the pointer, so the surface is drawn offset by
        // it - without this the image sits down-right of where clicks land.
        let hotspot = with_states(surface, |states| {
            states
                .data_map
                .get::<CursorImageSurfaceData>()
                .map(|d| d.lock().unwrap().hotspot)
                .unwrap_or_default()
        });
        let at = (local.0 - hotspot.x, local.1 - hotspot.y);
        return render_elements_from_surface_tree(renderer, surface, at, 1.0, 1.0, Kind::Cursor);
    }

    // No client image (or a named shape we don't have art for): the
    // built-in arrow, whose hotspot is its tip, so no offset.
    let at = (local.0 as f64, local.1 as f64);
    match MemoryRenderBufferRenderElement::from_buffer(renderer, at, buffer, None, None, None, Kind::Cursor) {
        Ok(e) => vec![OverlayElement::Memory(e)],
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
