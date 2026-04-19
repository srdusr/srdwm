//! `zwlr_virtual_pointer_unstable_v1`: lets a client emulate a physical
//! pointer device - motion, buttons and scroll - through a real Wayland
//! protocol, the same job `zwp_virtual_keyboard_manager_v1` already does
//! for synthetic keystrokes (see `protocols/virtual_keyboard.rs`). Smithay
//! 0.7 ships no helper for this one, same situation as `screencopy.rs` and
//! `output_management.rs`, so the `GlobalDispatch`/`Dispatch` plumbing
//! below is written out by hand against the raw `wayland-protocols-wlr`
//! server bindings, following the same shape those two files already
//! established.
//!
//! Real, scoped gap this closes - not a nice-to-have: `docs/TODO.md`'s
//! "ydotool's `--absolute` is unusable on this machine" entry and the
//! "wl_pointer motion/button coordinates" investigation both trace back to
//! the same root problem, that this compositor had no real protocol path
//! for synthetic pointer input at all. `ydotool`'s own uinput device has
//! no `EV_ABS` capability on this hardware (relative-only, and libinput's
//! pointer-acceleration curve warps even that), which is why every
//! synthetic-click verification this project has ever done needed a
//! fragile corner-clamp-then-walk workaround instead of a precise,
//! reliable placement. A virtual pointer client (a `wlrctl`/custom tool
//! built against this protocol, or a future `ydotool` that speaks it)
//! sidesteps all of that: `motion_absolute` lands exactly where asked, and
//! `motion` (relative) is a raw compositor-space delta with no libinput
//! acceleration applied, since it never touches a uinput device at all.
//!
//! Every request is fed through the exact same `handle_pointer_position`/
//! `handle_pointer_button` entry points a real libinput hardware event
//! goes through (`udev/session.rs::handle_libinput_event`) - a virtual
//! pointer is indistinguishable from a real mouse to every other part of
//! this compositor (hit-testing, drag/resize, focus-follows-mouse, all of
//! it), by construction, rather than a second, easily-drifting code path.
//!
//! Scroll is the one piece that can't reuse an existing entry point: real
//! scroll handling (`udev/session.rs`'s `InputEvent::PointerAxis` arm)
//! reads its values through smithay's `PointerAxisEvent` trait, which is
//! implemented for real backend event types, not something a synthetic
//! caller can construct. Built directly against `AxisFrame` instead (the
//! same builder that trait ultimately feeds into) - accumulated across
//! this protocol's own `axis`/`axis_source`/`axis_stop`/`axis_discrete`
//! requests exactly as the protocol groups them, and committed on `frame`.
//!
//! One real limitation, not silently glossed over: unpinned motion is only
//! ever applied when `CompState::udev` is live (the real-hardware
//! backend). The winit/nested backend has no equivalent multi-monitor
//! `bounds()` to clamp against and no daily-driver use case for synthetic
//! input, so a virtual pointer bound there is accepted (the global still
//! exists, a client's `create_virtual_pointer` still succeeds) but its
//! unpinned motion requests are no-ops - documented here rather than
//! silently dropped with no explanation, matching this codebase's own
//! "degrade honestly" convention elsewhere (`monitor_layout::load`,
//! `icon_theme::find_icon`). Pinned motion (below) has no such limitation
//! - it never touches `udev`/`bounds()` at all, so it works identically
//! on both backends, which is what makes the winit/nested backend a real
//! place to validate it.
//!
//! **Phase 2 of this project's own multi-cursor plan** (see
//! `docs/TODO.md`'s "Multi-cursor Phase 2" entry for the full reasoning):
//! pinning a virtual pointer object to a specific window so its
//! motion/button events reach that window directly, independent of
//! wherever the shared seat's real focus/`pointer_pos` currently is. This
//! is the concrete answer to "an agent could operate one window while the
//! user works another, genuinely simultaneously" - confirmed against
//! smithay 0.7.0's own source that a second `wl_seat` would be invisible
//! to every real client (GTK/Qt/Electron only ever bind the first one
//! advertised), so the fix has to work *through* the one seat every
//! client already binds, not around it.
//!
//! `CompState::set_virtual_pointer_pin` (queued via a `pin_input`/
//! `unpin_input` IPC dispatch, `crates/platform/src/ipc.rs`, and drained
//! the same one-poll-tick-later way `set_output_position` already is)
//! finds every virtual pointer object owned by a given client pid --
//! `Client::get_credentials` - and sets its `pinned_window`. A pid, not
//! an opaque per-object id, is the pinning handle: nothing outside this
//! compositor could ever learn a `zwlr_virtual_pointer_v1` object's own
//! internal id to pass back in, whereas a controlling tool already knows
//! its own pid (`std::process::id()`) for free.
//!
//! A pinned object's `motion`/`motion_absolute`/`button` requests bypass
//! `handle_pointer_position`/`handle_pointer_button` entirely - they
//! never move `pointer_pos`, change focus, or raise the target window.
//! Instead they hand-roll the real `wl_pointer.enter`/`motion`/`button`/
//! `frame`/`leave` wire messages directly against every `WlPointer`
//! resource the target surface's own client has bound
//! (`PointerHandle::client_pointers`, a real smithay-public API for
//! exactly this) - the same "construct the protocol object by hand, this
//! is a narrow case smithay's higher-level seat model wasn't built for"
//! shape this module's own unpinned path already is. From the target
//! client's own point of view this is indistinguishable from an ordinary,
//! correctly-interleaved pointer entering and moving over its surface; the
//! human's real seat, focus and cursor are never touched.

use std::sync::Mutex;

use smithay::backend::input::{Axis, AxisSource};
use smithay::input::pointer::AxisFrame;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_pointer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use smithay::utils::{Coordinate, Logical, Point, SERIAL_COUNTER};
use wayland_protocols_wlr::virtual_pointer::v1::server::zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1};
use wayland_protocols_wlr::virtual_pointer::v1::server::zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1};

use srdwm_core::WindowId;

use crate::elements::window_wl_surface;
use crate::input::{handle_pointer_button, handle_pointer_position, last_pointer_pos};
use crate::state::CompState;

/// The virtual pointer manager global. Held by `CompState` purely to keep
/// the global alive for the compositor's lifetime, same as `ScreencopyState`.
#[derive(Debug)]
pub struct VirtualPointerState {
    _global: smithay::reexports::wayland_server::backend::GlobalId,
}

impl VirtualPointerState {
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrVirtualPointerManagerV1, ()> + 'static,
    {
        Self { _global: dh.create_global::<D, ZwlrVirtualPointerManagerV1, _>(2, ()) }
    }
}

/// State attached to each `zwlr_virtual_pointer_v1`. `output` is only ever
/// set by `create_virtual_pointer_with_output`, and only changes
/// `motion_absolute`'s own mapping (see that handler) - everything else
/// about a virtual pointer is identical regardless of which constructor
/// made it.
#[derive(Debug, Default)]
pub struct VirtualPointerData {
    output: Option<Output>,
    /// Accumulated across `axis`/`axis_source`/`axis_stop`/`axis_discrete`
    /// requests until this same object's own `frame` request commits it --
    /// A `Mutex`, not a plain `RefCell`, because `wayland-server`'s own
    /// `DataInit::init` requires per-object user data to be `Send + Sync`
    /// - `Dispatch::request` only ever hands out `&self`, not `&mut
    /// self`, for the object the request arrived on, so interior
    /// mutability is unavoidable either way.
    pending_axis: Mutex<Option<AxisFrame>>,
    /// Set by `CompState::set_virtual_pointer_pin` - see this module's
    /// own doc comment for the full Phase 2 design. `Some(id)` routes
    /// every motion/button request on this object straight to that
    /// window's surface instead of the shared seat path.
    pinned_window: Mutex<Option<WindowId>>,
    /// This pinned stream's own local position, physical pixels relative
    /// to the target window's content top-left - entirely separate from
    /// `pointer_pos`. `None` until the first motion after being pinned (or
    /// after the target window changes), at which point it starts at the
    /// window's own center, the same "start somewhere reasonable, not at
    /// a corner" convention a real pointer entering a window has no
    /// equivalent need for (it already has a real position to carry in).
    pinned_pos: Mutex<Option<Point<f64, Logical>>>,
    /// The surface a real `wl_pointer.enter` has actually been sent to for
    /// this pinned stream, if any - so a `leave` reaches the right place
    /// when unpinned, re-pinned elsewhere, or destroyed, matching a real
    /// pointer's own enter/leave discipline instead of leaving a client's
    /// idea of pointer presence stuck forever.
    pinned_entered: Mutex<Option<WlSurface>>,
}

impl GlobalDispatch<ZwlrVirtualPointerManagerV1, ()> for CompState {
    fn bind(_state: &mut Self, _dh: &DisplayHandle, _client: &Client, manager: New<ZwlrVirtualPointerManagerV1>, _data: &(), data_init: &mut DataInit<'_, Self>) {
        data_init.init(manager, ());
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrVirtualPointerManagerV1,
        request: zwlr_virtual_pointer_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_virtual_pointer_manager_v1::Request;
        match request {
            // `seat` is documented as "a suggestion to the compositor" --
            // this compositor has exactly one real `Seat`, so there is
            // nothing to route between and the suggestion is a no-op by
            // construction, not an oversight.
            Request::CreateVirtualPointer { seat: _, id } => {
                let resource = data_init.init(id, VirtualPointerData::default());
                // Registered so `set_virtual_pointer_pin` (Phase 2, this
                // module's own doc comment) can find it later by the
                // owning client's pid - see that doc comment for why pid
                // rather than a per-object id.
                state.virtual_pointers.push(resource);
            }
            Request::CreateVirtualPointerWithOutput { seat: _, output, id } => {
                let output = output.as_ref().and_then(Output::from_resource);
                let resource = data_init.init(id, VirtualPointerData { output, ..Default::default() });
                state.virtual_pointers.push(resource);
            }
            Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrVirtualPointerV1, VirtualPointerData> for CompState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _pointer: &ZwlrVirtualPointerV1,
        request: zwlr_virtual_pointer_v1::Request,
        data: &VirtualPointerData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_virtual_pointer_v1::Request;
        match request {
            Request::Motion { time, dx, dy } => {
                if let Some(window) = *data.pinned_window.lock().unwrap() {
                    let (Some((w, h)), Some(surface)) = (pinned_window_size(state, window), pinned_target_surface(state, window)) else { return };
                    let base = data.pinned_pos.lock().unwrap().unwrap_or_else(|| Point::from((w / 2.0, h / 2.0)));
                    let target = Point::<f64, Logical>::from((
                        (base.x + dx.to_f64()).clamp(0.0, (w - 1.0).max(0.0)),
                        (base.y + dy.to_f64()).clamp(0.0, (h - 1.0).max(0.0)),
                    ));
                    *data.pinned_pos.lock().unwrap() = Some(target);
                    pinned_move_to(state, data, &surface, target, time);
                    return;
                }
                let (min_x, min_y, max_x, max_y) = pointer_bounds(state);
                let pos = last_pointer_pos(state);
                let target = Point::<f64, Logical>::from((
                    (pos.x + dx.to_f64()).clamp(min_x, (max_x - 1.0).max(min_x)),
                    (pos.y + dy.to_f64()).clamp(min_y, (max_y - 1.0).max(min_y)),
                ));
                if let Some(udev) = state.udev.as_mut() {
                    udev.pointer_pos = target;
                }
                handle_pointer_position(state, target, time);
            }
            Request::MotionAbsolute { time, x, y, x_extent, y_extent } => {
                if x_extent == 0 || y_extent == 0 {
                    return;
                }
                if let Some(window) = *data.pinned_window.lock().unwrap() {
                    let (Some((w, h)), Some(surface)) = (pinned_window_size(state, window), pinned_target_surface(state, window)) else { return };
                    let (nx, ny) = (x as f64 / x_extent as f64, y as f64 / y_extent as f64);
                    let target = Point::<f64, Logical>::from(((nx * w).clamp(0.0, (w - 1.0).max(0.0)), (ny * h).clamp(0.0, (h - 1.0).max(0.0))));
                    *data.pinned_pos.lock().unwrap() = Some(target);
                    pinned_move_to(state, data, &surface, target, time);
                    return;
                }
                let (nx, ny) = (x as f64 / x_extent as f64, y as f64 / y_extent as f64);
                // Mapped onto the requested output's own full geometry if
                // `create_virtual_pointer_with_output` named one, otherwise
                // the union of every head - the same "whole addressable
                // span" `PointerMotionAbsolute`'s real-hardware handling
                // already uses (`udev/session.rs`), just picked per-request
                // instead of always being the full union.
                let (min_x, min_y, w, h) = if let Some(output) = &data.output {
                    let name = output.name();
                    match state.wm.borrow().monitors().iter().find(|m| m.name == name) {
                        Some(m) => (m.full_geometry.x as f64, m.full_geometry.y as f64, m.full_geometry.width as f64, m.full_geometry.height as f64),
                        None => {
                            let (min_x, min_y, max_x, max_y) = pointer_bounds(state);
                            (min_x, min_y, max_x - min_x, max_y - min_y)
                        }
                    }
                } else {
                    let (min_x, min_y, max_x, max_y) = pointer_bounds(state);
                    (min_x, min_y, max_x - min_x, max_y - min_y)
                };
                let target = Point::<f64, Logical>::from(((min_x + nx * w).clamp(min_x, min_x + w - 1.0), (min_y + ny * h).clamp(min_y, min_y + h - 1.0)));
                if let Some(udev) = state.udev.as_mut() {
                    udev.pointer_pos = target;
                }
                handle_pointer_position(state, target, time);
            }
            Request::Button { time, button, state: button_state } => {
                let Ok(button_state) = button_state.into_result() else { return };
                let pressed = button_state == wl_pointer::ButtonState::Pressed;
                if let Some(window) = *data.pinned_window.lock().unwrap() {
                    pinned_deliver_button(state, data, window, button, pressed, time);
                    return;
                }
                let pos = last_pointer_pos(state);
                handle_pointer_button(state, pos, button, pressed, time);
            }
            Request::Axis { time, axis, value } => {
                let Ok(axis) = axis.into_result() else { return };
                let axis = wire_axis(axis);
                let mut pending = data.pending_axis.lock().unwrap();
                let frame = pending.take().unwrap_or_else(|| AxisFrame::new(time));
                *pending = Some(frame.value(axis, value.to_f64()));
            }
            Request::AxisSource { axis_source } => {
                let Ok(axis_source) = axis_source.into_result() else { return };
                let Some(source) = wire_axis_source(axis_source) else { return };
                let mut pending = data.pending_axis.lock().unwrap();
                let frame = pending.take().unwrap_or_else(|| AxisFrame::new(0));
                *pending = Some(frame.source(source));
            }
            Request::AxisStop { time, axis } => {
                let Ok(axis) = axis.into_result() else { return };
                let axis = wire_axis(axis);
                let mut pending = data.pending_axis.lock().unwrap();
                let frame = pending.take().unwrap_or_else(|| AxisFrame::new(time));
                *pending = Some(frame.stop(axis));
            }
            Request::AxisDiscrete { time, axis, value, discrete } => {
                let Ok(axis) = axis.into_result() else { return };
                let ax = wire_axis(axis);
                let mut pending = data.pending_axis.lock().unwrap();
                let frame = pending.take().unwrap_or_else(|| AxisFrame::new(time));
                // v120 is the modern wl_pointer convention for "discrete
                // steps" (120 units per notch) - `discrete` here is the
                // older plain step count, so it's scaled the same way
                // smithay's own libinput backend already does for a real
                // wheel (see `input/gestures.rs`).
                *pending = Some(frame.value(ax, value.to_f64()).v120(ax, discrete * 120));
            }
            Request::Frame => {
                let Some(frame) = data.pending_axis.lock().unwrap().take() else { return };
                let Some(pointer) = state.seat.get_pointer() else { return };
                pointer.axis(state, frame);
                pointer.frame(state);
            }
            Request::Destroy => {}
            _ => {}
        }
    }

    /// The client that owns this object disconnected, or the object was
    /// otherwise dropped without an explicit `destroy` request - either
    /// way, if this pinned stream had a real `wl_pointer.enter` on record
    /// somewhere, that target's client is owed a `leave` (it may well be
    /// a completely different, still-alive client - the one being
    /// controlled, not the one that just went away) so it doesn't keep
    /// thinking a pointer is present forever.
    fn destroyed(state: &mut Self, _client: smithay::reexports::wayland_server::backend::ClientId, pointer: &ZwlrVirtualPointerV1, data: &VirtualPointerData) {
        state.virtual_pointers.retain(|p| p.id() != pointer.id());
        pinned_leave_current(state, data);
    }
}

impl CompState {
    /// Pins (`window` is `Some`) or unpins (`None`) every virtual pointer
    /// object owned by the client with process id `pid` - see `virtual_
    /// pointer.rs`'s own module doc comment for the full Phase 2 design.
    /// Queued via the `pin_input`/`unpin_input` IPC dispatch
    /// (`crates/platform/src/ipc.rs`) and drained the same one-poll-tick-
    /// later way `set_output_position` already is (`WindowManager::drain_
    /// pin_input_requests`).
    pub(crate) fn set_virtual_pointer_pin(&mut self, pid: i32, window: Option<WindowId>) {
        // Real, applied state - not just the request that led here - so
        // an IPC caller can read back "is pid X pinned to a window right
        // now" instead of only ever writing blind. See `WindowManager::
        // set_pinned_window`'s own doc comment.
        self.wm.borrow_mut().set_pinned_window(pid, window);
        self.virtual_pointers.retain(|p| p.is_alive());
        let matching: Vec<ZwlrVirtualPointerV1> = self
            .virtual_pointers
            .iter()
            .filter(|p| p.client().and_then(|c| c.get_credentials(&self.dh).ok()).is_some_and(|c| c.pid == pid))
            .cloned()
            .collect();
        for pointer in matching {
            let Some(data) = pointer.data::<VirtualPointerData>() else { continue };
            *data.pinned_window.lock().unwrap() = window;
            *data.pinned_pos.lock().unwrap() = None;
            // Re-pinning to a *different* window is handled lazily, by
            // `pinned_move_to`'s own leave-before-re-enter check on the
            // next motion - but unpinning outright has no next motion to
            // do that on, so the leave has to happen right here instead,
            // immediately, rather than leaving the old target thinking a
            // pointer is still present until whenever (if ever) this same
            // pid is pinned somewhere else again.
            if window.is_none() {
                pinned_leave_current(self, data);
            }
        }
    }
}

/// The whole addressable pointer span, in logical coordinates - the union
/// of every head on the DRM backend, and the union of every
/// `WindowManager` monitor otherwise.
///
/// Every `Motion`/`MotionAbsolute` bounds lookup here used to read
/// `UdevState::bounds()` directly, behind an early `return` when
/// `state.udev` was `None`. That field is `Some` only for the DRM backend
/// (see `state/mod.rs`), so on the nested winit backend this protocol
/// advertised its global, accepted `create_virtual_pointer`, accepted
/// every request, and then silently discarded all motion: no error, no
/// log, nothing on screen. That is the exact backend a nested test
/// instance runs on, so the one safe way to drive synthetic input at a
/// throwaway compositor - a Wayland client of that compositor, which
/// cannot reach any other session by construction, unlike a uinput-level
/// tool such as `ydotool` - did not work at all. Found while trying to
/// verify Nemo's right-click popup without clicking blind at the user's
/// real desktop.
///
/// `WindowManager::monitors()` is filled from `Platform::monitors()` at
/// startup and on every hotplug poll (`crates/srdwm/src/main.rs`), by both
/// backends, so it is the backend-agnostic source. The DRM branch stays
/// first and unchanged: `heads` is what that backend actually clamps its
/// own `pointer_pos` against, and the two lists can legitimately disagree
/// mid-hotplug.
fn pointer_bounds(state: &CompState) -> (f64, f64, f64, f64) {
    if let Some(udev) = state.udev.as_ref() {
        return udev.bounds();
    }
    let wm = state.wm.borrow();
    crate::udev::bounds_of(wm.monitors().iter().map(|m| (m.full_geometry.x, m.full_geometry.y, m.full_geometry.width as i32, m.full_geometry.height as i32)))
}

/// This pinned stream's target window's own current content size,
/// physical pixels - `core::Window::geometry` is already physical, the
/// same convention `MotionEvent.location` and everything else in this
/// pointer pipeline uses (see this module's own doc comment on the
/// separate, already-documented `wl_pointer` client-scale gap this shares
/// rather than compounds). `None` once the window no longer exists.
fn pinned_window_size(state: &CompState, window: WindowId) -> Option<(f64, f64)> {
    state.wm.borrow().windows().find(|w| w.id == window).map(|w| (w.geometry.width as f64, w.geometry.height as f64))
}

/// This pinned stream's target window's own main surface, if it still
/// exists - shared with `raise_pinned`'s own `id_to_window` lookup
/// (`state/geometry.rs`), reusing `elements::window_wl_surface` for the
/// Wayland/X11-both-kinds resolution every other caller of it already
/// needs.
fn pinned_target_surface(state: &CompState, window: WindowId) -> Option<WlSurface> {
    state.id_to_window.get(&window).and_then(window_wl_surface)
}

/// Every real `WlPointer` resource the client owning `surface` has bound
/// on the one real seat - `PointerHandle::client_pointers`, a genuine
/// smithay-public API for exactly this (not something hand-rolled around
/// its back). Empty if the surface has no client (already destroyed) or
/// that client never bound a pointer on this seat at all.
fn client_pointers_for(state: &CompState, surface: &WlSurface) -> Vec<wl_pointer::WlPointer> {
    let Some(client) = surface.client() else { return Vec::new() };
    let Some(pointer) = state.seat.get_pointer() else { return Vec::new() };
    pointer.client_pointers(&client).collect()
}

/// Sends `leave` (plus `frame`) to whatever surface this pinned stream
/// last actually entered, if any, and clears that record - called before
/// re-entering a *different* surface, on an explicit unpin, and on
/// destroy. A real pointer's own enter/leave discipline, applied to a
/// synthetic one: a client that never gets a matching `leave` has no
/// reason to believe the pointer it saw `enter` ever went away.
fn pinned_leave_current(state: &CompState, data: &VirtualPointerData) {
    let Some(prev) = data.pinned_entered.lock().unwrap().take() else { return };
    let serial = u32::from(SERIAL_COUNTER.next_serial());
    for p in client_pointers_for(state, &prev) {
        p.leave(serial, &prev);
        p.frame();
    }
}

/// Ensures `surface` has a real `wl_pointer.enter` on record for this
/// pinned stream (sending `leave` first to whatever it was previously
/// entered into, if that was a *different* surface - re-pinned to
/// another window with no intervening unpin), then sends `motion` and
/// `frame` to every bound pointer resource. `local` is content-relative,
/// physical pixels, already clamped to the target window's own bounds by
/// every caller.
fn pinned_move_to(state: &mut CompState, data: &VirtualPointerData, surface: &WlSurface, local: Point<f64, Logical>, time: u32) {
    let pointers = client_pointers_for(state, surface);
    if pointers.is_empty() {
        return;
    }
    let needs_enter = data.pinned_entered.lock().unwrap().as_ref() != Some(surface);
    if needs_enter {
        pinned_leave_current(state, data);
        let serial = u32::from(SERIAL_COUNTER.next_serial());
        for p in &pointers {
            p.enter(serial, surface, local.x, local.y);
        }
        *data.pinned_entered.lock().unwrap() = Some(surface.clone());
    }
    for p in &pointers {
        p.motion(time, local.x, local.y);
        p.frame();
    }
}

/// Delivers a pinned `button` request: makes sure the target window has
/// actually been entered at *some* known position first (a button press
/// with no prior motion on this pinned stream still needs a real
/// enter/motion pair before a button event makes sense to a client, same
/// as a real pointer that had just appeared over a window), defaulting to
/// its content center the same way a fresh pin with no motion yet does,
/// then sends the real `button`/`frame` wire events.
fn pinned_deliver_button(state: &mut CompState, data: &VirtualPointerData, window: WindowId, button: u32, pressed: bool, time: u32) {
    let Some(surface) = pinned_target_surface(state, window) else { return };
    let local = data.pinned_pos.lock().unwrap().unwrap_or_else(|| {
        let (w, h) = pinned_window_size(state, window).unwrap_or((0.0, 0.0));
        Point::from((w / 2.0, h / 2.0))
    });
    *data.pinned_pos.lock().unwrap() = Some(local);
    pinned_move_to(state, data, &surface, local, time);
    let serial = u32::from(SERIAL_COUNTER.next_serial());
    let button_state = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
    for p in client_pointers_for(state, &surface) {
        p.button(serial, time, button, button_state);
        p.frame();
    }
}

fn wire_axis(axis: wl_pointer::Axis) -> Axis {
    match axis {
        wl_pointer::Axis::HorizontalScroll => Axis::Horizontal,
        _ => Axis::Vertical,
    }
}

fn wire_axis_source(source: wl_pointer::AxisSource) -> Option<AxisSource> {
    Some(match source {
        wl_pointer::AxisSource::Wheel => AxisSource::Wheel,
        wl_pointer::AxisSource::Finger => AxisSource::Finger,
        wl_pointer::AxisSource::Continuous => AxisSource::Continuous,
        wl_pointer::AxisSource::WheelTilt => AxisSource::WheelTilt,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_axis_maps_horizontal_and_vertical_correctly() {
        assert_eq!(wire_axis(wl_pointer::Axis::HorizontalScroll), Axis::Horizontal);
        assert_eq!(wire_axis(wl_pointer::Axis::VerticalScroll), Axis::Vertical);
    }

    #[test]
    fn wire_axis_source_maps_every_known_source() {
        assert!(wire_axis_source(wl_pointer::AxisSource::Wheel).is_some());
        assert!(wire_axis_source(wl_pointer::AxisSource::Finger).is_some());
        assert!(wire_axis_source(wl_pointer::AxisSource::Continuous).is_some());
        assert!(wire_axis_source(wl_pointer::AxisSource::WheelTilt).is_some());
    }
}
