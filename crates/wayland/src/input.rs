//! Input routing: keyboard, pointer, and what "focus" means.
//!
//! Shared by both backends - smithay delivers keyboard/pointer events
//! through generic `InputBackend` traits, so the precise keybinding matching
//! and titlebar hit-testing exist once here and are called from the winit
//! backend ([`crate::winit`]) and the libinput/udev one ([`crate::udev`])
//! alike.
//!
//! Every function that routes an event checks the session lock first: while
//! locked, input goes to the lock surface and nowhere else. See
//! [`crate::lock`].
//!
//! Split by concern, one file per input-event kind, matching niri's own
//! per-grab-kind module boundaries: [`layers`] (layer-shell hit-testing and
//! the layer-driven maximize geometry), [`focus`] (focusing/raising/closing
//! a window - needed by every other kind below regardless of what
//! triggered the change), [`pointer`] (motion, button, cursor shape),
//! [`keyboard`] (key events and keysym/modifier translation), [`gestures`]
//! (workspace scroll and touchpad swipe). `notify_idle_activity` and
//! [`DRAG_MODIFIER`] stay here, at the root, since every one of those
//! modules needs at least one of them.

mod focus;
mod gestures;
mod keyboard;
mod layers;
mod pointer;

use std::time::{Duration, Instant};

use smithay::utils::{Logical, Point};

use srdwm_core::Modifiers;

use crate::state::CompState;

pub(crate) use focus::{close_dwindow, dwindow_wl_surface, focus_window, raise_in_space, sync_keyboard_focus};
pub(crate) use gestures::{handle_gesture_swipe_begin, handle_gesture_swipe_end, handle_gesture_swipe_update, handle_workspace_scroll};
pub(crate) use keyboard::handle_keyboard_key_event;
pub(crate) use layers::maximize_geometry_for;
pub(crate) use pointer::{handle_pointer_button, handle_pointer_position};

/// Modifier that turns a drag anywhere in a window into move/resize.
/// Matches the `SUPER` the shipped and ported configs use for
/// `bindm ... movewindow` / `resizewindow`.
pub(super) const DRAG_MODIFIER: Modifiers = Modifiers::SUPER;

pub(crate) fn last_pointer_pos(state: &CompState) -> Point<f64, Logical> {
    state.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default()
}

/// `ext_idle_notify_v1`'s whole job is answering "has the user touched an
/// input device recently" - `IdleNotifierState` does the actual timer
/// bookkeeping (see its own doc comment), this just has to be called from
/// every real input path, deliberately including while the session is
/// locked: idle activity is about the seat, not about which surface (if
/// any) an event ends up delivered to, and a lock daemon watching this
/// protocol to decide when to re-dim/re-lock still needs to see real
/// movement even though nothing else happens with it at a locked screen.
///
/// Throttled to once per 250ms: `notify_activity` removes and re-inserts a
/// calloop timer for every live notification, every call, with no
/// throttling of its own - fine at keypress/click frequency, but pointer
/// motion can fire far more often than that during a drag, and idle
/// timeouts are measured in minutes, not milliseconds, so nothing about
/// idle detection needs - or can even perceive - finer resolution than
/// this. The same class of hot-path-on-every-motion-event cost that made
/// this session's earlier diagnostic logging a real, measured regression
/// (see `docs/IMPLEMENTATION_STATUS.md`), just cheap enough here (in-memory
/// bookkeeping, not synchronous I/O) that throttling rather than removing
/// it outright is the right amount of caution.
pub(super) fn notify_idle_activity(state: &mut CompState) {
    const THROTTLE: Duration = Duration::from_millis(250);
    let now = Instant::now();
    if state.last_idle_notify.is_some_and(|last| now.duration_since(last) < THROTTLE) {
        return;
    }
    state.last_idle_notify = Some(now);
    let seat = state.seat.clone();
    state.idle_notifier_state.notify_activity(&seat);
}
