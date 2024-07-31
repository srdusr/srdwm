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
