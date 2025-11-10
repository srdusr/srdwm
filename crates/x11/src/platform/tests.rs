    use super::*;

    /// Builds a flattened `GetModifierMappingReply.keycodes`-shaped slice:
    /// 8 slots (Shift, Lock, Control, Mod1..Mod5) of `per` keycodes each,
    /// zero-padded, with `assignments` placing one real keycode into
    /// specific slots.
    fn modmap(per: usize, assignments: &[(usize, u8)]) -> Vec<u8> {
        let mut v = vec![0u8; per * 8];
        for &(slot, kc) in assignments {
            v[slot * per] = kc;
        }
        v
    }

    #[test]
    fn finds_numlock_on_mod2_the_common_case() {
        let keycodes = modmap(2, &[(4, 77)]); // slot 4 == Mod2
        assert_eq!(modmask_for_keycode_in_mod_slots(77, 2, &keycodes), ModMask::M2);
    }

    #[test]
    fn finds_numlock_on_mod5_an_uncommon_but_real_layout() {
        let keycodes = modmap(2, &[(7, 90)]); // slot 7 == Mod5
        assert_eq!(modmask_for_keycode_in_mod_slots(90, 2, &keycodes), ModMask::M5);
    }

    #[test]
    fn ignores_the_keycode_if_it_only_appears_in_shift_lock_or_control() {
        // A keycode bound to Lock (e.g. Caps Lock's own keycode) must never
        // be mistaken for Num Lock - only slots 3..8 (Mod1..Mod5) count.
        let keycodes = modmap(2, &[(1, 66)]); // slot 1 == Lock
        assert_eq!(modmask_for_keycode_in_mod_slots(66, 2, &keycodes), ModMask::from(0u16));
    }

    #[test]
    fn keycode_zero_never_matches_even_if_a_slot_is_unpadded_zero() {
        // Unused modifier slots are zero-padded, so keycode 0 must never
        // resolve to a mask - otherwise a keyboard with no Num Lock key at
        // all would spuriously "find" it in the first empty slot.
        let keycodes = modmap(2, &[]);
        assert_eq!(modmask_for_keycode_in_mod_slots(0, 2, &keycodes), ModMask::from(0u16));
    }

    #[test]
    fn no_match_anywhere_returns_empty_mask() {
        let keycodes = modmap(2, &[(3, 50)]);
        assert_eq!(modmask_for_keycode_in_mod_slots(99, 2, &keycodes), ModMask::from(0u16));
    }

    fn top_strut(height: u32, start_x: i32, end_x: i32) -> Strut {
        Strut { top: height, top_start_x: start_x, top_end_x: end_x, ..Strut::default() }
    }

    #[test]
    fn a_top_bar_shrinks_the_monitor_it_actually_spans() {
        let full = Rect::new(0, 0, 1920, 1080);
        let strut = top_strut(32, 0, 1920);
        let usable = usable_rect(full, (1920, 1080), std::iter::once(strut));
        assert_eq!(usable, Rect::new(0, 32, 1920, 1048));
    }

    #[test]
    fn a_bar_confined_to_a_different_monitor_leaves_this_one_alone() {
        // A 1920-wide bar sitting entirely over the first monitor
        // (x 0..1920) must not shrink a second monitor placed to its
        // right (x 1920..3840) - struts are screen-global, not
        // monitor-relative, so this is the only thing that tells them
        // apart.
        let second_monitor = Rect::new(1920, 0, 1920, 1080);
        let strut = top_strut(32, 0, 1920);
        let usable = usable_rect(second_monitor, (3840, 1080), std::iter::once(strut));
        assert_eq!(usable, second_monitor);
    }

    #[test]
    fn a_bottom_strut_is_measured_from_the_screen_bottom_not_the_monitor() {
        let full = Rect::new(0, 0, 1920, 1080);
        let strut = Strut { bottom: 40, bottom_start_x: 0, bottom_end_x: 1920, ..Strut::default() };
        let usable = usable_rect(full, (1920, 1080), std::iter::once(strut));
        assert_eq!(usable, Rect::new(0, 0, 1920, 1040));
    }

    #[test]
    fn a_strut_larger_than_the_monitor_clamps_to_zero_size_not_a_negative_one() {
        let full = Rect::new(0, 0, 800, 600);
        let strut = top_strut(1000, 0, 800);
        let usable = usable_rect(full, (800, 600), std::iter::once(strut));
        assert_eq!(usable.height, 0);
        assert!(usable.width > 0, "only the top edge was reserved, the sides must stay untouched");
    }

    #[test]
    fn no_struts_at_all_leaves_the_monitor_exactly_as_reported() {
        let full = Rect::new(100, 50, 1024, 768);
        let usable = usable_rect(full, (1920, 1080), std::iter::empty());
        assert_eq!(usable, full);
    }

    #[test]
    fn zero_border_width_leaves_the_frame_geometry_unchanged() {
        let geom = Rect::new(0, 0, 1920, 1080);
        assert_eq!(frame_geometry_for(geom, 0), (0, 0, 1920, 1080));
    }

    #[test]
    fn a_real_border_width_keeps_the_true_footprint_equal_to_the_requested_geometry() {
        // Live-reported bug: a maximized window with a native X11 border
        // sat 2*border_width pixels past the right/bottom edges, since the
        // border draws outside the configured width/height. The true
        // on-screen footprint - configured origin minus the border on the
        // near side, configured size plus the border on both sides - must
        // reproduce the original geometry exactly.
        let geom = Rect::new(100, 50, 1920, 1080);
        let (x, y, w, h) = frame_geometry_for(geom, 4);
        assert_eq!((x - 4, y - 4, w + 8, h + 8), (geom.x, geom.y, geom.width, geom.height));
    }

    #[test]
    fn a_maximized_window_no_longer_overhangs_the_monitor_with_a_real_border() {
        // The exact live scenario: a window maximized to fill the whole
        // monitor must not visually extend past it just because it also
        // has a nonzero border.
        let monitor = Rect::new(0, 0, 1920, 1080);
        let (x, y, w, h) = frame_geometry_for(monitor, 4);
        assert_eq!(x - 4, 0, "left edge (including border) must not sit left of the monitor");
        assert_eq!(y - 4, 0, "top edge (including border) must not sit above the monitor");
        assert_eq!(x + w as i32 + 4, 1920, "right edge (including border) must not overhang the monitor");
        assert_eq!(y + h as i32 + 4, 1080, "bottom edge (including border) must not overhang the monitor");
    }
