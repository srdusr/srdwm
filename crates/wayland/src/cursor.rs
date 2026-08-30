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
//! 2. **The system's real XCursor theme** (`load_theme_cursor`), resolved
//!    from `XCURSOR_THEME`/`XCURSOR_SIZE` or, failing that, GTK's own
//!    `gtk-cursor-theme-name`/`-size` - for when no client has set an
//!    image (over srdwm's own decorations and the desktop), tried for
//!    every shape below, not just the plain arrow.
//! 3. **A built-in hand-rasterized shape**, only if theme loading found
//!    nothing at all for that specific shape - no theme installed, an
//!    unreadable file, or the theme genuinely has no icon under any of
//!    the names tried. A cursor that is always present beats a prettier
//!    one that sometimes isn't there, the same reasoning as `decoration.
//!    rs`'s font fallback; this is the same set of bitmaps that used to
//!    be the *only* option for every shape but the arrow.
//!
//! `CursorImageStatus::Hidden` is honoured, so a client that hides the
//! pointer still gets its way. Named shapes we have dedicated art for
//! (text entry, the four resize directions, crosshair, move, and the
//! pointing-hand link-hover shape) go through the *same* theme resolution
//! the arrow does - each tries a short list of the theme's own names for
//! that shape (`ew-resize`, `sb_h_double_arrow`, ... for the horizontal
//! resize cursor, say) before falling back to the hand-drawn bitmap.
//! Every shape used to skip straight to the hand-drawn version regardless
//! of what the theme actually shipped, which is what made them look
//! noticeably cruder than the arrow next to them - reported live as the
//! resize cursor in particular looking "hideous", and the pointer/move
//! shapes barely visible at all, while the plain arrow (already theme-
//! resolved) looked fine.

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
    /// Every shape's hotspot travels with its buffer now, not just the
    /// arrow's: a real theme cursor's `xhot`/`yhot` when `load_theme_
    /// cursor` found one for this shape, or the fixed built-in value
    /// (`CENTERED_HOTSPOT`/`POINTER_HOTSPOT`/`(0, 0)`) when it fell back
    /// to the hand-drawn bitmap - see `make_buffers`.
    pub(crate) arrow_hotspot: (i32, i32),
    pub(crate) text: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) text_hotspot: (i32, i32),
    pub(crate) ns_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) ns_resize_hotspot: (i32, i32),
    pub(crate) ew_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) ew_resize_hotspot: (i32, i32),
    pub(crate) nesw_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) nesw_resize_hotspot: (i32, i32),
    pub(crate) nwse_resize: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) nwse_resize_hotspot: (i32, i32),
    pub(crate) crosshair: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) crosshair_hotspot: (i32, i32),
    pub(crate) move_icon: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) move_icon_hotspot: (i32, i32),
    pub(crate) pointer: smithay::backend::renderer::element::memory::MemoryRenderBuffer,
    pub(crate) pointer_hotspot: (i32, i32),
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

/// Tries each of `names` against the resolved theme in order, uploads the
/// first that resolves; falls back to `built_in()` (drawn at the fixed
/// `hotspot_fallback`) if none of them do. One helper for all nine shapes
/// `make_buffers` builds, so every one of them gets the same "real theme
/// cursor first, hand-drawn shape only if the theme genuinely has nothing"
/// treatment the arrow alone used to get.
fn load_or_draw(
    theme: &xcursor::CursorTheme,
    size: u32,
    names: &[&str],
    built_in: impl Fn() -> Vec<u8>,
    hotspot_fallback: (i32, i32),
) -> (smithay::backend::renderer::element::memory::MemoryRenderBuffer, (i32, i32)) {
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
    use smithay::utils::Transform;
    match load_theme_cursor(theme, size, names) {
        Some(tc) => (MemoryRenderBuffer::from_slice(&tc.bgra, Fourcc::Argb8888, tc.size, 1, Transform::Normal, None), tc.hotspot),
        None => (upload(built_in()), hotspot_fallback),
    }
}

/// The centered shapes' fallback hotspot: dead center of the built-in
/// bitmap, unlike the arrow's tip-at-origin. Only used when a shape falls
/// back to the hand-drawn bitmap - a real theme cursor carries its own
/// `xhot`/`yhot` regardless of where that happens to fall.
pub(crate) const CENTERED_HOTSPOT: (i32, i32) = (CURSOR_SIZE / 2, CURSOR_SIZE / 2);

pub(crate) fn make_buffers() -> CursorBuffers {
    // Resolved once, not once per shape: `CursorTheme::load` re-walks the
    // theme's `index.theme` inheritance chain and search paths every call,
    // real (if small) work worth not repeating nine times over for what is
    // - for every shape's own lookup - the exact same theme and size.
    let (theme_name, size) = theme_and_size();
    let theme = xcursor::CursorTheme::load(&theme_name);

    let (arrow, arrow_hotspot) = load_or_draw(&theme, size, &["left_ptr", "default", "arrow"], arrow_bitmap, (0, 0));
    let (text, text_hotspot) = load_or_draw(&theme, size, &["text", "xterm"], text_bitmap, CENTERED_HOTSPOT);
    let (ns_resize, ns_resize_hotspot) = load_or_draw(
        &theme,
        size,
        &["ns-resize", "sb_v_double_arrow", "v_double_arrow", "size_ver", "size-ver", "row-resize"],
        || straight_resize_bitmap(false),
        CENTERED_HOTSPOT,
    );
    let (ew_resize, ew_resize_hotspot) = load_or_draw(
        &theme,
        size,
        &["ew-resize", "sb_h_double_arrow", "h_double_arrow", "size_hor", "size-hor", "col-resize"],
        || straight_resize_bitmap(true),
        CENTERED_HOTSPOT,
    );
    let (nesw_resize, nesw_resize_hotspot) =
        load_or_draw(&theme, size, &["nesw-resize", "size_bdiag", "size-bdiag", "ne-resize", "sw-resize"], || diagonal_resize_bitmap(false), CENTERED_HOTSPOT);
    let (nwse_resize, nwse_resize_hotspot) =
        load_or_draw(&theme, size, &["nwse-resize", "size_fdiag", "size-fdiag", "nw-resize", "se-resize"], || diagonal_resize_bitmap(true), CENTERED_HOTSPOT);
    let (crosshair, crosshair_hotspot) = load_or_draw(&theme, size, &["crosshair", "cross", "tcross"], crosshair_bitmap, CENTERED_HOTSPOT);
    let (move_icon, move_icon_hotspot) = load_or_draw(&theme, size, &["move", "fleur", "size_all", "all-scroll"], move_bitmap, CENTERED_HOTSPOT);
    let (pointer, pointer_hotspot) = load_or_draw(&theme, size, &["pointer", "hand2", "pointing_hand", "hand1", "link"], pointer_bitmap, POINTER_HOTSPOT);

    CursorBuffers {
        arrow,
        arrow_hotspot,
        text,
        text_hotspot,
        ns_resize,
        ns_resize_hotspot,
        ew_resize,
        ew_resize_hotspot,
        nesw_resize,
        nesw_resize_hotspot,
        nwse_resize,
        nwse_resize_hotspot,
        crosshair,
        crosshair_hotspot,
        move_icon,
        move_icon_hotspot,
        pointer,
        pointer_hotspot,
    }
}

/// [`pointer_bitmap`]'s hotspot: the fingertip, near the top of the canvas
/// - unlike the centered resize/crosshair/move shapes, a pointing hand's
///   "active point" for click purposes is where the finger tip actually is,
///   the same reasoning the built-in arrow's tip-at-origin hotspot already
///   uses.
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

    // No client image. A named shape we have dedicated art for gets it;
    // anything else (Default, or one of the many shapes we still don't
    // draw, e.g. Grab/Wait/Help/NotAllowed) falls back to the arrow.
    // Every shape's hotspot travels with its own buffer now (`make_buffers`)
    // - a real theme cursor's `xhot`/`yhot` when one was found for that
    // specific shape, the fixed `CENTERED_HOTSPOT`/`POINTER_HOTSPOT`/
    // `(0, 0)` fallback otherwise - rather than every non-arrow shape
    // assuming the same centered point regardless of what actually got
    // drawn.
    let (buffer, hotspot) = match status {
        CursorImageStatus::Named(icon) => match icon {
            CursorIcon::Text | CursorIcon::VerticalText => (&buffers.text, buffers.text_hotspot),
            CursorIcon::EResize | CursorIcon::WResize | CursorIcon::EwResize | CursorIcon::ColResize => (&buffers.ew_resize, buffers.ew_resize_hotspot),
            CursorIcon::NResize | CursorIcon::SResize | CursorIcon::NsResize | CursorIcon::RowResize => (&buffers.ns_resize, buffers.ns_resize_hotspot),
            CursorIcon::NeResize | CursorIcon::SwResize | CursorIcon::NeswResize => (&buffers.nesw_resize, buffers.nesw_resize_hotspot),
            CursorIcon::NwResize | CursorIcon::SeResize | CursorIcon::NwseResize | CursorIcon::AllResize => (&buffers.nwse_resize, buffers.nwse_resize_hotspot),
            CursorIcon::Crosshair => (&buffers.crosshair, buffers.crosshair_hotspot),
            CursorIcon::Move => (&buffers.move_icon, buffers.move_icon_hotspot),
            CursorIcon::Pointer => (&buffers.pointer, buffers.pointer_hotspot),
            _ => (&buffers.arrow, buffers.arrow_hotspot),
        },
        _ => (&buffers.arrow, buffers.arrow_hotspot),
    };
    let at = (local.0 - hotspot.0, local.1 - hotspot.1);
    let at = (at.0 as f64, at.1 as f64);
    match MemoryRenderBufferRenderElement::from_buffer(renderer, at, buffer, None, None, None, Kind::Cursor) {
        Ok(e) => vec![OverlayElement::Memory(e)],
        Err(e) => {
            log::warn!("cursor: failed to import bitmap: {e}");
            Vec::new()
        }
    }
}


/// Resolves which XCursor theme to load real cursors from, and at what
/// size.
///
/// `XCURSOR_THEME`/`XCURSOR_SIZE` are the standard override, but nothing
/// sets them on a session started this way (confirmed live) - so the
/// fallback below, reading GTK's own `gtk-cursor-theme-name`/
/// `gtk-cursor-theme-size` straight out of `settings.ini`, is what actually
/// resolves the theme apps on the same session are themed with in practice.
/// Without it, `xcursor::CursorTheme::load`'s own search lands on
/// `/usr/share/icons/default/index.theme`, which inherits Adwaita on a
/// machine with no `~/.icons/default` override - not what GTK reports
/// (confirmed live: Sweet-cursors), so the compositor's own pointer would
/// keep not matching every app's client-drawn cursor even after this.
fn theme_and_size() -> (String, u32) {
    if let Ok(theme) = std::env::var("XCURSOR_THEME") {
        if !theme.is_empty() {
            let size = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(CURSOR_SIZE as u32);
            return (theme, size);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let settings = std::path::Path::new(&home).join(".config/gtk-3.0/settings.ini");
        if let Ok(contents) = std::fs::read_to_string(&settings) {
            let mut name = None;
            let mut size = None;
            for line in contents.lines() {
                let line = line.trim();
                if let Some(v) = line.strip_prefix("gtk-cursor-theme-name=") {
                    name = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("gtk-cursor-theme-size=") {
                    size = v.trim().parse().ok();
                }
            }
            if let Some(name) = name {
                return (name, size.unwrap_or(CURSOR_SIZE as u32));
            }
        }
    }
    ("default".to_string(), CURSOR_SIZE as u32)
}

/// A loaded XCursor image, converted to what `make_buffers` needs to upload
/// it: BGRA8888 pixels, pixel dimensions, and hotspot. Generic over which
/// shape it came from - was arrow-only (`ThemeArrow`) before every shape
/// started resolving through the theme.
struct ThemeCursor {
    bgra: Vec<u8>,
    size: (i32, i32),
    hotspot: (i32, i32),
}

/// Loads one named cursor (trying each of `names` in order, using the
/// first that resolves) from an already-resolved theme, picking whichever
/// bundled nominal size is closest to the target. Was arrow-only
/// (`load_theme_arrow`, a single hardcoded `"left_ptr"`) before every
/// shape started resolving through the theme - several themes only ship
/// the legacy X11 name for a given shape (`sb_h_double_arrow` rather than
/// the modern `ew-resize`, say), so trying a short list rather than one
/// fixed name is what makes this actually portable across themes, not
/// just the one installed here.
///
/// Converts pixels from the crate's RGBA byte order (`Image::pixels_rgba`,
/// straight off disk) to the BGRA order every buffer in this file uses for
/// `Fourcc::Argb8888` - see `arrow_bitmap`'s own per-pixel byte order.
/// XCursor pixel data is already premultiplied alpha per the file format
/// spec, same as every bitmap built here, so only the channel order needs
/// converting, not the alpha itself.
///
/// Returns `None` on any failure - none of `names` found, a corrupt file,
/// a pixel count that doesn't match the declared dimensions - so the
/// caller (`load_or_draw`) falls back to that shape's own hand-drawn
/// bitmap, which is the entire reason that fallback exists: see this
/// module's own doc comment.
fn load_theme_cursor(theme: &xcursor::CursorTheme, size: u32, names: &[&str]) -> Option<ThemeCursor> {
    let path = names.iter().find_map(|name| theme.load_icon(name))?;
    let bytes = std::fs::read(&path).ok()?;
    let images = xcursor::parser::parse_xcursor(&bytes)?;
    let image = images.into_iter().min_by_key(|img| (img.size as i64 - size as i64).abs())?;
    if image.width == 0 || image.height == 0 || image.pixels_rgba.len() != (image.width * image.height * 4) as usize {
        return None;
    }
    Some(ThemeCursor {
        bgra: rgba_to_bgra(&image.pixels_rgba),
        size: (image.width as i32, image.height as i32),
        hotspot: (image.xhot as i32, image.yhot as i32),
    })
}

/// Per-pixel R,G,B,A -> B,G,R,A channel reorder - the byte order `Fourcc::
/// Argb8888` buffers use everywhere else in this file (see `arrow_bitmap`'s
/// own doc comment), versus the straight-off-disk order `xcursor::parser`
/// hands back in `Image::pixels_rgba`. Alpha is untouched: XCursor pixel
/// data is already premultiplied per the file format spec, same as every
/// bitmap built here.
fn rgba_to_bgra(pixels_rgba: &[u8]) -> Vec<u8> {
    let mut bgra = Vec::with_capacity(pixels_rgba.len());
    for px in pixels_rgba.as_chunks::<4>().0 {
        bgra.push(px[2]);
        bgra.push(px[1]);
        bgra.push(px[0]);
        bgra.push(px[3]);
    }
    bgra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_bgra_reorders_channels_and_leaves_alpha_alone() {
        // Same fixture xcursor's own rgba_to_argb test uses, so the two
        // conversions are easy to cross-check by eye: R=0x12, G=0x34,
        // B=0x56, A=0x78.
        let rgba = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(rgba_to_bgra(&rgba), vec![0x56, 0x34, 0x12, 0x78]);
    }

    #[test]
    fn rgba_to_bgra_handles_multiple_pixels_independently() {
        let rgba = [0x01, 0x02, 0x03, 0x04, 0xaa, 0xbb, 0xcc, 0xdd];
        assert_eq!(rgba_to_bgra(&rgba), vec![0x03, 0x02, 0x01, 0x04, 0xcc, 0xbb, 0xaa, 0xdd]);
    }

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
