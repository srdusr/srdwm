//! Session lock (`ext-session-lock-v1`).
//!
//! Kept as its own module because the security-relevant invariant spans
//! state, protocol handling *and* rendering: while [`SessionLock::locked`]
//! is set, no client surface other than the lock surface may be drawn or
//! receive input. The render helpers here are the "drawn" half; the input
//! half is the locked-session branches in [`crate::input`], and the focus
//! half is the guard in [`crate::state::CompState::set_keyboard_focus`].

use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker};

use crate::input::dwindow_wl_surface;
use std::collections::{HashMap, HashSet};

use crate::state::CompState;

/// Session-lock runtime state (`ext-session-lock-v1`).
///
/// The security-relevant invariant is that `locked` gates *both* rendering
/// and input: while it is set, no client surface other than a lock surface
/// is drawn or receives events. `pending_confirm` exists because the
/// protocol requires the compositor to confirm the lock only *after* a frame
/// with no client content has actually been presented - confirming earlier
/// would tell the locker "the screen is safe" while the user's windows were
/// still on screen. So `lock()` only stashes the confirmation here, and the
/// render paths call `confirm_lock_if_presented()` once such a frame is out.
///
/// With multiple monitors the locker creates **one lock surface per
/// output**, and the confirmation must wait for *every* output to have both
/// a surface and a presented frame - otherwise a second monitor could still
/// be showing the desktop at the moment the locker is told the session is
/// locked.
#[derive(Default)]
pub(crate) struct SessionLock {
    pub(crate) locked: bool,
    /// Lock surface per output, keyed by `Output::name()`.
    pub(crate) surfaces: HashMap<String, LockSurface>,
    pub(crate) pending_confirm: Option<SessionLocker>,
    /// Outputs that have presented a client-content-free frame since the
    /// lock request, keyed the same way.
    pub(crate) presented: HashSet<String>,
}

impl CompState {
    /// Records that `output` has presented a lock frame, and confirms the
    /// lock once *every* output has done so. Called from both backends right
    /// after a frame is presented. See `SessionLock::pending_confirm`.
    pub(crate) fn confirm_lock_if_presented(&mut self, output: &Output) {
        if !self.lock.locked || self.lock.pending_confirm.is_none() {
            return;
        }
        self.lock.presented.insert(output.name());

        // Every output must be both covered by a lock surface and have shown
        // a frame of it. An output still missing its surface means the locker
        // hasn't got to it yet - keep waiting rather than confirm early.
        let all_covered = self
            .outputs()
            .all(|o| self.lock.surfaces.contains_key(&o.name()) && self.lock.presented.contains(&o.name()));
        if !all_covered {
            return;
        }
        if let Some(confirm) = self.lock.pending_confirm.take() {
            log::info!("session lock: all {} output(s) presented a cleared frame, confirming lock", self.outputs.len());
            confirm.lock();
        }
    }

    /// The lock surface covering `output`, if we are locked and the locker
    /// has created one for it. Used by the render paths and input routing.
    pub(crate) fn lock_surface_for(&self, output: &Output) -> Option<&WlSurface> {
        if !self.lock.locked {
            return None;
        }
        self.lock.surfaces.get(&output.name()).map(|s| s.wl_surface())
    }

    /// Any lock surface at all - used by input routing, which needs a
    /// keyboard-focus target rather than a per-output one.
    pub(crate) fn any_lock_surface(&self) -> Option<&WlSurface> {
        if !self.lock.locked {
            return None;
        }
        self.lock.surfaces.values().next().map(|s| s.wl_surface())
    }
}

impl SessionLockHandler for CompState {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        log::info!("session lock: locking");
        self.lock.locked = true;
        self.lock.pending_confirm = Some(confirmation);
        // Drop keyboard focus off whatever client had it immediately, so no
        // keystroke can reach a normal client in the window between the lock
        // request and the lock surface being mapped. Focus moves to the lock
        // surface in `new_surface` once it exists.
        self.set_keyboard_focus(None);
    }

    fn unlock(&mut self) {
        log::info!("session lock: unlocking");
        self.lock.locked = false;
        self.lock.surfaces.clear();
        self.lock.presented.clear();
        self.lock.pending_confirm = None;
        // udev backend only: the lock scene was rendered through the same
        // per-head damage tracker as the normal desktop (see
        // `render_udev_frame`), always with a forced full redraw, so its
        // element-state history now reflects the lock surface rather than
        // whatever was on screen before locking. Reset each head's buffer
        // ages so the next normal-scene render is a full redraw too,
        // instead of asking the tracker to diff the desktop against a
        // now-irrelevant lock-scene history.
        if let Some(udev) = self.udev.as_mut() {
            for head in &mut udev.heads {
                head.ages = [0, 0];
            }
        }
        // Hand focus back to whatever srdwm considers the focused window.
        let surface = self
            .wm
            .borrow()
            .focused_id()
            .and_then(|id| self.id_to_window.get(&id).cloned())
            .and_then(|w| dwindow_wl_surface(&w));
        self.set_keyboard_focus(surface);
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        // The locker creates one surface per output and names which; size it
        // to that output specifically, since monitors differ in resolution.
        let Some(entry) = self.output_for_wl(&wl_output) else {
            log::warn!("session lock: lock surface for an output we don't drive; ignoring");
            return;
        };
        let name = entry.output.name();
        let size = entry.size();
        surface.with_pending_state(|state| {
            state.size = Some((size.w as u32, size.h as u32).into());
        });
        surface.send_configure();
        // Store before focusing: `set_keyboard_focus`'s locked-session guard
        // checks the focus target *against* the stored lock surfaces, so
        // setting it afterwards would make the guard reject this very
        // surface.
        let wl_surface = surface.wl_surface().clone();
        self.lock.surfaces.insert(name, surface);
        self.set_keyboard_focus(Some(wl_surface));
    }
}

/// Render elements for a locked session: the lock surface alone, at the
/// output origin. Deliberately *not* built from the `Space` or the
/// `LayerMap` - while locked, nothing else may reach the screen, so the
/// caller pairs this with an opaque black clear color. Shared by both
/// backends (winit and udev), which differ only in their renderer type.
/// Takes the surface rather than `&CompState` so the udev backend can call
/// it while already holding a `&mut` borrow of its own `UdevOutput`.
pub(crate) fn lock_render_elements<R>(lock_surface: Option<&WlSurface>, renderer: &mut R) -> Vec<WaylandSurfaceRenderElement<R>>
where
    R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportAll,
    R::TextureId: Clone + 'static,
{
    let Some(surface) = lock_surface else { return Vec::new() };
    render_elements_from_surface_tree(renderer, surface, (0, 0), 1.0, 1.0, Kind::Unspecified)
}

/// Frame callbacks for the lock surface. Without these the locker never
/// gets to draw a second frame - no cursor blink, no password-dot
/// feedback, no failed-attempt shake.
pub(crate) fn send_lock_frame(lock_surface: Option<&WlSurface>, output: &Output, time: std::time::Duration) {
    if let Some(surface) = lock_surface {
        smithay::desktop::utils::send_frames_surface_tree(surface, output, time, None, |_, _| Some(output.clone()));
    }
}
