//! `SUPER`+scroll and touchpad-swipe workspace switching.

use crate::state::CompState;

use super::keyboard::core_modifiers_from_xkb;
use super::{notify_idle_activity, DRAG_MODIFIER};

/// Modifier+scroll cycles workspaces, consuming the event.
///
/// Returns `true` if it handled the scroll, in which case the caller must
/// *not* also forward it to the client. Generic over the input backend for
/// the same reason the keyboard handler is: both backends deliver scroll
/// through smithay's `PointerAxisEvent` trait.
pub(crate) fn handle_workspace_scroll<B, E>(state: &mut CompState, event: &E) -> bool
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::PointerAxisEvent<B>,
{
    use smithay::backend::input::Axis;

    notify_idle_activity(state);
    if state.lock.locked {
        return false;
    }
    let mods = state.seat.get_keyboard().map(|k| core_modifiers_from_xkb(&k.modifier_state()));
    if !mods.is_some_and(|m| m.contains(DRAG_MODIFIER)) {
        return false;
    }
    let Some(v) = event.amount(Axis::Vertical).filter(|v| *v != 0.0) else { return false };
    // Scrolling down (positive) advances, matching `workspace, e+1`.
    switch_workspace_relative(state, v > 0.0)
}

/// Switches to the next (`forward`) or previous workspace in id order,
/// wrapping around, and fires the two follow-up broadcasts a plain
/// `WindowManager::switch_workspace` call alone doesn't cover. The shared
/// body behind every *relative* workspace switch - `SUPER`+scroll above,
/// and a 3+-finger touchpad swipe (`handle_gesture_swipe_end` below) --
/// pulled out here rather than duplicated a second time: both gaps below
/// were found missing for the scroll gesture specifically during this same
/// session, and nothing about either is scroll-only, so a second call site
/// copy-pasting the same steps would have been one missed broadcast away
/// from reintroducing the exact bug that was just fixed once already.
/// Returns `false` (and does nothing) if there are no workspaces at all.
fn switch_workspace_relative(state: &mut CompState, forward: bool) -> bool {
    let mut wm = state.wm.borrow_mut();
    let ids: Vec<_> = wm.workspaces().iter().map(|w| w.id).collect();
    if ids.is_empty() {
        return false;
    }
    let current = ids.iter().position(|&id| id == wm.current_workspace()).unwrap_or(0);
    let next = if forward { (current + 1) % ids.len() } else { (current + ids.len() - 1) % ids.len() };
    wm.switch_workspace(ids[next]);
    drop(wm);
    // Without this, the switch above is invisible: nothing shows or hides
    // a single window for the new workspace until `main.rs`'s `sync()`
    // runs, which only happens when a polled event sets `dirty` - see
    // `srdwm_core::Event::WorkspaceChanged`'s doc comment. Found live-
    // testing the unrelated `ext_workspace_v1` protocol's own `activate`
    // request, which has the identical problem; the scroll gesture had the
    // exact same bug already, just never one anyone traced back this far.
    state.pending.borrow_mut().push(srdwm_core::Event::WorkspaceChanged);
    // Same reasoning as `foreign_toplevel::send_state`'s call sites: without
    // this, a dock's workspace pill only ever tracked switches driven
    // through `ext_workspace_handle_v1.activate` itself, going stale the
    // moment a gesture (or any other non-protocol trigger) changed the
    // active workspace instead.
    crate::workspace::broadcast_active_workspace(state);
    true
}

/// A 3+-finger touchpad swipe just started - resets the running horizontal
/// offset `handle_gesture_swipe_update` accumulates into, or leaves it
/// `None` while the session is locked so a swipe over the lock screen does
/// nothing (matching every other pointer/keyboard path's "locked: no normal
/// handling" rule - see this module's own doc comment).
pub(crate) fn handle_gesture_swipe_begin<B, E>(state: &mut CompState, event: &E)
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::GestureBeginEvent<B>,
{
    notify_idle_activity(state);
    state.gesture_swipe = if state.lock.locked { None } else { Some((event.fingers(), 0.0)) };
}

/// Accumulates one update's worth of horizontal motion into the swipe
/// started by `handle_gesture_swipe_begin` - `delta_x` is relative to the
/// *previous* update, not a running total (see `gesture_swipe`'s own doc
/// comment on `CompState`), so summing here is the only way to know the
/// swipe's real total distance once it ends.
pub(crate) fn handle_gesture_swipe_update<B, E>(state: &mut CompState, event: &E)
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::GestureSwipeUpdateEvent<B>,
{
    if let Some((_, total_dx)) = state.gesture_swipe.as_mut() {
        *total_dx += event.delta_x();
    }
}

/// A touchpad swipe just ended - switches workspace if it was a genuine
/// 3+-finger swipe past `SWIPE_THRESHOLD` and wasn't cancelled (a libinput
/// gesture is marked cancelled when it doesn't resolve to a clean single
/// direction, e.g. the fingers moved back and forth). Below the threshold
/// or below 3 fingers, this does nothing - the same "did you mean it"
/// floor a mis-clicked drag gets elsewhere in this file, and 2-finger
/// motion is already handled as ordinary scroll (`PointerAxis`) rather
/// than reaching here at all on a correctly configured touchpad.
///
/// Deliberately claimed entirely by the compositor rather than forwarded to
/// the focused client, unlike pinch/hold (forwarded as-is in
/// `udev::session`): `wp_pointer_gestures` swipe is specifically the
/// 3/4-finger overview-style gesture, and the handful of desktops that
/// support it at all (GNOME, sway, Hyprland) all reserve it for workspace
/// switching the same way - there is no real client-side consumer to lose
/// by not forwarding it. Swipe left (negative `total_dx`) advances to the
/// next workspace, right goes back, matching macOS's own convention for
/// swiping between spaces.
const SWIPE_THRESHOLD: f64 = 60.0;

pub(crate) fn handle_gesture_swipe_end<B, E>(state: &mut CompState, event: &E)
where
    B: smithay::backend::input::InputBackend,
    E: smithay::backend::input::GestureEndEvent<B>,
{
    let Some((fingers, total_dx)) = state.gesture_swipe.take() else { return };
    if event.cancelled() || fingers < 3 || total_dx.abs() < SWIPE_THRESHOLD {
        return;
    }
    switch_workspace_relative(state, total_dx < 0.0);
}
