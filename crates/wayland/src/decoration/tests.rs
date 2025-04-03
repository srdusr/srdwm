use super::*;
use super::buttons::*;
use super::color::*;
use super::shadow::*;

#[test]
fn border_strips_surround_geometry_without_overlapping_it() {
    let geom = srdwm_core::Rect::new(100, 100, 200, 150);
    let [top, bottom, left, right] = border_strips(geom, 3);
    // Every strip's own rect must stay entirely outside `geom` - these
    // are meant to frame the window, not clip into its own titlebar or
    // content.
    assert_eq!(top, srdwm_core::Rect::new(97, 97, 206, 3));
    assert_eq!(bottom, srdwm_core::Rect::new(97, 250, 206, 3));
    assert_eq!(left, srdwm_core::Rect::new(97, 100, 3, 150));
    assert_eq!(right, srdwm_core::Rect::new(300, 100, 3, 150));
}

#[test]
fn shadow_rect_expands_geometry_by_shadow_size_on_every_side() {
    let geom = srdwm_core::Rect::new(100, 100, 200, 150);
    let s = shadow_rect(geom);
    assert_eq!(s, srdwm_core::Rect::new(100 - SHADOW_SIZE as i32, 100 - SHADOW_SIZE as i32, 200 + SHADOW_SIZE * 2, 150 + SHADOW_SIZE * 2));
}

#[test]
fn shadow_bitmap_is_the_expected_size_and_transparent_under_the_window() {
    let buf = shadow_bitmap(40, 20, 0, SHADOW_MAX_ALPHA);
    let width = 40 + SHADOW_SIZE * 2;
    let height = 20 + SHADOW_SIZE * 2;
    assert_eq!(buf.len(), (width * height * 4) as usize);
    // Dead center is inside the window's own footprint - must stay
    // fully transparent, since the window's own content draws over it.
    let mid = ((height / 2) * width + width / 2) * 4;
    assert_eq!(buf[mid as usize + 3], 0);
}

#[test]
fn shadow_bitmap_is_darkest_right_at_the_window_edge_and_fades_outward() {
    let buf = shadow_bitmap(40, 20, 0, SHADOW_MAX_ALPHA);
    let width = (40 + SHADOW_SIZE * 2) as usize;
    // Walking straight up from the window's horizontal center, from one
    // pixel above its top edge (row SHADOW_SIZE - 1) out to the shadow's
    // own outer edge (row 0): alpha must start near SHADOW_MAX_ALPHA and
    // strictly decrease to 0.
    let x = width / 2;
    let mut last_alpha = 255u8;
    for row in (0..SHADOW_SIZE as usize).rev() {
        let i = (row * width + x) * 4;
        let alpha = buf[i + 3];
        assert!(alpha <= last_alpha, "alpha rose from {last_alpha} to {alpha} moving outward at row {row}");
        last_alpha = alpha;
    }
    assert_eq!(last_alpha, 0, "outermost row must be fully transparent");
}

#[test]
fn shadow_falloff_is_eased_not_linear() {
    // The actual visual change: a plain linear ramp drops the same
    // amount of alpha every pixel, which reads as a visible ring with
    // a hard-ish inner edge even though it's transparent at both
    // ends. An eased (smoothstep) curve drops *less* per pixel near
    // both ends and *more* in the middle - this is what distinguishes
    // it from linear at the byte level, not just "still monotonic".
    let buf = shadow_bitmap(40, 20, 0, SHADOW_MAX_ALPHA);
    let width = (40 + SHADOW_SIZE * 2) as usize;
    let x = width / 2;
    let alpha_at = |dist_from_edge: u32| {
        let row = SHADOW_SIZE - dist_from_edge;
        buf[(row as usize * width + x) * 4 + 3] as i32
    };
    // Step near the shadow's own inner edge (dist 1 -> 2) versus a
    // step through the middle (dist SHADOW_SIZE/2 -> SHADOW_SIZE/2+1):
    // eased must drop less near the edge than a linear ramp would
    // (linear drops `max_alpha / SHADOW_SIZE` every single step,
    // uniformly) - the middle step must drop more than that same
    // uniform linear amount to compensate, since both curves start
    // and end at the same two values.
    let linear_step = SHADOW_MAX_ALPHA as i32 / SHADOW_SIZE as i32;
    let near_edge_drop = alpha_at(1) - alpha_at(2);
    let middle_drop = alpha_at(SHADOW_SIZE / 2) - alpha_at(SHADOW_SIZE / 2 + 1);
    assert!(near_edge_drop < linear_step, "near-edge step ({near_edge_drop}) should be gentler than a linear ramp's own uniform step ({linear_step})");
    assert!(middle_drop > near_edge_drop, "the steepest part of an eased curve should be in the middle ({middle_drop}), not at the edge ({near_edge_drop})");
}

#[test]
fn a_lower_max_alpha_produces_a_strictly_fainter_shadow_throughout() {
    // Locks in the `max_alpha` parameter's actual effect - an
    // unfocused window's dimmed shadow (see `redraw_decoration_
    // buffer`'s own call site) must never be darker than the focused
    // one at any pixel, not just "different".
    let focused = shadow_bitmap(40, 20, 0, SHADOW_MAX_ALPHA);
    let dimmed = shadow_bitmap(40, 20, 0, SHADOW_MAX_ALPHA / 2);
    assert_ne!(focused, dimmed, "sanity: halving max_alpha must actually change the bitmap");
    for (f, d) in focused.chunks_exact(4).zip(dimmed.chunks_exact(4)) {
        assert!(d[3] <= f[3], "dimmed alpha {} must never exceed focused alpha {}", d[3], f[3]);
    }
}

#[test]
fn rounded_shadow_corner_tip_is_further_than_the_square_corners_own() {
    // Regression coverage for the actual live bug: a window's shadow
    // used to always taper with plain Chebyshev (square) distance, so
    // its own corner cut a hard diagonal well outside a *rounded*
    // window's real curve, sitting there as a visibly different,
    // unrelated shape - confirmed live via a real screenshot at
    // actual render resolution. At the window's old sharp-corner tip
    // (dx = dy = 0), the rounded window's boundary has curved away
    // entirely, so this point must read as *further* from the window
    // than plain Chebyshev's `0` - not `0`, which would mean "right on
    // the window's own edge", true only for a genuinely square corner.
    let radius = 6;
    assert_eq!(rounded_edge_distance(0, 0, 0), 0, "sanity: radius 0 must match plain Chebyshev distance exactly");
    let rounded = rounded_edge_distance(0, 0, radius);
    assert!(rounded > 0, "the old sharp corner's own tip must be outside a rounded window, not sitting right on its edge");
    // Exact value, worked out by hand: distance from the tip to the
    // rounding circle's centre (radius, radius) is radius*sqrt(2), so
    // distance to the circle's own boundary is radius*(sqrt(2) - 1).
    let expected = (radius as f32 * (2.0f32.sqrt() - 1.0)).round() as u32;
    assert_eq!(rounded, expected);
}

#[test]
fn rounded_edge_distance_matches_plain_chebyshev_along_a_flat_edge() {
    // Rounding only ever touches the four corners - directly above,
    // below, left or right of the window (exactly one of dx/dy is 0),
    // a rounded rect's boundary is identical to a square one's.
    for (dx, dy) in [(5, 0), (0, 5), (12, 0), (0, 12)] {
        assert_eq!(rounded_edge_distance(dx, dy, 6), dx.max(dy), "flat-edge point ({dx}, {dy}) must use plain Chebyshev distance, not the corner formula");
    }
}

#[test]
fn shadow_bitmap_corner_is_softer_than_a_square_windows_when_rounded() {
    let square = shadow_bitmap(40, 40, 0, SHADOW_MAX_ALPHA);
    let rounded = shadow_bitmap(40, 40, 6, SHADOW_MAX_ALPHA);
    let width = (40 + SHADOW_SIZE * 2) as usize;
    // The old sharp corner's own tip: `SHADOW_SIZE` pixels in from the
    // bitmap's own outer edge on both axes, i.e. exactly at row/column
    // `SHADOW_SIZE` - where `dx == dy == 0` in `edge_distance`'s own
    // terms, the window's true corner point. A square window's shadow
    // is at its darkest possible value there (`dist == 0`, right on
    // the window's own edge); a rounded window's real boundary has
    // already curved away from that exact point, so it must read
    // strictly lighter there (a real positive distance means a
    // partially-faded, not maximum, alpha) - not identical, which is
    // what the bug this fixes looked like.
    let tip = ((SHADOW_SIZE as usize) * width + SHADOW_SIZE as usize) * 4;
    assert_eq!(square[tip + 3], 0, "sanity: a square window's shadow is fully suppressed right at its own corner");
    assert!(rounded[tip + 3] > 0, "a rounded window's shadow must actually draw at the old sharp-corner tip, since that point is genuinely outside the rounded boundary");
}

#[test]
fn fills_background_when_no_text() {
    let buf = render_titlebar(40, 20, "", (0x2e, 0x34, 0x40), (0xec, 0xef, 0xf4), true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    assert_eq!(buf.len(), 40 * 20 * 4);
    // Center, not (0,0): the top-left pixel is inside the rounded
    // corner `round_top_corners` clips away, so it's transparent by
    // design - see `corners_are_clipped_but_the_middle_is_not` below.
    let mid = ((20 / 2) * 40 + 40 / 2) * 4;
    assert_eq!(&buf[mid..mid + 4], &rgb_to_bgra((0x2e, 0x34, 0x40), 255));
}

#[test]
fn button_icons_are_drawn_in_the_squares_hit_test_assigns_them() {
    // Regression test for a bug where every drawn icon was one full
    // button-width left of where a click on it actually landed: the
    // visible "X" triggered Maximize, the visible square triggered
    // Minimize, and the true Close hit-zone (the rightmost
    // TITLEBAR_HEIGHT-wide band) was blank. `button_box`'s
    // `right_offset` must put each icon in the same square
    // `ResizeEdge::hit_test` assigns to it - checked here by picking
    // the centre pixel of each drawn icon's square and confirming
    // `hit_test` reports the matching button for that same point.
    let (width, height) = (300u32, srdwm_core::TITLEBAR_HEIGHT);
    let bg = (0x2e, 0x34, 0x40);
    let fg = (0xec, 0xef, 0xf4);
    let buf = render_titlebar(width, height, "", bg, fg, true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let frame = srdwm_core::Rect::new(0, 0, width, height);
    let (width, height) = (width as usize, height as usize);

    let bg_bytes = rgb_to_bgra(bg, 255);
    let pitch = srdwm_core::BUTTON_PITCH as usize;
    let margin = srdwm_core::BUTTON_CLUSTER_MARGIN as usize;
    for (right_offset, expected) in [(margin, srdwm_core::TitlebarHit::Close), (margin + pitch, srdwm_core::TitlebarHit::Maximize), (margin + pitch * 2, srdwm_core::TitlebarHit::Minimize)] {
        let (x0, y0, x1, y1) = button_box(width, height, right_offset, false, BUTTON_MARGIN);
        let drawn = (y0..=y1).any(|y| (x0..=x1).any(|x| buf[(y as usize * width + x as usize) * 4..(y as usize * width + x as usize) * 4 + 4] != bg_bytes));
        assert!(drawn, "expected some drawn icon pixel inside the right_offset={right_offset} square");
        let cx = (x0 + x1) / 2;
        let cy = (y0 + y1) / 2;
        assert_eq!(
            srdwm_core::ResizeEdge::hit_test(frame, cx, cy, true, 0, srdwm_core::RESIZE_MARGIN, false, None, false),
            Some(expected),
            "icon drawn at right_offset={right_offset} does not land in the square hit_test assigns to {expected:?}"
        );
    }
}

#[test]
fn a_dialog_draws_only_close_and_never_a_coloured_traffic_light() {
    // Requested directly: "dialog windows shouldn't have maximize/
    // minimize buttons... don't use traffic lights there ever."
    let (width, height) = (300u32, srdwm_core::TITLEBAR_HEIGHT);
    let bg = (0x2e, 0x34, 0x40);
    let fg = (0xec, 0xef, 0xf4);
    let bg_bytes = rgb_to_bgra(bg, 255);
    // `traffic_lights = true` passed in deliberately - `is_dialog`
    // must override it regardless of what the active theme otherwise
    // uses everywhere else. `glyph_always = true` so Close's own X is
    // visible without needing a live hover to check it landed.
    let dialog = render_titlebar(width, height, "", bg, fg, true, CORNER_RADIUS, 0, true, None, false, false, true, None, true, true);
    let normal = render_titlebar(width, height, "", bg, fg, true, CORNER_RADIUS, 0, true, None, false, false, true, None, true, false);
    let (w, h) = (width as usize, height as usize);

    // Where a normal (non-dialog) titlebar draws Maximize (offset
    // `BUTTON_PITCH`) must be untouched background on the dialog --
    // that button was never drawn there at all, not just hit-test-
    // unreachable.
    let margin = srdwm_core::BUTTON_CLUSTER_MARGIN as usize;
    let maximize_box = button_box(w, h, margin + srdwm_core::BUTTON_PITCH as usize, false, BUTTON_MARGIN);
    let maximize_drawn_on_normal =
        (maximize_box.1..=maximize_box.3).any(|y| (maximize_box.0..=maximize_box.2).any(|x| normal[(y as usize * w + x as usize) * 4..(y as usize * w + x as usize) * 4 + 4] != bg_bytes));
    assert!(maximize_drawn_on_normal, "sanity: a normal titlebar really does draw something at the Maximize slot");
    let maximize_drawn_on_dialog =
        (maximize_box.1..=maximize_box.3).any(|y| (maximize_box.0..=maximize_box.2).any(|x| dialog[(y as usize * w + x as usize) * 4..(y as usize * w + x as usize) * 4 + 4] != bg_bytes));
    assert!(!maximize_drawn_on_dialog, "a dialog must draw nothing at all at the Maximize slot");

    // Close's own box: no coloured fill (a flat, opaque red/near-red
    // dot, same family `TRAFFIC_LIGHT_CLOSE` produces) anywhere in it
    // - only the plain-glyph X, drawn in `fg`, may appear.
    let close_box = button_box(w, h, margin, false, BUTTON_MARGIN);
    for y in close_box.1..=close_box.3 {
        for x in close_box.0..=close_box.2 {
            let i = (y as usize * w + x as usize) * 4;
            let px = &dialog[i..i + 4];
            // BGRA - a red-family traffic light has a strongly
            // dominant red channel (index 2) well above both blue and
            // green; the plain glyph (`fg`, near-white/grey) and the
            // untouched background do not.
            let (b, g, r) = (px[0] as i32, px[1] as i32, px[2] as i32);
            assert!(!(r > g + 40 && r > b + 40), "close button pixel at ({x},{y}) reads as a coloured traffic light: bgra={px:?}");
        }
    }
}

#[test]
fn drawing_title_changes_some_pixels_when_font_available() {
    if find_system_font().is_none() {
        eprintln!("skipping: no system font found in this sandbox");
        return;
    }
    let bg = (0x2e, 0x34, 0x40);
    let fg = (0xec, 0xef, 0xf4);
    let buf = render_titlebar(200, 30, "Terminal", bg, fg, true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let bg_bytes = rgb_to_bgra(bg, 255);
    let changed = buf.chunks_exact(4).any(|px| px != bg_bytes);
    assert!(changed, "expected at least one pixel to differ from the background once text is drawn");
}

#[test]
fn centered_title_starts_further_right_than_left_aligned() {
    if find_system_font().is_none() {
        eprintln!("skipping: no system font found in this sandbox");
        return;
    }
    let bg = (0x2e, 0x34, 0x40);
    let fg = (0xec, 0xef, 0xf4);
    let (width, height) = (60u32, 30u32);
    let bg_bytes = rgb_to_bgra(bg, 255);
    // No buttons at this width (`button_count` gates on
    // `width >= BUTTON_CLUSTER_MARGIN + BUTTON_PITCH * 3`), so
    // `text_limit` is the full width and a short title has real room to
    // actually move when centered - otherwise this could pass even
    // with centering silently broken.
    assert!(width < srdwm_core::BUTTON_CLUSTER_MARGIN + srdwm_core::BUTTON_PITCH * 3, "sanity: this fixture must have no button squares eating into text_limit");
    // Only rows clear of `round_top_corners`' own clipping (confined to
    // `CORNER_RADIUS` rows from the top - see its doc comment) --
    // otherwise a clipped-to-transparent corner pixel (a different
    // byte pattern than `bg_bytes`) registers as "ink" at column 0
    // regardless of where the text actually starts.
    let scan_rows = CORNER_RADIUS as usize..height as usize;
    let leftmost_ink_column = |buf: &[u8]| -> Option<usize> {
        (0..width as usize).find(|&x| scan_rows.clone().any(|y| buf[(y * width as usize + x) * 4..(y * width as usize + x) * 4 + 4] != bg_bytes))
    };
    let left = render_titlebar(width, height, "Hi", bg, fg, true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let centered = render_titlebar(width, height, "Hi", bg, fg, true, CORNER_RADIUS, 0, true, None, true, false, false, None, true, false);
    let left_start = leftmost_ink_column(&left).expect("left-aligned title must draw some ink");
    let centered_start = leftmost_ink_column(&centered).expect("centered title must draw some ink");
    assert!(centered_start > left_start, "a short title centered in a wide titlebar must start well to the right of the left-aligned version (left starts at {left_start}, centered at {centered_start})");
}

#[test]
fn centered_title_ignores_the_button_reservation_and_centers_on_the_whole_width() {
    if find_system_font().is_none() {
        eprintln!("skipping: no system font found in this sandbox");
        return;
    }
    let bg = (0x2e, 0x34, 0x40);
    let fg = (0xec, 0xef, 0xf4);
    // Wide enough for the 3-button reservation (`width >=
    // BUTTON_CLUSTER_MARGIN + BUTTON_PITCH * 3`) on the left - this is
    // exactly the case that used to center in the narrower `text_start
    // ..text_limit` span left over after the buttons, landing off the
    // window's true center instead of at `width / 2`.
    let (width, height) = (300u32, 30u32);
    assert!(width >= srdwm_core::BUTTON_CLUSTER_MARGIN + srdwm_core::BUTTON_PITCH * 3, "sanity: this fixture must have button squares eating into text_start");
    let bg_bytes = rgb_to_bgra(bg, 255);
    let scan_rows = CORNER_RADIUS as usize..height as usize;
    // Scan only from `text_start` (the button reservation's own right
    // edge) onward - the centering formula never places text left of
    // that regardless, and scanning from column 0 would also pick up
    // the button dots themselves (real, different-colour ink), not
    // just the title text this is actually trying to measure.
    let button_reservation = srdwm_core::BUTTON_CLUSTER_MARGIN as usize + srdwm_core::BUTTON_PITCH as usize * 3;
    let ink_columns = |buf: &[u8]| -> Vec<usize> {
        (button_reservation..width as usize).filter(|&x| scan_rows.clone().any(|y| buf[(y * width as usize + x) * 4..(y * width as usize + x) * 4 + 4] != bg_bytes)).collect()
    };
    let buf = render_titlebar(width, height, "Hi", bg, fg, true, CORNER_RADIUS, 0, true, None, true, true, false, None, true, false);
    let columns = ink_columns(&buf);
    let (first, last) = (*columns.first().expect("centered title must draw some ink"), *columns.last().unwrap());
    let midpoint = (first + last) as f32 / 2.0;
    let old_buggy_midpoint = 195.0; // center of the 90..300 span the button reservation used to leave
    assert!((midpoint - width as f32 / 2.0).abs() < 5.0, "title midpoint {midpoint} should land within a few px of the true window center ({}), not the button-adjusted span's own center ({old_buggy_midpoint})", width as f32 / 2.0);
}

#[test]
fn empty_title_leaves_buffer_all_background_outside_the_rounded_corners() {
    let bg = (0x10, 0x20, 0x30);
    let (width, height) = (50, 24);
    let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let bg_bytes = rgb_to_bgra(bg, 255);
    for (i, px) in buf.chunks_exact(4).enumerate() {
        let (x, y) = (i % width as usize, i / width as usize);
        let in_top_left = x < CORNER_RADIUS as usize && y < CORNER_RADIUS as usize;
        let in_top_right = x >= width as usize - CORNER_RADIUS as usize && y < CORNER_RADIUS as usize;
        if !in_top_left && !in_top_right {
            assert_eq!(px, bg_bytes, "unexpected non-background pixel at ({x}, {y})");
        }
    }
}

#[test]
fn corners_are_clipped_but_the_middle_is_not() {
    let bg = (0x10, 0x20, 0x30);
    let (width, height) = (50, 24);
    let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    // The very corner pixel is well outside the quarter-circle at any
    // sane radius - fully clipped.
    assert_eq!(alpha_at(0, 0), 0, "top-left corner pixel should be transparent");
    assert_eq!(alpha_at(width as usize - 1, 0), 0, "top-right corner pixel should be transparent");
    // Bottom corners are deliberately left square (see the function's
    // doc comment: the titlebar's bottom edge meets client content,
    // which can't be clipped the same way).
    assert_eq!(alpha_at(0, height as usize - 1), 255, "bottom-left must stay square");
    assert_eq!(alpha_at(width as usize - 1, height as usize - 1), 255, "bottom-right must stay square");
    // Centre is nowhere near either corner circle - untouched.
    assert_eq!(alpha_at(width as usize / 2, height as usize / 2), 255);
}

#[test]
fn clipped_corner_pixels_are_fully_premultiplied_zero_not_just_alpha() {
    // Regression test: `round_top_corners` used to zero only the alpha
    // byte of a clipped pixel, leaving the opaque background RGB behind
    // it untouched. This buffer is `Fourcc::Argb8888`, which both
    // Wayland/`wl_shm` and Pixman treat as premultiplied - Pixman's own
    // `OVER` compositing (`result = src + dst * (1 - src_alpha)`) does
    // not treat `alpha=0, rgb=<something>` as "contributes nothing": it
    // adds that stale, un-premultiplied `rgb` straight through, so the
    // "clipped" corner still rendered fully opaque and every window's
    // top corners read as square regardless of a nonzero radius --
    // confirmed live, pixel-by-pixel, zero transparency anywhere in a
    // real window's corner. A genuinely transparent premultiplied pixel
    // is `(0, 0, 0, 0)` in every channel, not just alpha.
    let bg = (0x10, 0x20, 0x30);
    let (width, height) = (50, 24);
    let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let px_at = |x: usize, y: usize| &buf[(y * width as usize + x) * 4..(y * width as usize + x) * 4 + 4];
    assert_eq!(px_at(0, 0), [0, 0, 0, 0], "top-left corner pixel must be fully zeroed (premultiplied transparent), not just alpha");
    assert_eq!(px_at(width as usize - 1, 0), [0, 0, 0, 0], "top-right corner pixel must be fully zeroed (premultiplied transparent), not just alpha");
}

#[test]
fn round_corners_false_leaves_the_top_corners_square() {
    let bg = (0x10, 0x20, 0x30);
    let (width, height) = (50, 24);
    let buf = render_titlebar(width, height, "", bg, (0xff, 0xff, 0xff), false, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    assert_eq!(alpha_at(0, 0), 255, "top-left corner should stay square when round_corners is false");
    assert_eq!(alpha_at(width as usize - 1, 0), 255, "top-right corner should stay square when round_corners is false");
}

#[test]
fn brighten_lightens_every_channel_including_a_saturated_one() {
    let (r, g, b) = brighten((0x28, 0xc8, 0x00));
    assert!(r > 0x28, "already-bright channel must still move toward white, not stay put");
    assert!(g > 0xc8);
    assert!(b > 0x00, "a fully-unsaturated (0) channel must still brighten, not stay clamped at 0");
}

#[test]
fn hovering_the_close_button_brightens_only_that_dot() {
    // The actual feature this is for: hovering one button must change
    // *that* button's colour and leave the other two exactly as they
    // were - not brighten all three, and not brighten the wrong one.
    let (width, height) = (200u32, srdwm_core::TITLEBAR_HEIGHT);
    let bg = (0x2e, 0x34, 0x40);
    let fg = (0xec, 0xef, 0xf4);
    let plain = render_titlebar(width, height, "", bg, fg, true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let close_hovered = render_titlebar(width, height, "", bg, fg, true, CORNER_RADIUS, 0, true, Some((srdwm_core::TitlebarHit::Close, 255)), false, false, false, None, true, false);
    let frame = srdwm_core::Rect::new(0, 0, width, height);
    let (w, h) = (width as usize, height as usize);
    let margin = srdwm_core::BUTTON_CLUSTER_MARGIN as usize;
    let close_box = button_box(w, h, margin, false, BUTTON_MARGIN);
    let minimize_box = button_box(w, h, margin + srdwm_core::BUTTON_PITCH as usize * 2, false, BUTTON_MARGIN);
    let center_px = |buf: &[u8], (x0, y0, x1, y1): (i32, i32, i32, i32)| {
        let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
        let i = (cy as usize * w + cx as usize) * 4;
        buf[i..i + 4].to_vec()
    };
    assert_ne!(center_px(&plain, close_box), center_px(&close_hovered, close_box), "hovering close must actually change its own dot's colour");
    assert_eq!(center_px(&plain, minimize_box), center_px(&close_hovered, minimize_box), "hovering close must not also change the minimize dot");
    // Sanity: hit_test must actually route a click at this same centre
    // point to Close, or this test would be checking a hover state
    // that a real pointer could never reach in the first place.
    let (cx, cy) = ((close_box.0 + close_box.2) / 2, (close_box.1 + close_box.3) / 2);
    assert_eq!(srdwm_core::ResizeEdge::hit_test(frame, cx, cy, true, 0, srdwm_core::RESIZE_MARGIN, false, None, false), Some(srdwm_core::TitlebarHit::Close));
}

#[test]
fn button_dot_has_a_glossy_highlight_toward_the_upper_left_and_shadow_toward_the_lower_right() {
    // The actual visual change requested: a flat-filled dot read as
    // noticeably flatter than real macOS's own traffic lights (see
    // `glossy_shade`'s own doc comment, referenced against a real
    // screenshot). This locks in the shape of that gradient, not just
    // that pixels differ from the center - upper-left must be
    // brighter than center, lower-right must be darker, matching a
    // light source up-and-to-the-left.
    let (width, height) = (200u32, srdwm_core::TITLEBAR_HEIGHT);
    let bg = (0x2e, 0x34, 0x40);
    let fg = (0xec, 0xef, 0xf4);
    let buf = render_titlebar(width, height, "", bg, fg, true, CORNER_RADIUS, 0, true, None, false, false, false, None, true, false);
    let (w, h) = (width as usize, height as usize);
    let close_box = button_box(w, h, srdwm_core::BUTTON_CLUSTER_MARGIN as usize, false, BUTTON_MARGIN);
    let (cx, cy) = ((close_box.0 + close_box.2) / 2, (close_box.1 + close_box.3) / 2);
    let radius = ((close_box.2 - close_box.0).min(close_box.3 - close_box.1) as f32 / 2.0) * 0.6;
    let px = |x: i32, y: i32| -> u32 {
        let i = (y as usize * w + x as usize) * 4;
        // BGRA - sum the colour channels, ignore alpha (always 255
        // here), so "brighter"/"darker" reads as a plain luma proxy.
        buf[i] as u32 + buf[i + 1] as u32 + buf[i + 2] as u32
    };
    let center = px(cx, cy);
    let highlight = px(cx - radius.round() as i32, cy - radius.round() as i32);
    let shadow = px(cx + radius.round() as i32, cy + radius.round() as i32);
    assert!(highlight > center, "upper-left of the dot ({highlight}) should be brighter than its centre ({center})");
    assert!(shadow < center, "lower-right of the dot ({shadow}) should be darker than its centre ({center})");
}

#[test]
fn border_top_rounds_its_own_top_corners_to_match_the_titlebar() {
    // Regression coverage for the "not all window borders are rounded"
    // report: a bordered window's titlebar used to render with
    // `round_corners = false` specifically to avoid clashing with this
    // strip's square corners. Now that this strip rounds too, that
    // workaround is gone (`render_titlebar` is always called with
    // `true`) - this just confirms the strip actually does what that
    // change now depends on.
    let color = (0x40, 0x50, 0x60);
    let (width, thickness) = (60, 2);
    let buf = render_border_top(width, thickness, color, CORNER_RADIUS);
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    assert_eq!(alpha_at(0, 0), 0, "top-left corner pixel should be clipped");
    assert_eq!(alpha_at(width as usize - 1, 0), 0, "top-right corner pixel should be clipped");
    // The strip is only `thickness` pixels tall, but its curve is the
    // titlebar's own radius continued outward (`radius + thickness`,
    // see `render_border_top`) - so the horizontal centre, well clear
    // of either corner's cut zone, must stay opaque regardless of how
    // much taller than the strip that combined radius is.
    assert_eq!(alpha_at(width as usize / 2, thickness as usize - 1), 255, "centre of the strip must stay opaque");
}

#[test]
fn border_top_and_titlebar_corners_meet_without_a_seam() {
    // Regression coverage for a *different* bug than the one this test
    // used to check (see git history for the old
    // `border_top_corner_curve_matches_the_titlebars_larger_radius`):
    // an earlier version of this pair rounded the border strip to
    // `radius + thickness` while the titlebar rounded to plain
    // `radius` - two circles sharing a centre but with *different*
    // radii, which do not meet smoothly at any boundary between them.
    // Confirmed live, screenshotted at actual render resolution: a
    // hard stepped notch right where a decorated window's border met
    // its titlebar, not a continuous curve. Both now draw their own
    // slice of the exact same circle (`round_top_corners`'s own doc
    // comment has the full geometry) - this checks that promise
    // directly, by rendering both real bitmaps with the same
    // parameters a live window actually uses and comparing the alpha
    // at the border's last row against the titlebar's first row,
    // immediately below it.
    let color = (0x40, 0x50, 0x60);
    let (width, thickness, radius) = (60, 4, 6);
    let border = render_border_top(width, thickness, color, radius);
    let titlebar = render_titlebar(width, 24, "", color, (0xff, 0xff, 0xff), true, radius, thickness, true, None, false, false, false, None, true, false);
    let border_alpha_at = |x: usize| border[((thickness as usize - 1) * width as usize + x) * 4 + 3];
    let titlebar_alpha_at = |xt: usize| titlebar[xt * 4 + 3];
    // `x` below is the shared *global* column - distance from the true
    // left corner tip, in the border strip's own coordinate frame (its
    // column 0 is that tip). The titlebar's own buffer starts `thickness`
    // columns further in (its column 0 sits at global column `thickness`,
    // not 0 - a border strip is wider than the titlebar it sits above by
    // `thickness` on each side, same as `round_top_corners`'s own
    // `center_col` doc comment already says for the vertical case), so
    // the titlebar-local column that corresponds to a given global column
    // `x` is `x - thickness`, not `x` itself. Comparing raw index `x` on
    // both sides used to "pass" here for the wrong reason: both curves
    // used the same (incorrectly) unshifted centre column, so indexing
    // them identically happened to compare matching *formula* output
    // without the two indices actually referring to the same real screen
    // column - fixing that shared centre (`round_top_corners`'s own
    // `center_col` parameter) is what surfaced this test needing the same
    // correction.
    //
    // Every column across the curve's actual reach, not just one sample
    // point - a seam bug shows up as a jump at some columns and not
    // others, so checking only the corner pixel or only the centre could
    // miss it entirely, the same way the original bug slipped past the
    // test above it for months. `< 90` catches any regression back toward
    // a visibly stepped seam without demanding more precision than two
    // 1px-apart raster buffers a single row apart can give.
    for x in thickness as usize..radius as usize + 2 {
        let xt = x - thickness as usize;
        let (b, t) = (border_alpha_at(x), titlebar_alpha_at(xt));
        let jump = (b as i32 - t as i32).abs();
        assert!(jump < 90, "global column {x} (titlebar column {xt}): border's last row (alpha={b}) and titlebar's first row (alpha={t}) must be close, not a sharp seam (jump={jump})");
    }
    // Past the tip's unavoidable steepness (the first two global columns
    // above), the curve should be genuinely, near-exactly continuous --
    // both rows fully opaque by then for this radius/thickness, not just
    // "close enough".
    for x in (thickness as usize + 2)..radius as usize + 2 {
        let xt = x - thickness as usize;
        let (b, t) = (border_alpha_at(x), titlebar_alpha_at(xt));
        let jump = (b as i32 - t as i32).abs();
        assert!(jump <= 2, "global column {x} (titlebar column {xt}): past the corner tip the seam should be essentially exact, not just under the looser tip tolerance (jump={jump})");
    }
}

#[test]
fn border_top_visible_rows_decorated_shows_the_whole_taller_buffer() {
    let (row0, rows, shift) = border_top_visible_rows(true, 4, 11);
    assert_eq!((row0, rows, shift), (0, 11, 0), "decorated: full max(border_width, radius) buffer, unshifted");
}

#[test]
fn border_top_visible_rows_undecorated_crops_to_just_the_nominal_thickness() {
    // The actual regression this exists for: reported live as a
    // border-coloured wedge cut into a real undecorated Firefox
    // window's top-left corner, confirmed via a real screenshot to be
    // neither Firefox's own rendering nor the content-mask feature --
    // this buffer's own titlebar-band-only extra rows, painted
    // straight onto real content instead, were the only remaining
    // source.
    let (row0, rows, shift) = border_top_visible_rows(false, 4, 11);
    assert_eq!((row0, rows, shift), (0, 4, 0), "undecorated: cropped to exactly border_width rows, still starting at row 0 (this strip grows downward)");
}

#[test]
fn border_top_visible_rows_radius_no_bigger_than_border_is_a_no_op_either_way() {
    // When the buffer was never grown taller than `border_width` in
    // the first place (radius <= border_width), decorated and
    // undecorated must agree - there are no "extra" rows to disagree
    // about.
    assert_eq!(border_top_visible_rows(true, 6, 4), border_top_visible_rows(false, 6, 4));
}

#[test]
fn border_bottom_visible_rows_decorated_shows_the_whole_taller_buffer_shifted_up() {
    let (row0, rows, shift) = border_bottom_visible_rows(true, 4, 11);
    assert_eq!((row0, rows, shift), (0, 11, 7), "decorated: full buffer, shifted up by extra = max(4,11) - 4 = 7");
}

#[test]
fn border_bottom_visible_rows_undecorated_crops_to_the_buffers_last_rows_unshifted() {
    // Same real bug as the top strip, confirmed on the same Firefox
    // window's bottom-left corner via a real screenshot - this strip
    // grows *upward* into content instead of downward, so the safe
    // rows are the buffer's *last* `border_width` of them (starting at
    // `row0 = extra`), not its first, and no position shift is needed
    // once the extra rows themselves are never drawn.
    let (row0, rows, shift) = border_bottom_visible_rows(false, 4, 11);
    assert_eq!((row0, rows, shift), (7, 4, 0), "undecorated: last border_width rows only, starting past the 7-row extra, unshifted");
}

#[test]
fn border_bottom_visible_rows_radius_no_bigger_than_border_is_a_no_op_either_way() {
    assert_eq!(border_bottom_visible_rows(true, 6, 4), border_bottom_visible_rows(false, 6, 4));
}

#[test]
fn border_top_extra_rows_are_transparent_outside_the_corners() {
    // `render_border_bottom`'s mirror - see
    // `border_bottom_extra_rows_are_transparent_outside_the_corners`'s
    // doc comment for the full story (this is the same real,
    // live-confirmed bug, the top-strip half of the pair).
    let color = (0x40, 0x50, 0x60);
    let (width, thickness, radius) = (60, 2, 6);
    let buf = render_border_top(width, thickness, color, radius);
    let height = (thickness as usize).max(radius as usize);
    assert_eq!(buf.len(), width as usize * height * 4, "buffer must actually be the taller max(thickness, radius) height");
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    // The strip's real thickness sits at the *top* here (rows grow
    // downward into content, the mirror of the bottom strip growing
    // upward) - rows 0..thickness must still be fully opaque in the
    // middle, exactly as before this fix.
    for y in 0..thickness as usize {
        assert_eq!(alpha_at(width as usize / 2, y), 255, "row {y}: within the strip's real thickness, the middle column must stay opaque");
    }
    for y in thickness as usize..height {
        assert_eq!(alpha_at(width as usize / 2, y), 0, "row {y}: middle column of an extra row must be transparent, not solid border colour");
    }
}

#[test]
fn border_top_curve_actually_closes_within_the_side_strips_own_width() {
    // The actual, directly-observable bug this whole fix is for:
    // confirmed live via pixel-level inspection of a real screenshot
    // (not just reasoned about) that the horizontal top border segment
    // only became opaque some ~20 columns in from the corner while the
    // *flat, curve-blind* left/right strip only ever covers its own
    // `border_width` columns - so with `radius` meaningfully larger
    // than `border_width`, there was a real gap of bare background
    // between them, at exactly the column range a real vertical border
    // strip occupies. `border_width` here matches `thickness` (as every
    // real call site does - both come from the same `w.border_width`),
    // not an arbitrary different value: `corners::carve_inner_corner_
    // pixel`'s own inner-ring cut is sized from `thickness`, so a test
    // that fed it a different, unrelated "real side strip width" would
    // no longer be testing this compositor's own real geometry at all.
    //
    // `> 180`, not `== 255`: the ring now has smooth antialiasing on
    // *both* its outer and inner edges (see `carve_inner_corner_pixel`'s
    // own doc comment for why the inner one is new) - the exact corner
    // this samples (right at the ring's own width, on its very last row)
    // sits close enough to both transition bands to be genuinely, by
    // design, a little short of fully opaque, the same way the outer
    // curve's own edge already was before this fix existed. `> 180`
    // catches the real regression this test exists for - the curve
    // never reaching this column at all (alpha near 0, a visible gap) --
    // without demanding more precision than two overlapping antialiased
    // edges can give at their closest approach.
    let color = (0x40, 0x50, 0x60);
    let (width, thickness, radius) = (60u32, 3u32, 6u32);
    let buf = render_border_top(width, thickness, color, radius);
    let height = (thickness as usize).max(radius as usize);
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    let alpha = alpha_at(thickness as usize - 1, height - 1);
    assert!(alpha > 180, "the side strip's own rightmost column must be visibly covered by the curve at the buffer's last row, not left as a gap (alpha={alpha})");
}

#[test]
fn border_bottom_rounds_its_own_bottom_corners() {
    // `thickness` (2) < `CORNER_RADIUS` (6) here, same as the live
    // theme defaults (border width 4, radius 6) - so the buffer is
    // taller than `thickness`, and the true tip (fully clipped corner)
    // sits at the buffer's own *last* row, not `thickness - 1` (see
    // `render_border_bottom`'s doc comment for why the buffer grows at
    // all).
    let color = (0x40, 0x50, 0x60);
    let (width, thickness) = (60, 2);
    let buf = render_border_bottom(width, thickness, color, CORNER_RADIUS);
    let height = (thickness as usize).max(CORNER_RADIUS as usize);
    assert_eq!(buf.len(), width as usize * height * 4, "buffer must actually be the taller max(thickness, radius) height");
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    let tip_row = height - 1;
    assert_eq!(alpha_at(0, tip_row), 0, "bottom-left corner pixel should be clipped");
    assert_eq!(alpha_at(width as usize - 1, tip_row), 0, "bottom-right corner pixel should be clipped");
    assert_eq!(alpha_at(width as usize / 2, tip_row), 255, "centre of the strip must stay opaque even at the tip row");
}

#[test]
fn border_bottom_extra_rows_are_transparent_outside_the_corners() {
    // Regression coverage for the real bug this pair of functions was
    // fixed for: with `radius > thickness`, the strip's own curve
    // didn't reach far enough to meet the (curve-blind, flat) side
    // strips, leaving a wedge of bare background between them --
    // confirmed live via pixel-level inspection of a real screenshot,
    // not just reasoned about. The fix grows the buffer to
    // `max(thickness, radius)` so the curve has room to fully resolve,
    // but the *extra* rows (above the original `thickness`, since this
    // strip grows upward into content - see `render_border_bottom`'s
    // doc comment) must stay transparent in the middle (non-corner)
    // columns, or they'd paint a solid border-coloured bar across
    // whatever the content actually owns there.
    let color = (0x40, 0x50, 0x60);
    let (width, thickness, radius) = (60, 2, 6);
    let buf = render_border_bottom(width, thickness, color, radius);
    let height = (thickness as usize).max(radius as usize);
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    for y in 0..height - thickness as usize {
        assert_eq!(alpha_at(width as usize / 2, y), 0, "row {y}: middle column of an extra row must be transparent, not solid border colour");
    }
    // The original `thickness` rows (now at the *bottom* of the taller
    // buffer) must still be the strip's real, fully opaque body in the
    // middle - this fix must not have eaten into the strip's own
    // legitimate thickness.
    for y in height - thickness as usize..height {
        assert_eq!(alpha_at(width as usize / 2, y), 255, "row {y}: within the strip's real thickness, the middle column must stay opaque");
    }
}

#[test]
fn context_menu_is_one_row_tall_per_item() {
    let items = [("Minimize", false), ("Maximize", false), ("Always on Top", false), ("Close", false)];
    let buf = render_context_menu(160, 28, &items, (0x2e, 0x34, 0x40), (0xff, 0xff, 0xff), (0x4c, 0x56, 0x6a), (0x10, 0x10, 0x10));
    assert_eq!(buf.len(), 160 * (28 * 4) * 4);
}

#[test]
fn context_menu_highlighted_row_has_a_different_background_than_the_rest() {
    let items = [("Minimize", false), ("Close", true)];
    let bg = (0x2e, 0x34, 0x40);
    let highlight = (0x4c, 0x56, 0x6a);
    let buf = render_context_menu(160, 28, &items, bg, (0xff, 0xff, 0xff), highlight, (0x10, 0x10, 0x10));
    let width = 160usize;
    // Sample a background pixel from each row, away from the text/border.
    let px_at = |x: usize, y: usize| -> [u8; 3] {
        let i = (y * width + x) * 4;
        [buf[i + 2], buf[i + 1], buf[i]] // BGRA -> RGB
    };
    assert_eq!(px_at(100, 5), [bg.0, bg.1, bg.2], "row 0 (not highlighted) should use bg");
    assert_eq!(px_at(100, 33), [highlight.0, highlight.1, highlight.2], "row 1 (highlighted) should use highlight_bg");
}

#[test]
fn context_menu_border_is_opaque_at_every_edge() {
    let items = [("Close", false)];
    let buf = render_context_menu(100, 28, &items, (0, 0, 0), (0xff, 0xff, 0xff), (0, 0, 0), (0x99, 0x99, 0x99));
    let alpha_at = |x: usize, y: usize| buf[(y * 100 + x) * 4 + 3];
    assert_eq!(alpha_at(0, 0), 255);
    assert_eq!(alpha_at(99, 0), 255);
    assert_eq!(alpha_at(0, 27), 255);
    assert_eq!(alpha_at(99, 27), 255);
}

#[test]
fn snap_flyout_is_sized_for_a_full_grid_of_labels() {
    let labels = ["Left Half", "Right Half", "Top Left", "Top Right", "Bottom Left", "Bottom Right"];
    let buf = render_snap_flyout(3, 90, 60, &labels, (0x2e, 0x34, 0x40), (0xff, 0xff, 0xff), (0x10, 0x10, 0x10));
    // 3 columns x 2 rows (6 labels / 3 columns, rounded up).
    assert_eq!(buf.len(), (90 * 3) * (60 * 2) * 4);
}

#[test]
fn snap_flyout_border_is_opaque_at_every_outer_edge() {
    let labels = ["A", "B", "C", "D", "E", "F"];
    let (cell_w, cell_h) = (90, 60);
    let buf = render_snap_flyout(3, cell_w, cell_h, &labels, (0, 0, 0), (0xff, 0xff, 0xff), (0x99, 0x99, 0x99));
    let (width, height) = (cell_w * 3, cell_h * 2);
    let alpha_at = |x: usize, y: usize| buf[(y * width as usize + x) * 4 + 3];
    assert_eq!(alpha_at(0, 0), 255);
    assert_eq!(alpha_at(width as usize - 1, 0), 255);
    assert_eq!(alpha_at(0, height as usize - 1), 255);
    assert_eq!(alpha_at(width as usize - 1, height as usize - 1), 255);
}

#[test]
fn snap_flyout_has_an_internal_grid_line_between_columns() {
    let labels = ["A", "B", "C", "D", "E", "F"];
    let (cell_w, cell_h) = (90, 60);
    let buf = render_snap_flyout(3, cell_w, cell_h, &labels, (0, 0, 0), (0xff, 0xff, 0xff), (0x99, 0x99, 0x99));
    let width = cell_w * 3;
    // The boundary between column 0 and column 1, away from the outer border.
    let idx = (30 * width as usize + cell_w as usize) * 4;
    assert_eq!(buf[idx + 3], 255, "column boundary must be drawn, not just the outer border");
}
