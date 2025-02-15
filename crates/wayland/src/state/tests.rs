    use super::*;

    #[test]
    fn focused_window_keeps_its_configured_colour() {
        // The dim factor is irrelevant when focused - passing an
        // obviously-wrong one here doubles as proof the early return never
        // even looks at it.
        assert_eq!(effective_border_color((136, 192, 208), true, 0.0), (136, 192, 208));
    }

    #[test]
    fn unfocused_window_is_dimmed_but_still_recognisably_that_colour() {
        let dimmed = effective_border_color((136, 192, 208), false, 0.35);
        // Dimmer in every channel...
        assert!(dimmed.0 < 136 && dimmed.1 < 192 && dimmed.2 < 208);
        // ...but not black, and the channels' relative order is preserved
        // (still "bluish", not just "gray") so a per-window colour set via
        // a rule stays distinguishable from another window's even while
        // unfocused.
        assert!(dimmed.0 > 0 || dimmed.1 > 0 || dimmed.2 > 0);
        assert!(dimmed.2 >= dimmed.1 && dimmed.1 >= dimmed.0);
    }

    #[test]
    fn inactive_dim_factor_is_actually_configurable() {
        // `theme.decorations.border.inactive_dim` - `1.0` keeps an
        // unfocused border identical to focused, `0.0` removes it entirely
        // (fully black, matching every channel scaled to zero).
        assert_eq!(effective_border_color((136, 192, 208), false, 1.0), (136, 192, 208));
        assert_eq!(effective_border_color((136, 192, 208), false, 0.0), (0, 0, 0));
    }

    #[test]
    fn window_anim_starts_at_from_and_ends_at_to() {
        let anim = WindowAnim {
            from: srdwm_core::Rect::new(0, 100, 300, 200),
            to: srdwm_core::Rect::new(0, 0, 300, 200),
            start: Instant::now(),
            duration: Duration::from_millis(200),
        };
        assert_eq!(anim.current_rect(), anim.from);
        assert!(!anim.is_done());
    }

    #[test]
    fn window_anim_is_done_and_settles_exactly_on_to_once_duration_elapses() {
        let anim = WindowAnim {
            from: srdwm_core::Rect::new(0, 100, 300, 200),
            to: srdwm_core::Rect::new(50, 0, 600, 400),
            start: Instant::now() - Duration::from_millis(500),
            duration: Duration::from_millis(200),
        };
        assert!(anim.is_done());
        assert_eq!(anim.current_rect(), anim.to);
    }

    #[test]
    fn window_anim_midway_is_strictly_between_from_and_to_on_every_axis() {
        let anim = WindowAnim {
            from: srdwm_core::Rect::new(0, 200, 200, 100),
            to: srdwm_core::Rect::new(100, 0, 800, 600),
            start: Instant::now() - Duration::from_millis(100),
            duration: Duration::from_millis(200),
        };
        let r = anim.current_rect();
        assert!(r.x > 0 && r.x < 100);
        assert!(r.y > 0 && r.y < 200);
        assert!(r.width > 200 && r.width < 800);
        assert!(r.height > 100 && r.height < 600);
    }
