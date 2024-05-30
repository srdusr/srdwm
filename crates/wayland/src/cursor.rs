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
//! pointer still gets its way. Named shapes we have dedicated art for
//! (text entry, the four resize directions, crosshair, move, and the
//! pointing-hand link-hover shape) render as that shape; anything else
//! falls back to the arrow.
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
///
/// The tail (below the triangular head, past the horizontal shelf at row
/// 15) used to fork into two separate legs of visibly different widths --
/// the left one tapering down to a point like the rest of the shape, the
/// right one a constant-width block that never tapered at all, ending in
/// an abrupt flat stop. It rendered fine at a glance in a screenshot but
/// reads as lopsided/broken up close, exactly as reported live ("one side
/// is bigger than the other"). Replaced with a single triangular foot,
/// straight left edge continuing the head's, right edge tapering linearly
/// inward row by row down to a point - the same shape language the head
/// itself already uses, just mirrored.
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
    "#******#######..........",
    "#**********#............",
    "#********#..............",
    "#******#................",
    "#****#..................",
    "#**#....................",
    "#.......................",
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
    add_white_halo(&mut buf);
    buf
}

/// Adds a 1px opaque-white ring around every opaque pixel, on whichever
/// neighbouring pixels are still fully transparent.
///
/// Without this, the black outline drawn by the code above becomes
/// invisible over a dark background: confirmed live from a screenshot of
/// this exact arrow over a black terminal, where the outline had merged
/// completely into the background, leaving only a stark, edgeless white
/// silhouette. The resize/text shapes (drawn as plain opaque black lines,
/// no fill - see `set_px`) have the same problem more severely: solid
/// black on a dark window is close to invisible outright. A halo keeps
/// every shape readable against both light and dark content underneath it,
/// the same trick real cursor themes use - it is the faint white fringe
/// visible around an ordinary system arrow cursor.
///
/// Two-pass by construction: halo positions are collected against the
/// buffer's original opacity before any of them are written, so the ring
/// stays exactly 1px thick instead of dilating outward on itself.
fn add_white_halo(buf: &mut [u8]) {
    let is_opaque = |buf: &[u8], x: i32, y: i32| -> bool {
        if x < 0 || y < 0 || x >= CURSOR_SIZE || y >= CURSOR_SIZE {
            return false;
        }
        buf[((y * CURSOR_SIZE + x) * 4 + 3) as usize] != 0
    };
    let mut halo = Vec::new();
    for y in 0..CURSOR_SIZE {
        for x in 0..CURSOR_SIZE {
            if is_opaque(buf, x, y) {
                continue;
            }
            let touches_shape = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1), (x - 1, y - 1), (x + 1, y - 1), (x - 1, y + 1), (x + 1, y + 1)]
                .into_iter()
                .any(|(nx, ny)| is_opaque(buf, nx, ny));
            if touches_shape {
                halo.push((x, y));
            }
        }
    }
    for (x, y) in halo {
        let i = ((y * CURSOR_SIZE + x) * 4) as usize;
        buf[i] = 0xff;
        buf[i + 1] = 0xff;
        buf[i + 2] = 0xff;
        buf[i + 3] = 0xff;
    }
}


/// A small set of built-in cursor bitmaps beyond the default arrow, for the
/// named shapes a client requests most often: text entry, the four resize
/// directions, crosshair, move, and the pointing-hand hyperlink-hover
/// shape. Everything else (grab, wait, help, ...) still falls back to the
/// arrow - one arrow beats zero effort spent on a dozen rarely-seen icons,
/// but "I-beam over a text field", "double arrow at a window edge", and
/// "hand over a link" are common and immediately noticeable when wrong,
/// which is what made the cursor "always look the same regardless of
/// what's under it" worth fixing at all.
///
/// Built once at startup (`make_buffers`), same as the arrow.
#[derive(Clone)]
pub(crate) struct CursorBuffers {
    pub(crate) arrow: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) text: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) ns_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) ew_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) nesw_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) nwse_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) crosshair: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) move_icon: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) pointer: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
}

/// Sets one pixel to opaque white-on-black-outline isn't needed here (these
/// shapes are drawn solid black, unlike the arrow's outline+fill) --
/// straight opaque black, since these are thin enough that an outline
/// would just eat the whole shape.
fn set_px(buf: &mut [u8], x: i32, y: i32) {
    if x < 0 || y < 0 || x >= CURSOR_SIZE || y >= CURSOR_SIZE {
        return;
    }
    let i = ((y * CURSOR_SIZE + x) * 4) as usize;
    buf[i] = 0x00;
    buf[i + 1] = 0x00;
    buf[i + 2] = 0x00;
    buf[i + 3] = 0xff;
}

/// Bresenham line, thickened by `width` (drawn as `width` parallel lines
/// offset perpendicular to travel) since a single-pixel line is nearly
/// invisible at this size.
fn draw_line(buf: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, width: i32) {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = ((dx * dx + dy * dy) as f32).sqrt().max(1.0);
    // Perpendicular unit direction, scaled for the offsets below.
    let (px, py) = (-(dy as f32) / len, (dx as f32) / len);
    for w in 0..width {
        let offset = w - width / 2;
        let ox = (px * offset as f32).round() as i32;
        let oy = (py * offset as f32).round() as i32;
        draw_thin_line(buf, x0 + ox, y0 + oy, x1 + ox, y1 + oy);
    }
}

fn draw_thin_line(buf: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32) {
    let (mut x0, mut y0) = (x0, y0);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        set_px(buf, x0, y0);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// I-beam: a vertical stem with top/bottom serifs, centered in the bitmap
/// (unlike the arrow, whose hotspot is its tip at (0,0) - an I-beam's
/// hotspot is its center, where the text caret actually is).
fn text_bitmap() -> Vec<u8> {
    let mut buf = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    let mid = CURSOR_SIZE / 2;
    draw_line(&mut buf, mid, 3, mid, CURSOR_SIZE - 4, 2);
    draw_line(&mut buf, mid - 4, 3, mid + 4, 3, 2);
    draw_line(&mut buf, mid - 4, CURSOR_SIZE - 4, mid + 4, CURSOR_SIZE - 4, 2);
    add_white_halo(&mut buf);
    buf
}

/// Double-headed arrow along one axis (horizontal if `horizontal`,
/// vertical otherwise), hotspot at center - the standard edge/side resize
/// cursor shape.
fn straight_resize_bitmap(horizontal: bool) -> Vec<u8> {
    let mut buf = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    let mid = CURSOR_SIZE / 2;
    let (lo, hi) = (3, CURSOR_SIZE - 4);
    if horizontal {
        draw_line(&mut buf, lo, mid, hi, mid, 2);
        draw_line(&mut buf, lo, mid, lo + 5, mid - 5, 2);
        draw_line(&mut buf, lo, mid, lo + 5, mid + 5, 2);
        draw_line(&mut buf, hi, mid, hi - 5, mid - 5, 2);
        draw_line(&mut buf, hi, mid, hi - 5, mid + 5, 2);
    } else {
        draw_line(&mut buf, mid, lo, mid, hi, 2);
        draw_line(&mut buf, mid, lo, mid - 5, lo + 5, 2);
        draw_line(&mut buf, mid, lo, mid + 5, lo + 5, 2);
        draw_line(&mut buf, mid, hi, mid - 5, hi - 5, 2);
        draw_line(&mut buf, mid, hi, mid + 5, hi - 5, 2);
    }
    add_white_halo(&mut buf);
    buf
}

/// Double-headed arrow along a diagonal: NW-SE if `nwse`, NE-SW otherwise.
/// Hotspot at center, same as the straight resize shapes.
fn diagonal_resize_bitmap(nwse: bool) -> Vec<u8> {
    let mut buf = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    let (lo, hi) = (3, CURSOR_SIZE - 4);
    let (x0, y0, x1, y1) = if nwse { (lo, lo, hi, hi) } else { (lo, hi, hi, lo) };
    draw_line(&mut buf, x0, y0, x1, y1, 2);
    // Arrowheads: two short strokes angled off each end, perpendicular-ish
    // to the main diagonal so they read as a `<` / `>`-style head.
    let head = |buf: &mut [u8], hx: i32, hy: i32, ax1: i32, ay1: i32, ax2: i32, ay2: i32| {
        draw_line(buf, hx, hy, ax1, ay1, 2);
        draw_line(buf, hx, hy, ax2, ay2, 2);
    };
    if nwse {
        head(&mut buf, x0, y0, x0 + 7, y0, x0, y0 + 7);
        head(&mut buf, x1, y1, x1 - 7, y1, x1, y1 - 7);
    } else {
        head(&mut buf, x0, y0, x0 + 7, y0, x0, y0 - 7);
        head(&mut buf, x1, y1, x1 - 7, y1, x1, y1 + 7);
    }
    add_white_halo(&mut buf);
    buf
}

/// Solid-fills a rectangle, clamped to the canvas - used by [`pointer_bitmap`]
/// instead of [`draw_line`]'s thin strokes, since a hand cursor reads better
/// as a few blocky filled shapes than as an outline at this resolution.
fn fill_rect(buf: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32) {
    for y in y0.max(0)..=y1.min(CURSOR_SIZE - 1) {
        for x in x0.max(0)..=x1.min(CURSOR_SIZE - 1) {
            set_px(buf, x, y);
        }
    }
}

/// A crosshair: full-height vertical line through full-width horizontal
/// line, hotspot dead center where the two cross - `zwp_pointer_constraints`
/// clients (games, precise pixel-editors) and any `cursor: crosshair` CSS
/// both expect this exact shape.
fn crosshair_bitmap() -> Vec<u8> {
    let mut buf = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    let mid = CURSOR_SIZE / 2;
    draw_line(&mut buf, mid, 1, mid, CURSOR_SIZE - 2, 1);
    draw_line(&mut buf, 1, mid, CURSOR_SIZE - 2, mid, 1);
    add_white_halo(&mut buf);
    buf
}

/// Four-way move arrow: one line from center to each edge, with an
/// arrowhead at every tip - `cursor: move` (draggable panels, reordering
/// lists), built the same way [`diagonal_resize_bitmap`] builds its two
/// arrowheads, just aimed at all four cardinal directions instead of one
/// diagonal.
fn move_bitmap() -> Vec<u8> {
    let mut buf = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    let mid = CURSOR_SIZE / 2;
    let (lo, hi) = (2, CURSOR_SIZE - 3);
    draw_line(&mut buf, mid, lo, mid, hi, 2);
    draw_line(&mut buf, lo, mid, hi, mid, 2);
    let head = |buf: &mut [u8], hx: i32, hy: i32, ax1: i32, ay1: i32, ax2: i32, ay2: i32| {
        draw_line(buf, hx, hy, ax1, ay1, 2);
        draw_line(buf, hx, hy, ax2, ay2, 2);
    };
    head(&mut buf, mid, lo, mid - 4, lo + 5, mid + 4, lo + 5);
    head(&mut buf, mid, hi, mid - 4, hi - 5, mid + 4, hi - 5);
    head(&mut buf, lo, mid, lo + 5, mid - 4, lo + 5, mid + 4);
    head(&mut buf, hi, mid, hi - 5, mid - 4, hi - 5, mid + 4);
    add_white_halo(&mut buf);
    buf
}

/// A blocky pointing hand: an upright index finger with the hotspot at its
/// tip, above a wider palm block - `CursorIcon::Pointer`, the single most
/// common named shape after the default arrow (every hyperlink, every
/// clickable non-form control). Filled rectangles rather than an outline,
/// since a recognisable hand silhouette needs more coverage than a few thin
/// strokes can give at 24px.
fn pointer_bitmap() -> Vec<u8> {
    let mut buf = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    fill_rect(&mut buf, 8, 1, 11, 11);
    fill_rect(&mut buf, 4, 10, 19, 20);
    add_white_halo(&mut buf);
    buf
}

fn upload(data: Vec<u8>) -> smithay::backend::renderer::element::memory::MemoryRenderBuffer {
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
    use smithay::utils::Transform;
    MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (CURSOR_SIZE, CURSOR_SIZE), 1, Transform::Normal, None)
}

/// The centered shapes' hotspot: dead center of the bitmap, unlike the
/// arrow's tip-at-origin. Shared by every shape built here except the
/// arrow itself.
pub(crate) const CENTERED_HOTSPOT: (i32, i32) = (CURSOR_SIZE / 2, CURSOR_SIZE / 2);

pub(crate) fn make_buffers() -> CursorBuffers {
    CursorBuffers {
        arrow: make_buffer(),
        text: upload(text_bitmap()),
        ns_resize: upload(straight_resize_bitmap(false)),
        ew_resize: upload(straight_resize_bitmap(true)),
        nesw_resize: upload(diagonal_resize_bitmap(false)),
        nwse_resize: upload(diagonal_resize_bitmap(true)),
        crosshair: upload(crosshair_bitmap()),
        move_icon: upload(move_bitmap()),
        pointer: upload(pointer_bitmap()),
    }
}

/// [`pointer_bitmap`]'s hotspot: the fingertip, near the top of the canvas
/// - unlike the centered resize/crosshair/move shapes, a pointing hand's
/// "active point" for click purposes is where the finger tip actually is,
/// the same reasoning the built-in arrow's tip-at-origin hotspot already
/// uses.
const POINTER_HOTSPOT: (i32, i32) = (9, 1);

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
    buffers: &CursorBuffers,
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
    use smithay::input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData};
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

    // No client image. A named shape we have dedicated art for gets it
    // (centered on the pointer - these are all symmetric shapes, unlike
    // the arrow); anything else (Default, or one of the many shapes we
    // don't draw, e.g. Grab/Pointer/Crosshair) falls back to the arrow,
    // whose hotspot is its tip instead, at the origin.
    let (buffer, at) = match status {
        CursorImageStatus::Named(icon) => match icon {
            CursorIcon::Text | CursorIcon::VerticalText => {
                (&buffers.text, (local.0 - CENTERED_HOTSPOT.0, local.1 - CENTERED_HOTSPOT.1))
            }
            CursorIcon::EResize | CursorIcon::WResize | CursorIcon::EwResize | CursorIcon::ColResize => {
                (&buffers.ew_resize, (local.0 - CENTERED_HOTSPOT.0, local.1 - CENTERED_HOTSPOT.1))
            }
            CursorIcon::NResize | CursorIcon::SResize | CursorIcon::NsResize | CursorIcon::RowResize => {
                (&buffers.ns_resize, (local.0 - CENTERED_HOTSPOT.0, local.1 - CENTERED_HOTSPOT.1))
            }
            CursorIcon::NeResize | CursorIcon::SwResize | CursorIcon::NeswResize => {
                (&buffers.nesw_resize, (local.0 - CENTERED_HOTSPOT.0, local.1 - CENTERED_HOTSPOT.1))
            }
            CursorIcon::NwResize | CursorIcon::SeResize | CursorIcon::NwseResize | CursorIcon::AllResize => {
                (&buffers.nwse_resize, (local.0 - CENTERED_HOTSPOT.0, local.1 - CENTERED_HOTSPOT.1))
            }
            CursorIcon::Crosshair => (&buffers.crosshair, (local.0 - CENTERED_HOTSPOT.0, local.1 - CENTERED_HOTSPOT.1)),
            CursorIcon::Move => (&buffers.move_icon, (local.0 - CENTERED_HOTSPOT.0, local.1 - CENTERED_HOTSPOT.1)),
            CursorIcon::Pointer => (&buffers.pointer, (local.0 - POINTER_HOTSPOT.0, local.1 - POINTER_HOTSPOT.1)),
            _ => (&buffers.arrow, (local.0, local.1)),
        },
        _ => (&buffers.arrow, (local.0, local.1)),
    };
    let at = (at.0 as f64, at.1 as f64);
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

    #[test]
    fn arrow_tail_is_a_single_tapering_shape_not_a_lopsided_fork() {
        // Regression test: the tail below the arrowhead used to split into
        // two separate legs of visibly different widths (one tapering to a
        // point, the other a constant-width block that never tapered) --
        // reported live as "one side is bigger than the other, not
        // conventional at all". Each row of the tail must now be a single
        // contiguous opaque run starting at column 0 (no gap splitting it
        // into two pieces), and its width must never *grow* from the row
        // above - a monotonic taper, not a fork.
        let buf = arrow_bitmap();
        let opaque_at = |x: usize, y: usize| buf[(y * CURSOR_SIZE as usize + x) * 4 + 3] != 0;
        let mut prev_width: Option<usize> = None;
        for y in 15..CURSOR_SIZE as usize {
            let width = (0..CURSOR_SIZE as usize).take_while(|&x| opaque_at(x, y)).count();
            if width == 0 {
                continue;
            }
            assert!(opaque_at(0, y), "row {y}: tail must start flush at column 0");
            for x in width..CURSOR_SIZE as usize {
                assert!(!opaque_at(x, y), "row {y}: found opaque pixel at x={x} past a gap - tail has forked into two pieces");
            }
            if let Some(prev) = prev_width {
                assert!(width <= prev, "row {y}: tail width grew from {prev} to {width} - not a monotonic taper");
            }
            prev_width = Some(width);
        }
    }

    fn opaque_px_count(buf: &[u8]) -> usize {
        buf.chunks_exact(4).filter(|px| px[3] != 0).count()
    }

    #[test]
    fn text_bitmap_is_the_expected_size_and_draws_something() {
        let buf = text_bitmap();
        assert_eq!(buf.len(), (CURSOR_SIZE * CURSOR_SIZE * 4) as usize);
        assert!(opaque_px_count(&buf) > 10, "expected a visible I-beam");
    }

    #[test]
    fn straight_resize_bitmaps_are_distinguishable_from_each_other() {
        let horizontal = straight_resize_bitmap(true);
        let vertical = straight_resize_bitmap(false);
        assert!(opaque_px_count(&horizontal) > 10);
        assert!(opaque_px_count(&vertical) > 10);
        // A horizontal double-arrow and a vertical one should not paint the
        // exact same pixels - if they did, `render_elements` would be
        // silently showing the same shape for both directions.
        assert_ne!(horizontal, vertical);
    }

    #[test]
    fn diagonal_resize_bitmaps_are_distinguishable_from_each_other() {
        let nwse = diagonal_resize_bitmap(true);
        let nesw = diagonal_resize_bitmap(false);
        assert!(opaque_px_count(&nwse) > 10);
        assert!(opaque_px_count(&nesw) > 10);
        assert_ne!(nwse, nesw);
    }

    fn has_white_px(buf: &[u8]) -> bool {
        buf.chunks_exact(4).any(|px| px[3] != 0 && px[0] == 0xff && px[1] == 0xff && px[2] == 0xff)
    }

    /// The resize/text shapes are drawn as plain opaque black lines (see
    /// `set_px`'s doc comment) - with no halo they would be solid black
    /// with zero white pixels, i.e. nearly invisible over a dark window.
    /// Confirmed live: a screenshot of the resize cursor over a black
    /// terminal before this fix showed no visible shape at all. This test
    /// would fail against that code.
    #[test]
    fn resize_and_text_shapes_get_a_visible_halo() {
        assert!(has_white_px(&text_bitmap()), "I-beam has no white halo");
        assert!(has_white_px(&straight_resize_bitmap(true)), "ew-resize has no white halo");
        assert!(has_white_px(&straight_resize_bitmap(false)), "ns-resize has no white halo");
        assert!(has_white_px(&diagonal_resize_bitmap(true)), "nwse-resize has no white halo");
        assert!(has_white_px(&diagonal_resize_bitmap(false)), "nesw-resize has no white halo");
    }

    #[test]
    fn crosshair_is_the_expected_size_and_draws_something() {
        let buf = crosshair_bitmap();
        assert_eq!(buf.len(), (CURSOR_SIZE * CURSOR_SIZE * 4) as usize);
        assert!(opaque_px_count(&buf) > 10, "expected a visible crosshair");
        assert!(has_white_px(&buf), "crosshair has no white halo");
    }

    #[test]
    fn move_icon_is_the_expected_size_and_draws_something() {
        let buf = move_bitmap();
        assert_eq!(buf.len(), (CURSOR_SIZE * CURSOR_SIZE * 4) as usize);
        assert!(opaque_px_count(&buf) > 10, "expected a visible move icon");
        assert!(has_white_px(&buf), "move icon has no white halo");
    }

    #[test]
    fn pointer_hand_is_the_expected_size_and_draws_something() {
        let buf = pointer_bitmap();
        assert_eq!(buf.len(), (CURSOR_SIZE * CURSOR_SIZE * 4) as usize);
        assert!(opaque_px_count(&buf) > 10, "expected a visible pointer hand");
        assert!(has_white_px(&buf), "pointer hand has no white halo");
        // The fingertip (the hotspot) must actually be opaque, same
        // requirement the arrow's tip-pixel test already checks - otherwise
        // the cursor would appear offset from where clicks land.
        let (hx, hy) = POINTER_HOTSPOT;
        let i = ((hy * CURSOR_SIZE + hx) * 4 + 3) as usize;
        assert_eq!(buf[i], 0xff, "fingertip hotspot pixel must be opaque");
    }

    #[test]
    fn crosshair_move_and_pointer_are_distinguishable_from_each_other_and_from_existing_shapes() {
        let shapes = [crosshair_bitmap(), move_bitmap(), pointer_bitmap(), text_bitmap(), straight_resize_bitmap(true), diagonal_resize_bitmap(true)];
        for (i, a) in shapes.iter().enumerate() {
            for (j, b) in shapes.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "shapes {i} and {j} render identically");
                }
            }
        }
    }
}
