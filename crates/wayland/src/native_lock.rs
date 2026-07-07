//! srdwm's own session-lock UI: no external locker client needed.
//!
//! Triggered by `srd dispatch lock` (`crates/platform/src/ipc.rs`'s
//! `"lock"` command queues the intent on `WindowManager`; each backend
//! drains it on its own next poll, same cross-boundary pattern
//! `output_position_requests` already uses). Once triggered, srdwm
//! captures each output's currently-displayed content, blurs it
//! (`crate::blur`), and draws its own centered password-entry box over
//! that blurred background - a live-blurred lock screen without relying
//! on an external locker binary at all.
//!
//! Authentication is real PAM (`srdwm_platform::authenticate`), run on a
//! background thread (`std::thread::spawn`, polled via a channel) rather
//! than blocking the compositor's single-threaded event loop - PAM can
//! deliberately introduce a multi-second delay on a wrong password
//! (`pam_fail_delay`), and blocking the whole compositor for that long
//! would freeze rendering and input for everything, not just the lock UI.
//!
//! Security posture, stated plainly because this is the one place in the
//! compositor where getting it wrong has real consequences: `state.lock.
//! locked` (checked by `crate::input`'s locked-session branches and
//! `CompState::set_keyboard_focus`'s guard) is the single source of truth
//! that gates both input and rendering, identical to the external-locker
//! path this reuses rather than duplicates. This module only ever flips
//! it to `true` after every output has a captured, blurred background
//! ready (see `finalize_if_ready`) - never speculatively - and only
//! ever flips it back to `false` after a genuine PAM `authenticate` *and*
//! `acct_mgmt` success (see `poll_auth`). There is no code path that
//! unlocks on a timeout, a malformed keystroke, or any error condition;
//! every failure mode here resolves to "stay locked".

use crate::state::CompState;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::utils::Transform;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Vertical gap between the header (clock/date/avatar), the password box,
/// and the on-screen keyboard - one shared constant so `lock_stack_
/// layout` (used at both render and click-hit-test time) and the eye
/// can't disagree with each other.
const SECTION_GAP: i32 = 20;

/// How long a wrong-password shake plays for, and how far it moves the
/// box/shadow at its peak - see `shake_offset`'s own doc comment for the
/// motion itself. Short and small: this is feedback, not an animation to
/// watch, matching how briefly a real macOS/GNOME lock screen shakes its
/// own password field.
const SHAKE_DURATION: Duration = Duration::from_millis(400);
const SHAKE_AMPLITUDE: f32 = 10.0;

/// One key on the on-screen keyboard - see `render_keyboard`'s own doc
/// comment for the layout this is built from.
pub(crate) struct VirtualKey {
    /// Position and size within the keyboard's own buffer, logical
    /// pixels - what `keyboard_hit_test` compares a click against once
    /// it's translated the click into this same local space.
    rect: (i32, i32, i32, i32),
    /// Fed straight to `native_lock_key` as its own `name` parameter --
    /// `"BackSpace"`/`"Return"`/`"Shift"` for the three keys that aren't
    /// plain character entry, empty for every other key (which instead
    /// carries its character in `utf8_lower`/`utf8_upper`, exactly the
    /// way a real keysym's resolved UTF-8 already does).
    name: &'static str,
    utf8_lower: &'static str,
    utf8_upper: &'static str,
}

/// `srd.lock`'s live state - see this module's own doc comment for the
/// lifecycle. Constructed by `begin`, lives on `SessionLock::native` for
/// as long as `state.lock.locked` is `true` via this path.
pub(crate) struct NativeLock {
    /// Output names still awaiting a captured background before the lock
    /// actually takes effect - see `begin`'s doc comment for why capture
    /// has to finish *before* `state.lock.locked` flips, not after.
    pending_capture: std::collections::HashSet<String>,
    /// Blurred captured background per output, keyed by `Output::name()`.
    backgrounds: HashMap<String, MemoryRenderBuffer>,
    /// The password-entry box, identical on every output (each output
    /// positions it centered at render time) - rebuilt only when the
    /// visible state actually changes (a keystroke, a failed attempt),
    /// the same "cache until dirty" pattern `context_menu`/`snap_flyout`
    /// already use, not rebuilt every frame.
    ui_buffer: Option<MemoryRenderBuffer>,
    ui_size: (i32, i32),
    /// The box's drop shadow - built and cached alongside `ui_buffer`
    /// (same invalidation: both go stale together, since the shadow's
    /// size is derived from the box's own), positioned `SHADOW_SIZE`
    /// pixels up/left of wherever the box itself renders.
    shadow_buffer: Option<MemoryRenderBuffer>,
    /// The clock/date/avatar/username header shown above the password
    /// box - see `render_header_box`'s own doc comment. Cached
    /// separately from `ui_buffer`/rebuilt every render call rather than
    /// only on a state change, since the clock's own text changes once a
    /// minute with nothing else here to signal that - cheap enough
    /// (a handful of glyphs, once per output per frame) that a real
    /// dirty-check isn't worth the bookkeeping.
    header_buffer: Option<MemoryRenderBuffer>,
    header_size: (i32, i32),
    /// The on-screen keyboard, if `LockConfig::show_keyboard` is on --
    /// `None` when the feature is off, same as every other optional
    /// section here. Rebuilt when `shift` toggles (the labels' case
    /// changes) or the theme changes, same invalidation trigger as
    /// `ui_buffer`.
    keyboard_buffer: Option<MemoryRenderBuffer>,
    keyboard_size: (i32, i32),
    /// Each key's own clickable rect (in the keyboard buffer's local
    /// space) and what it types - `keyboard_hit_test`'s own lookup
    /// table, rebuilt alongside `keyboard_buffer` (the two must always
    /// agree on where each key actually is).
    keyboard_keys: Vec<VirtualKey>,
    username: String,
    password: String,
    failed_attempts: u32,
    show_error: bool,
    checking: bool,
    /// The keyboard's caps-lock state as of the last keypress - not
    /// polled independently, since every real keystroke already carries
    /// it (`ModifiersState::caps_lock`, read in `crate::input`'s locked-
    /// keyboard branch), so there is no separate query needed.
    caps_lock: bool,
    /// The on-screen keyboard's own shift state - independent of
    /// `caps_lock` above (a real keyboard's own hardware state) since a
    /// touchscreen session with no physical keyboard at all still needs
    /// a way to type an uppercase letter. Toggled by clicking the
    /// keyboard's own Shift key (`native_lock_key`'s `"Shift"` arm).
    shift: bool,
    /// When the most recent wrong-password shake started, if one is
    /// still playing - see `shake_offset`'s own doc comment. `None`
    /// once `SHAKE_DURATION` has elapsed, so steady-state rendering
    /// doesn't keep computing an animation that's already finished.
    shake_start: Option<Instant>,
    auth_rx: Option<Receiver<bool>>,
}

/// `$USER`, resolved once when the lock begins - same resolution every
/// other real screen locker on this kind of single-user session relies
/// on. If it's ever unset (a broken environment, not a real login
/// session), authentication simply cannot succeed for anyone - fails
/// secure by construction, not a special case to handle.
fn current_username() -> String {
    std::env::var("USER").unwrap_or_default()
}

impl NativeLock {
    /// Builds a fresh, empty lock state - pulled out to one place now
    /// that the struct has grown past the two or three fields it started
    /// with, so `begin_native_lock`'s two call sites (headless vs. the
    /// normal case) can't drift out of agreement on what "fresh" means.
    fn new(pending_capture: std::collections::HashSet<String>) -> Self {
        Self {
            pending_capture,
            backgrounds: HashMap::new(),
            ui_buffer: None,
            ui_size: (0, 0),
            shadow_buffer: None,
            header_buffer: None,
            header_size: (0, 0),
            keyboard_buffer: None,
            keyboard_size: (0, 0),
            keyboard_keys: Vec::new(),
            username: current_username(),
            password: String::new(),
            failed_attempts: 0,
            show_error: false,
            checking: false,
            caps_lock: false,
            shift: false,
            shake_start: None,
            auth_rx: None,
        }
    }
}

impl CompState {
    /// Starts srdwm's own lock - called once, when `WindowManager::
    /// drain_lock_request` reports a pending `srd dispatch lock`. Does
    /// *not* set `state.lock.locked` yet: every output needs a captured
    /// background first (`capture_output` below finishes the job once
    /// they're all in), the same reasoning `SessionLockHandler::lock`'s
    /// own doc comment gives for an external locker waiting on
    /// `pending_confirm` - flipping `locked` before the screen is
    /// actually ready to show something other than the live desktop would
    /// be exactly backwards for a lock screen. A no-op if a lock (native
    /// or external) is already in progress, so a duplicate `srd dispatch
    /// lock` (or one arriving while capture is still pending) can't
    /// restart the capture set and leak the in-progress password buffer.
    pub(crate) fn begin_native_lock(&mut self) {
        if self.lock.locked || self.lock.native.is_some() {
            return;
        }
        let pending_capture: std::collections::HashSet<String> = self.outputs().map(|o| o.name()).collect();
        if pending_capture.is_empty() {
            // No real output to capture from (headless/test invocation) --
            // lock immediately with an empty backdrop rather than waiting
            // forever for a capture that can never arrive.
            self.lock.native = Some(NativeLock::new(pending_capture));
            self.lock.locked = true;
            self.set_keyboard_focus(None);
            return;
        }
        self.lock.native = Some(NativeLock::new(pending_capture));
    }

    /// A backend just captured and blurred `output_name`'s current
    /// content - stores it and, once every output has one, actually
    /// locks. No-op if a native lock isn't in progress (the request was
    /// already satisfied, or never happened) or this output already has a
    /// background (a backend calling this twice for the same output on
    /// consecutive ticks, e.g. after a skipped/flip-pending head becomes
    /// ready, must not restart or double-count anything).
    pub(crate) fn capture_output(&mut self, output_name: &str, blurred: MemoryRenderBuffer) {
        let Some(native) = self.lock.native.as_mut() else { return };
        if self.lock.locked || !native.pending_capture.remove(output_name) {
            return;
        }
        native.backgrounds.insert(output_name.to_string(), blurred);
        if native.pending_capture.is_empty() {
            self.lock.locked = true;
            self.set_keyboard_focus(None);
            log::info!("session lock: native lock engaged, {} output(s) captured", self.lock.native.as_ref().map(|n| n.backgrounds.len()).unwrap_or(0));
        }
    }

    /// The blurred background for `output_name`, if a native lock has one
    /// ready for it yet (it might not, mid-capture on a multi-output
    /// setup where one head is still flip-pending).
    pub(crate) fn native_lock_background(&self, output_name: &str) -> Option<&MemoryRenderBuffer> {
        self.lock.native.as_ref().and_then(|n| n.backgrounds.get(output_name))
    }

    /// Whether a native lock is in progress and still waiting on
    /// `output_name`'s background specifically - what a backend's render
    /// loop checks, once per output per tick, to know whether to run
    /// `native_lock::capture_and_blur` against this pass's freshly
    /// rendered framebuffer.
    pub(crate) fn native_lock_needs_capture(&self, output_name: &str) -> bool {
        self.lock.native.as_ref().is_some_and(|n| n.pending_capture.contains(output_name))
    }

    /// The password-entry box, rebuilding it first (along with its drop
    /// shadow - see `shadow_buffer`'s own doc comment) if the visible
    /// state changed since the last render. `None` while a native lock
    /// isn't actually engaged yet (still waiting on captures) - nothing
    /// should render the UI box before `state.lock.locked` is true
    /// regardless.
    pub(crate) fn native_lock_ui(&mut self) -> Option<(&MemoryRenderBuffer, (i32, i32))> {
        if !self.lock.locked {
            return None;
        }
        let theme = self.wm.borrow().lock.clone();
        let native = self.lock.native.as_mut()?;
        if native.ui_buffer.is_none() {
            let (data, size) = render_ui_box(native, &theme);
            native.ui_buffer = Some(MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, size, 1, Transform::Normal, None));
            native.ui_size = size;
            let shadow_data = crate::decoration::shadow_bitmap(size.0.max(0) as u32, size.1.max(0) as u32, theme.corner_radius, crate::decoration::SHADOW_MAX_ALPHA);
            let shadow_size = crate::decoration::shadow_rect(srdwm_core::Rect::new(0, 0, size.0.max(0) as u32, size.1.max(0) as u32));
            native.shadow_buffer = Some(MemoryRenderBuffer::from_slice(&shadow_data, Fourcc::Argb8888, (shadow_size.width as i32, shadow_size.height as i32), 1, Transform::Normal, None));
        }
        native.ui_buffer.as_ref().map(|b| (b, native.ui_size))
    }

    /// The box's own drop shadow, sized `SHADOW_SIZE` larger than
    /// `native_lock_ui`'s own buffer on every side - always built
    /// alongside it (see that method's own body), so this is `Some` iff
    /// the UI box itself is.
    pub(crate) fn native_lock_shadow(&self) -> Option<&MemoryRenderBuffer> {
        self.lock.native.as_ref()?.shadow_buffer.as_ref()
    }

    /// The clock/date/avatar/username header shown above the password
    /// box - `None` when `LockConfig::show_clock` is off, same "cached,
    /// rebuild on demand" shape as `native_lock_ui`. Unlike that method,
    /// this rebuilds unconditionally rather than only on a state change:
    /// see `header_buffer`'s own doc comment for why (the clock's own
    /// text is real wall-clock time, which changes with nothing else here
    /// to signal it).
    pub(crate) fn native_lock_header(&mut self) -> Option<(&MemoryRenderBuffer, (i32, i32))> {
        if !self.lock.locked {
            return None;
        }
        let theme = self.wm.borrow().lock.clone();
        if !theme.show_clock {
            return None;
        }
        let native = self.lock.native.as_mut()?;
        let (data, size) = render_header_box(native, &theme);
        native.header_buffer = Some(MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, size, 1, Transform::Normal, None));
        native.header_size = size;
        native.header_buffer.as_ref().map(|b| (b, native.header_size))
    }

    /// The on-screen keyboard, if `LockConfig::show_keyboard` is on --
    /// same "cached, rebuild when the visible state changes" shape as
    /// `native_lock_ui` (the labels' case changes when `shift` toggles).
    pub(crate) fn native_lock_keyboard(&mut self) -> Option<(&MemoryRenderBuffer, (i32, i32))> {
        if !self.lock.locked {
            return None;
        }
        let theme = self.wm.borrow().lock.clone();
        if !theme.show_keyboard {
            return None;
        }
        let native = self.lock.native.as_mut()?;
        if native.keyboard_buffer.is_none() {
            let (data, size, keys) = render_keyboard(native, &theme);
            native.keyboard_buffer = Some(MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, size, 1, Transform::Normal, None));
            native.keyboard_size = size;
            native.keyboard_keys = keys;
        }
        native.keyboard_buffer.as_ref().map(|b| (b, native.keyboard_size))
    }

    /// How far, right now, the password box (and its shadow) should be
    /// shifted horizontally for a wrong-password shake - see
    /// `shake_offset`'s own doc comment for the actual motion. `0.0` when
    /// no shake is playing, including the common case (nothing has ever
    /// failed this lock session) and once `SHAKE_DURATION` has elapsed.
    pub(crate) fn native_lock_shake_offset(&mut self) -> f32 {
        let Some(native) = self.lock.native.as_mut() else { return 0.0 };
        let Some(start) = native.shake_start else { return 0.0 };
        let elapsed = start.elapsed();
        if elapsed >= SHAKE_DURATION {
            native.shake_start = None;
            return 0.0;
        }
        shake_offset(elapsed)
    }

    /// Routes one key press to the native lock's own input handling --
    /// called from `crate::input::handle_keyboard_key_event` instead of
    /// forwarding to a client, whenever `state.lock.native.is_some()`.
    /// `utf8` is whatever `xkbcommon::xkb::keysym_to_utf8` produced for
    /// this keysym - empty for anything non-printable (arrows, function
    /// keys, modifiers on their own).
    pub(crate) fn native_lock_key(&mut self, name: &str, utf8: &str, caps_lock: bool) {
        let Some(native) = self.lock.native.as_mut() else { return };
        if native.checking {
            // An auth attempt is already in flight - ignore further
            // input rather than queuing a second overlapping PAM call.
            return;
        }
        // The on-screen keyboard's own Shift key - a sentinel `name`
        // `keyboard_hit_test` sends, never a value a real keysym resolves
        // to (`"Shift_L"`/`"Shift_R"` are the real xkb names for the
        // physical key, a different string). Toggles the keyboard's own
        // case for the *next* letter typed through it; doesn't touch the
        // password itself, so it invalidates `keyboard_buffer` (the
        // labels' case changes) rather than `ui_buffer`.
        if name == "Shift" {
            native.shift = !native.shift;
            native.keyboard_buffer = None;
            return;
        }
        native.show_error = false;
        native.caps_lock = caps_lock;
        match name {
            "BackSpace" => {
                native.password.pop();
            }
            "Return" | "KP_Enter" => {
                if native.password.is_empty() {
                    return;
                }
                let (username, password) = (native.username.clone(), std::mem::take(&mut native.password));
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let ok = srdwm_platform::authenticate(&username, &password);
                    let _ = tx.send(ok);
                });
                native.auth_rx = Some(rx);
                native.checking = true;
            }
            "Escape" => {
                native.password.clear();
            }
            _ => {
                // Any other non-empty UTF-8 is a printable character to
                // append - covers letters, digits, symbols, and anything
                // a layout's own dead-key/compose sequence resolved to,
                // without hand-maintaining a list of "printable" keysym
                // names the way titlebar/menu code never has to.
                if !utf8.is_empty() {
                    native.password.push_str(utf8);
                }
            }
        }
        native.ui_buffer = None;
    }

    /// Routes one pointer click to the on-screen keyboard, if the native
    /// lock has one and the click landed on one of its keys - called
    /// from `crate::input::handle_pointer_button`'s own locked branch
    /// whenever `state.lock.native.is_some()`. `pos` is the same global-
    /// space point every other pointer handler already works in;
    /// `crate::state::CompState::output_at` resolves which output (and so
    /// which origin to subtract) the click actually landed on, the same
    /// way hit-testing anywhere else in this compositor already does.
    /// Returns whether a key was actually hit, purely so the caller can
    /// decide whether to also forward the click to a lock surface (it
    /// never should here, but keeps the two call sites symmetric).
    pub(crate) fn native_lock_click(&mut self, pos: smithay::utils::Point<f64, smithay::utils::Logical>) -> bool {
        let Some(entry) = self.output_at(pos) else { return false };
        let (origin, output_size) = (entry.location, entry.size());
        let output_size = (output_size.w, output_size.h);
        let theme = self.wm.borrow().lock.clone();
        let Some(native) = self.lock.native.as_ref() else { return false };
        if !theme.show_keyboard || native.keyboard_keys.is_empty() {
            return false;
        }
        let (_, _, keyboard_pos) = lock_stack_layout(output_size, theme.show_clock.then_some(native.header_size), native.ui_size, Some(native.keyboard_size));
        let Some(keyboard_pos) = keyboard_pos else { return false };
        let local_x = (pos.x - origin.x as f64 - keyboard_pos.0 as f64).round() as i32;
        let local_y = (pos.y - origin.y as f64 - keyboard_pos.1 as f64).round() as i32;
        let Some(key) = native.keyboard_keys.iter().find(|k| {
            let (kx, ky, kw, kh) = k.rect;
            local_x >= kx && local_x < kx + kw && local_y >= ky && local_y < ky + kh
        }) else {
            return false;
        };
        let shift_was_on = native.shift;
        let (name, utf8) = (key.name, if shift_was_on { key.utf8_upper } else { key.utf8_lower });
        let caps_lock = self.lock.native.as_ref().map(|n| n.caps_lock).unwrap_or(false);
        self.native_lock_key(name, utf8, caps_lock);
        // One-shot shift, like a real mobile on-screen keyboard: typing an
        // actual character while shift was on releases it again, so the
        // *next* letter isn't uppercase too unless clicked again. Only for
        // a genuine character key - Shift/BackSpace/Return themselves
        // (and Shift toggling itself, handled entirely inside `native_
        // lock_key` already) must not also trigger this.
        if shift_was_on && name.is_empty() && !utf8.is_empty() {
            if let Some(native) = self.lock.native.as_mut() {
                native.shift = false;
                native.keyboard_buffer = None;
            }
        }
        true
    }

    /// Checks whether a PAM authentication spawned by `native_lock_key`
    /// finished - called once per poll from both backends, same cadence
    /// `drain_lock_request` is drained at. On success, unlocks through
    /// the exact same `SessionLockHandler::unlock` path the external-
    /// locker protocol uses, so both routes leave `CompState` in one
    /// consistent post-unlock state (surfaces cleared, damage-tracker
    /// ages reset, focus handed back). On failure, clears the password
    /// and shows the configured failure message - never anything that
    /// distinguishes *why* it failed beyond the log line `srdwm_platform::
    /// authenticate` already wrote, so a locked-out account and a typo
    /// look identical from the lock screen itself.
    pub(crate) fn poll_native_lock_auth(&mut self) {
        let Some(native) = self.lock.native.as_mut() else { return };
        let Some(rx) = native.auth_rx.as_ref() else { return };
        match rx.try_recv() {
            Ok(true) => {
                use smithay::wayland::session_lock::SessionLockHandler;
                SessionLockHandler::unlock(self);
            }
            Ok(false) => {
                let native = self.lock.native.as_mut().expect("checked Some above");
                native.auth_rx = None;
                native.checking = false;
                native.failed_attempts += 1;
                native.show_error = true;
                native.ui_buffer = None;
                native.shake_start = Some(Instant::now());
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                // The auth thread panicked or was dropped without sending
                // - treat exactly like a failed attempt, never a silent
                // unlock. `catch_unwind` in the thread closure would be
                // stronger, but a disconnected channel already can't
                // reach the success arm above no matter what, so this is
                // fail-secure either way.
                let native = self.lock.native.as_mut().expect("checked Some above");
                native.auth_rx = None;
                native.checking = false;
                native.failed_attempts += 1;
                native.show_error = true;
                native.ui_buffer = None;
                native.shake_start = Some(Instant::now());
            }
        }
    }
}

/// Everything `native_lock_render_elements` needs, extracted from
/// `CompState` before the caller's own renderer-holding borrow starts --
/// see that function's own doc comment for why this can't just take
/// `&mut CompState` directly. One struct instead of five parameters
/// purely for readability at the (two) call sites; nothing here does any
/// work of its own.
pub(crate) struct NativeLockFrame<'a> {
    pub(crate) background: Option<&'a MemoryRenderBuffer>,
    pub(crate) header: Option<(&'a MemoryRenderBuffer, (i32, i32))>,
    pub(crate) shadow: Option<&'a MemoryRenderBuffer>,
    pub(crate) ui: Option<(&'a MemoryRenderBuffer, (i32, i32))>,
    pub(crate) keyboard: Option<(&'a MemoryRenderBuffer, (i32, i32))>,
    /// `CompState::native_lock_shake_offset`'s own doc comment - applied
    /// to the box and its shadow only, never the header or keyboard.
    pub(crate) shake_offset: f32,
}

/// Render elements for a native-locked output: the blurred background (if
/// this output's capture is ready), the header (clock/date/avatar), the
/// password box's drop shadow, the box itself, and the on-screen keyboard
/// - header/box/keyboard stacked and centered together via `lock_stack_
/// layout`, the same layout `CompState::native_lock_click` hit-tests
/// against. Mirrors `lock::lock_render_elements`'s shape/signature so both
/// backends can call whichever mode applies with the same pattern.
/// Takes every buffer by reference rather than `&mut CompState`, same
/// reasoning `lock::lock_render_elements`'s own doc comment gives for
/// taking a bare surface instead: both backends' render loops call this
/// while already holding a field-specific `&mut` borrow (`self.udev`/the
/// winit backend's own renderer), not a whole-`self` one, so a caller has
/// to extract every field of `NativeLockFrame` *before* that borrow
/// starts (cheap `MemoryRenderBuffer` clones, not a deep pixel copy) and
/// pass the clones in.
pub(crate) fn native_lock_render_elements<R>(frame: NativeLockFrame<'_>, output_size: (i32, i32), renderer: &mut R) -> Vec<MemoryRenderBufferRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let mut elements = Vec::new();
    let ui_size = frame.ui.map(|(_, s)| s).unwrap_or((0, 0));
    let (header_pos, ui_pos, keyboard_pos) = lock_stack_layout(output_size, frame.header.map(|(_, s)| s), ui_size, frame.keyboard.map(|(_, s)| s));
    // Header first (topmost) - purely decorative, so draw order against
    // the box/keyboard below it doesn't matter for correctness, only for
    // matching every other element list's own "first pushed, first
    // drawn" convention in this codebase.
    if let (Some((header, _)), Some(pos)) = (frame.header, header_pos) {
        match MemoryRenderBufferRenderElement::from_buffer(renderer, (pos.0 as f64, pos.1 as f64), header, None, None, None, Kind::Unspecified) {
            Ok(elem) => elements.push(elem),
            Err(e) => log::warn!("native lock: failed to import header buffer: {e}"),
        }
    }
    if let Some((keyboard, _)) = frame.keyboard {
        if let Some(pos) = keyboard_pos {
            match MemoryRenderBufferRenderElement::from_buffer(renderer, (pos.0 as f64, pos.1 as f64), keyboard, None, None, None, Kind::Unspecified) {
                Ok(elem) => elements.push(elem),
                Err(e) => log::warn!("native lock: failed to import keyboard buffer: {e}"),
            }
        }
    }
    // The box itself, then its shadow right behind it - both shifted
    // horizontally by the same wrong-password shake offset, so the
    // shadow reads as genuinely cast by the box moving rather than a
    // separate, independently-drifting element.
    if let Some((ui, _)) = frame.ui {
        let pos = (ui_pos.0 as f64 + frame.shake_offset as f64, ui_pos.1 as f64);
        match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, ui, None, None, None, Kind::Unspecified) {
            Ok(elem) => elements.push(elem),
            Err(e) => log::warn!("native lock: failed to import UI buffer: {e}"),
        }
    }
    if let Some(shadow) = frame.shadow {
        let shadow_size = crate::decoration::SHADOW_SIZE as f64;
        let pos = (ui_pos.0 as f64 - shadow_size + frame.shake_offset as f64, ui_pos.1 as f64 - shadow_size);
        match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, shadow, None, None, None, Kind::Unspecified) {
            Ok(elem) => elements.push(elem),
            Err(e) => log::warn!("native lock: failed to import shadow buffer: {e}"),
        }
    }
    if let Some(bg) = frame.background {
        match MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), bg, None, None, None, Kind::Unspecified) {
            Ok(elem) => elements.push(elem),
            Err(e) => log::warn!("native lock: failed to import background buffer: {e}"),
        }
    }
    elements
}

/// Captures `size` (physical, buffer-coordinate) pixels from `framebuffer`
/// as owned bytes, blurs them, and wraps the result as a
/// `MemoryRenderBuffer` ready to hand to `CompState::capture_output`.
/// Shared by both backends' capture hooks (see `udev/render.rs`/`winit/
/// render.rs` for where `framebuffer` itself actually comes from --
/// backend-specific: DRM/GBM vs the nested Wayland connection - which is
/// the only part that couldn't live here too).
///
/// `Xrgb8888`, not `Argb8888`, deliberately: the captured desktop content
/// is always fully opaque, but the alpha byte `copy_framebuffer`/
/// `map_texture` hands back for an opaque render is not guaranteed to
/// actually *be* `255` (nothing upstream promises that for a format
/// that's never supposed to need it) - reinterpreting it as `Argb8888`
/// would trust that undefined byte as real alpha. `Xrgb8888` tells
/// smithay the byte is meaningless and to treat the whole buffer as
/// opaque regardless of its value, the same technique `screencopy.rs`'s
/// own capture path already uses (`CAPTURE_FOURCC = Fourcc::Xrgb8888`)
/// for exactly this reason.
pub(crate) fn capture_and_blur<R>(renderer: &mut R, framebuffer: &R::Framebuffer<'_>, size: (i32, i32), radius: u32) -> Result<MemoryRenderBuffer, String>
where
    R: Renderer + smithay::backend::renderer::ExportMem,
{
    let src = smithay::utils::Rectangle::<i32, smithay::utils::Buffer>::from_size(size.into());
    let mapping = renderer.copy_framebuffer(framebuffer, src, Fourcc::Xrgb8888).map_err(|e| format!("copy_framebuffer: {e}"))?;
    let mut pixels = renderer.map_texture(&mapping).map_err(|e| format!("map_texture: {e}"))?.to_vec();
    crate::blur::box_blur(&mut pixels, size.0.max(0) as usize, size.1.max(0) as usize, radius);
    Ok(MemoryRenderBuffer::from_slice(&pixels, Fourcc::Xrgb8888, size, 1, Transform::Normal, None))
}

/// Where the header, the password box, and the on-screen keyboard each
/// land: stacked top-to-bottom with `SECTION_GAP` between whichever
/// sections are actually present, each individually centered on `output_
/// size.0`. Shared, byte-for-byte, by the render path (`native_lock_
/// render_elements`) and the click-hit-test path (`CompState::native_
/// lock_click`) so a key's on-screen position and its own clickable rect
/// can never silently disagree - the same "one function, two callers"
/// shape `udev/outputs.rs::next_logical_x` already established for
/// exactly this kind of layout-math-shared-with-a-consumer problem.
///
/// `header_size`/`keyboard_size` are `None` when that section is turned
/// off (`LockConfig::show_clock`/`show_keyboard`) or (for the keyboard
/// specifically, from the click path) simply hasn't rendered yet this
/// lock session - `ui_size` alone is never optional, since the password
/// box is the one section that always exists.
///
/// Returns each section's own top-left corner in `output_size`'s own
/// coordinate space - `None` for a section that was passed in as `None`.
#[allow(clippy::type_complexity)]
fn lock_stack_layout(output_size: (i32, i32), header_size: Option<(i32, i32)>, ui_size: (i32, i32), keyboard_size: Option<(i32, i32)>) -> (Option<(i32, i32)>, (i32, i32), Option<(i32, i32)>) {
    let header_h = header_size.map(|(_, h)| h + SECTION_GAP).unwrap_or(0);
    let keyboard_h = keyboard_size.map(|(_, h)| h + SECTION_GAP).unwrap_or(0);
    let total_h = header_h + ui_size.1 + keyboard_h;
    let top = (output_size.1 - total_h) / 2;
    let center_x = |w: i32| (output_size.0 - w) / 2;
    let header_pos = header_size.map(|(w, _)| (center_x(w), top));
    let ui_top = top + header_h;
    let ui_pos = (center_x(ui_size.0), ui_top);
    let keyboard_pos = keyboard_size.map(|(w, _)| (center_x(w), ui_top + ui_size.1 + SECTION_GAP));
    (header_pos, ui_pos, keyboard_pos)
}

/// A short, decaying horizontal shake - the wrong-password feedback
/// every mainstream lock screen (macOS, GNOME, Windows) gives alongside
/// (not instead of) a text message. A few full oscillations across
/// `SHAKE_DURATION`, amplitude decaying linearly from `SHAKE_AMPLITUDE`
/// to `0` - cheap, real motion with no animation state machine needed
/// beyond the one timestamp `shake_start` already is: this is a pure
/// function of "how long ago did the shake start", recomputed fresh every
/// frame, the same "derive from elapsed time, don't store a running
/// offset" approach `state::WindowAnim` already uses for open/close/move
/// tweens.
fn shake_offset(elapsed: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / SHAKE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    let decay = 1.0 - t;
    const CYCLES: f32 = 3.0;
    (t * CYCLES * std::f32::consts::TAU).sin() * SHAKE_AMPLITUDE * decay
}

const WEEKDAY_NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTH_NAMES: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// The current wall-clock time and date in the system's real local
/// timezone - `(hour, minute, weekday, day-of-month, month, year)`,
/// `weekday` `0..7` (Sunday-based, matching `tm_wday`) for indexing
/// `WEEKDAY_NAMES` directly. `libc::localtime_r`, not `std::time` alone:
/// `SystemTime` has no timezone concept at all (UTC only), and a lock
/// screen showing UTC on a non-UTC machine would just be showing the
/// wrong time, not a stylistic simplification - this is the one place
/// in this codebase real wall-clock time reaches the screen at all.
fn local_time_now() -> (i32, i32, i32, i32, i32, i32) {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // Safety: `now` is a valid `time_t` just obtained from `libc::time`
    // above (never null, never uninitialized), and `tm` is a plain
    // repr(C) struct `localtime_r` fully overwrites before this function
    // ever reads a single field of it - the zeroed value above is never
    // itself observed.
    unsafe { libc::localtime_r(&now, &mut tm) };
    (tm.tm_hour, tm.tm_min, tm.tm_wday, tm.tm_mday, tm.tm_mon + 1, tm.tm_year + 1900)
}

/// Fills a circle of `radius` centered at `(cx, cy)` with `color` as real
/// premultiplied-alpha BGRA, antialiased over its own outermost pixel --
/// the lock screen's own avatar, the one place this codebase draws an
/// actual disc rather than a rounded rectangle (`round_top_corners`/
/// `round_bottom_corners` cut a rect's *corners* to an arc; neither fills
/// a standalone circle, so this is a small, self-contained addition
/// rather than a reuse of either).
/// The user's own avatar picture, decoded and scaled to fill a circle of
/// `radius`, as straight BGRA. `None` when there is no avatar to show or it
/// cannot be read.
///
/// Looked for in the conventional places, most specific first:
/// `~/.face`, `~/.face.icon`, then AccountsService's own per-user icon,
/// which is where GNOME and KDE store the picture set through their
/// settings UI. These are conventionally JPEG or PNG rather than SVG, which
/// is why this needs a raster decoder rather than the `resvg` path the
/// desktop icons use.
///
/// Scaled to *cover* the circle rather than fit inside it: a portrait
/// letterboxed into a round frame looks like a mistake, and every desktop
/// that shows one crops instead. The shorter side is matched to the
/// diameter and the longer one centre-cropped.
fn load_avatar(radius: i32) -> Option<Vec<u8>> {
    let home = std::env::var("HOME").ok()?;
    let user = std::env::var("USER").unwrap_or_default();
    decode_avatar(&avatar_path(&home, &user)?, radius)
}

/// The first avatar file that exists, most specific first. Split from
/// `load_avatar` so the search order and the decoding can each be tested
/// without touching process-wide environment variables.
fn avatar_path(home: &str, user: &str) -> Option<std::path::PathBuf> {
    [
        std::path::PathBuf::from(home).join(".face"),
        std::path::PathBuf::from(home).join(".face.icon"),
        std::path::PathBuf::from("/var/lib/AccountsService/icons").join(user),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

fn decode_avatar(path: &std::path::Path, radius: i32) -> Option<Vec<u8>> {
    let decoded = match image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode() {
        Ok(image) => image,
        Err(e) => {
            log::debug!("lock: couldn't decode avatar {path:?} ({e}); falling back to the initial");
            return None;
        }
    };
    let diameter = (radius * 2).max(1) as u32;
    // `resize_to_fill` is exactly the cover-and-centre-crop described above.
    let scaled = decoded.resize_to_fill(diameter, diameter, image::imageops::FilterType::Lanczos3).to_rgba8();
    let mut out = vec![0u8; (diameter * diameter * 4) as usize];
    for (i, px) in scaled.pixels().enumerate() {
        let [r, g, b, a] = px.0;
        // Straight BGRA, the same convention `rgb_to_bgra` uses everywhere
        // else in this codebase.
        out[i * 4..i * 4 + 4].copy_from_slice(&[b, g, r, a]);
    }
    Some(out)
}

/// Blits `avatar` (a `2*radius` square, straight BGRA) into `buf` centred on
/// `(cx, cy)`, masked to a circle with a one-pixel-soft edge so it does not
/// read as a jagged cut-out.
fn blit_avatar_circle(buf: &mut [u8], width: usize, height: usize, cx: i32, cy: i32, radius: i32, avatar: &[u8]) {
    let diameter = radius * 2;
    for row in 0..diameter {
        for col in 0..diameter {
            let (dx, dy) = (col - radius, row - radius);
            let dist = ((dx * dx + dy * dy) as f32).sqrt();
            // Feathered over the outermost pixel rather than a hard test, so
            // the circle's edge is not visibly stepped.
            let coverage = (radius as f32 - dist).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let (x, y) = (cx - radius + col, cy - radius + row);
            if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
                continue;
            }
            let src = ((row * diameter + col) * 4) as usize;
            let dst = (y as usize * width + x as usize) * 4;
            let alpha = (avatar[src + 3] as f32 / 255.0) * coverage;
            if alpha <= 0.0 {
                continue;
            }
            for channel in 0..3 {
                let s = avatar[src + channel] as f32;
                let d = buf[dst + channel] as f32;
                buf[dst + channel] = (s * alpha + d * (1.0 - alpha)).round().clamp(0.0, 255.0) as u8;
            }
            let existing = buf[dst + 3] as f32 / 255.0;
            buf[dst + 3] = ((alpha + existing * (1.0 - alpha)) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn fill_circle_on_transparent(buf: &mut [u8], width: usize, height: usize, cx: i32, cy: i32, radius: i32, color: (u8, u8, u8)) {
    for y in (cy - radius - 1).max(0)..(cy + radius + 1).min(height as i32) {
        for x in (cx - radius - 1).max(0)..(cx + radius + 1).min(width as i32) {
            let (dx, dy) = ((x - cx) as f32, (y - cy) as f32);
            let dist = (dx * dx + dy * dy).sqrt();
            let coverage = (radius as f32 - dist).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let alpha = (255.0 * coverage).round() as u32;
            let premult = |c: u8| (c as u32 * alpha / 255) as u8;
            let idx = (y as usize * width + x as usize) * 4;
            buf[idx..idx + 4].copy_from_slice(&[premult(color.2), premult(color.1), premult(color.0), alpha as u8]);
        }
    }
}

/// The header shown above the password box: a large clock, the date, a
/// circular initial-letter avatar, and the username - drawn onto an
/// otherwise fully transparent canvas (`blit_glyph_on_transparent`/
/// `fill_circle_on_transparent`) so the blurred desktop shows through
/// everywhere except the glyphs/avatar themselves, the same way a real
/// macOS/GNOME/Windows lock screen's own clock floats directly over the
/// wallpaper rather than sitting inside a boxed panel. Unlike `render_ui_
/// box`, which is a genuinely opaque panel, nothing here is a filled
/// background at all.
fn render_header_box(native: &NativeLock, theme: &srdwm_core::LockConfig) -> (Vec<u8>, (i32, i32)) {
    use crate::decoration::{blit_glyph_on_transparent, find_ui_font, FONT_PIXELS};

    const WIDTH: usize = 420;
    const HEIGHT: usize = 200;
    const CLOCK_SIZE: f32 = 52.0;
    const DATE_SIZE: f32 = FONT_PIXELS;
    const AVATAR_RADIUS: i32 = 28;
    let mut buf = vec![0u8; WIDTH * HEIGHT * 4];

    let font = find_ui_font();
    let (hour, minute, weekday, day, month, _year) = local_time_now();
    let time_str = format!("{hour:02}:{minute:02}");
    let date_str = format!("{}, {} {day}", WEEKDAY_NAMES[weekday.clamp(0, 6) as usize], MONTH_NAMES[(month - 1).clamp(0, 11) as usize]);

    // A plain function, not a closure capturing `buf` - this needs to
    // interleave with other direct `buf` mutations (the avatar circle)
    // between calls, which a capturing closure can't do (it would hold
    // `buf` borrowed for its own entire lifetime, not just each call).
    #[allow(clippy::too_many_arguments)]
    fn draw_centered(buf: &mut [u8], width: usize, height: usize, font: &Option<fontdue::Font>, text: &str, y: f32, size: f32, color: (u8, u8, u8)) {
        let Some(font) = font else { return };
        let total_width: f32 = text.chars().map(|ch| font.rasterize(ch, size).0.advance_width).sum();
        let mut pen_x = (width as f32 - total_width) / 2.0;
        for ch in text.chars() {
            if ch.is_control() {
                continue;
            }
            let (metrics, coverage) = font.rasterize(ch, size);
            if metrics.width > 0 && metrics.height > 0 {
                let glyph_x = pen_x + metrics.xmin as f32;
                let glyph_y = y - metrics.height as f32 - metrics.ymin as f32;
                blit_glyph_on_transparent(buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, color);
            }
            pen_x += metrics.advance_width;
        }
    }

    draw_centered(&mut buf, WIDTH, HEIGHT, &font, &time_str, 60.0, CLOCK_SIZE, theme.text_color);
    draw_centered(&mut buf, WIDTH, HEIGHT, &font, &date_str, 84.0, DATE_SIZE, theme.text_color);

    let avatar_cy = 84.0 + 24.0 + AVATAR_RADIUS as f32;
    // The user's real picture when there is one, the coloured initial only
    // as a fallback. `~/.face` is the long-standing convention and was
    // simply never read: this drew the initial unconditionally, so a
    // machine with an avatar set still showed a letter.
    match load_avatar(AVATAR_RADIUS) {
        Some(avatar) => blit_avatar_circle(&mut buf, WIDTH, HEIGHT, WIDTH as i32 / 2, avatar_cy as i32, AVATAR_RADIUS, &avatar),
        None => {
            fill_circle_on_transparent(&mut buf, WIDTH, HEIGHT, WIDTH as i32 / 2, avatar_cy as i32, AVATAR_RADIUS, theme.avatar_bg);
            if let Some(font) = &font {
                let initial = native.username.chars().next().unwrap_or('?').to_ascii_uppercase();
                let (metrics, coverage) = font.rasterize(initial, AVATAR_RADIUS as f32);
                let glyph_x = WIDTH as i32 / 2 - metrics.width as i32 / 2;
                let glyph_y = avatar_cy as i32 - metrics.height as i32 / 2;
                blit_glyph_on_transparent(&mut buf, WIDTH, HEIGHT, glyph_x, glyph_y, &metrics, &coverage, theme.text_color);
            }
        }
    }

    let username_y = avatar_cy + AVATAR_RADIUS as f32 + 24.0;
    draw_centered(&mut buf, WIDTH, HEIGHT, &font, if native.username.is_empty() { "Locked" } else { &native.username }, username_y, DATE_SIZE, theme.text_color);

    (buf, (WIDTH as i32, HEIGHT as i32))
}

/// Draws the centered password box: rounded background, a row of dots
/// (one per character typed, never the character itself, or a dimmed
/// placeholder prompt while empty), and - depending on `LockConfig`/
/// current state - a caps-lock note and a failed-attempt message. The
/// username moved to `render_header_box` above; this box is just the
/// password field now, the way a real lock screen's own field is a
/// separate element from its clock/avatar, not one panel holding both.
/// Same rasterization primitives `decoration.rs` already uses for the
/// titlebar/context-menu/flyout (`find_system_font`/`blit_glyph`/
/// `rgb_to_bgra`), promoted to `pub(crate)` there rather than duplicated
/// here.
fn render_ui_box(native: &NativeLock, theme: &srdwm_core::LockConfig) -> (Vec<u8>, (i32, i32)) {
    use crate::decoration::{blit_glyph_over, find_ui_font, rgb_to_bgra, round_bottom_corners, round_top_corners, FONT_PIXELS, TEXT_LEFT_PADDING};

    const WIDTH: usize = 340;
    const HEIGHT: usize = 120;
    let mut buf = vec![0u8; WIDTH * HEIGHT * 4];
    // `box_opacity` 0 (the default) leaves the buffer fully transparent, so
    // the dots and status text sit straight on the blurred background with
    // no panel behind them at all - see `LockConfig::box_opacity`. Above
    // 0 the panel comes back at that opacity, border and rounded corners
    // included, exactly as it used to look at 1.0.
    let panel_alpha = (theme.box_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    if panel_alpha > 0 {
        let bg = rgb_to_bgra(theme.box_bg, panel_alpha);
        for px in buf.chunks_exact_mut(4) {
            px.copy_from_slice(&bg);
        }
    }

    let font = find_ui_font();
    let text_color = if native.show_error { theme.error_color } else { theme.text_color };

    // Centered, not left-padded like every other text row this codebase
    // draws (titlebar, context menu) - those are rows in a wide panel
    // with other content to align against; this box has nothing else in
    // it, so a left-padded dot row/placeholder read as randomly offset
    // rather than deliberately placed. Measures the row's own width first
    // (the same two-pass "measure, then centre" `render_header_box`'s own
    // `draw_centered` already does) rather than repeating that closure
    // here for one extra parameter's difference.
    // `tracking` is extra space added after every glyph. Zero for prose;
    // the password dots use it because a run of identical bullets with only
    // their own advance between them reads as one smeared blob rather than
    // as countable characters - reported as "no spacing in between the
    // obfuscated password entry". Every real password field spaces them.
    let mut draw_line_centered_tracked = |text: &str, y: f32, color: (u8, u8, u8), tracking: f32| {
        let Some(font) = &font else { return };
        let count = text.chars().filter(|c| !c.is_control()).count() as f32;
        // The trailing gap is not part of the drawn run, so it must not be
        // counted when centring or the row sits half a gap to the left.
        let glyphs: f32 = text.chars().map(|ch| font.rasterize(ch, FONT_PIXELS).0.advance_width).sum();
        let total_width: f32 = glyphs + tracking * (count - 1.0).max(0.0);
        let start_x = ((WIDTH as f32 - total_width) / 2.0).max(TEXT_LEFT_PADDING);
        let mut pen_x = start_x;
        for ch in text.chars() {
            if ch.is_control() {
                continue;
            }
            let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
            if metrics.width > 0 && metrics.height > 0 {
                let glyph_x = pen_x + metrics.xmin as f32;
                let glyph_y = y - metrics.height as f32 - metrics.ymin as f32;
                blit_glyph_over(&mut buf, WIDTH, HEIGHT, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, color);
            }
            pen_x += metrics.advance_width + tracking;
        }
    };

    // A dimmed placeholder prompt while nothing's been typed yet and
    // there's no error to show instead - an empty field with nothing in
    // it at all read as broken/unfinished, the same "looks unpolished"
    // complaint the box overall got. Real placeholder-text convention
    // (GNOME, macOS): dimmer than the real text colour, never mistakable
    // for an actual password once one is entered.
    if native.password.is_empty() && !native.show_error {
        // Dimmed toward the panel colour when there is a panel, and toward
        // plain black otherwise - mixing toward a background that is not
        // actually drawn would tint the placeholder for no visible reason.
        let toward = if panel_alpha > 0 { theme.box_bg } else { (0, 0, 0) };
        let placeholder = crate::decoration::mix_rgb(theme.text_color, toward, 0.5);
        draw_line_centered_tracked("Enter Password", 65.0, placeholder, 0.0);
    } else {
        let dots: String = std::iter::repeat_n(theme.dot_char, native.password.chars().count()).collect();
        draw_line_centered_tracked(&dots, 65.0, text_color, DOT_TRACKING);
    }

    let mut status_y = 100.0;
    if theme.show_caps_lock && native.caps_lock {
        draw_line_centered_tracked("Caps Lock is on", status_y, theme.error_color, 0.0);
        status_y += 20.0;
    }
    if theme.show_failed_attempts && native.show_error {
        let message = if native.failed_attempts > 1 { format!("{} ({} attempts)", theme.fail_message, native.failed_attempts) } else { theme.fail_message.clone() };
        draw_line_centered_tracked(&message, status_y, theme.error_color, 0.0);
    }

    // Border, drawn last so it isn't overdrawn by any fill above --
    // same convention `render_context_menu`/`render_snap_flyout` use.
    // 2px, matching `ThemeConfig::default_border_width` - a 1px line at
    // this box's size read as a thin, easy-to-miss hairline rather than a
    // deliberate frame around the box.
    const BORDER: usize = 2;
    // Border and corner rounding belong to the panel: with no panel there
    // is nothing to frame, and a floating rounded outline around bare text
    // is the "horrendus box" with its middle removed rather than the box
    // gone.
    if panel_alpha == 0 {
        return (buf, (WIDTH as i32, HEIGHT as i32));
    }
    let border_px = rgb_to_bgra(theme.box_border, 255);
    for t in 0..BORDER {
        for x in 0..WIDTH {
            buf[(t * WIDTH + x) * 4..(t * WIDTH + x) * 4 + 4].copy_from_slice(&border_px);
            let row = (HEIGHT - 1 - t) * WIDTH + x;
            buf[row * 4..row * 4 + 4].copy_from_slice(&border_px);
        }
        for y in 0..HEIGHT {
            let left = y * WIDTH + t;
            buf[left * 4..left * 4 + 4].copy_from_slice(&border_px);
            let right = y * WIDTH + WIDTH - 1 - t;
            buf[right * 4..right * 4 + 4].copy_from_slice(&border_px);
        }
    }

    // Rounded, like every other srdwm-drawn surface (titlebar, window
    // border) - `LockConfig::corner_radius` existed as a config field
    // (default 10) already, but nothing here ever actually read it, so the
    // lock box always rendered as a hard flat rectangle regardless of its
    // value. Clipping after the border fill above means the corner pixels
    // of that border get cut along with the background, the same "cut,
    // don't stroke" treatment `render_titlebar`'s own corners get.
    round_top_corners(&mut buf, WIDTH, HEIGHT, theme.corner_radius, theme.corner_radius as i32, theme.corner_radius as i32, None);
    round_bottom_corners(&mut buf, WIDTH, HEIGHT, theme.corner_radius, None);

    (buf, (WIDTH as i32, HEIGHT as i32))
}

/// One key's own static data: its label/typed character in each case,
/// and (for the three keys that aren't plain character entry) the `name`
/// `native_lock_key` already recognizes. `width` is in units of
/// `KEY_UNIT` - `1.0` for an ordinary key, wider for Backspace/Return/
/// Shift/Space, matching a real keyboard's own proportions well enough
/// to be usable without needing pixel-exact ergonomics for a lock
/// screen.
struct KeySpec {
    lower: &'static str,
    upper: &'static str,
    name: &'static str,
    width: f32,
}

const fn key(lower: &'static str, upper: &'static str) -> KeySpec {
    KeySpec { lower, upper, name: "", width: 1.0 }
}
const fn wide_key(lower: &'static str, upper: &'static str, name: &'static str, width: f32) -> KeySpec {
    KeySpec { lower, upper, name, width }
}

/// A plain, real, usable QWERTY-shaped layout - not a full XKB layout
/// translation (that needs real integration with this session's own
/// keymap, a separate and much larger piece of work), but every letter,
/// digit and every ASCII punctuation character a US layout can type.
///
/// The punctuation is not decoration. This keyboard exists for a session
/// with no reachable physical keyboard, and it previously offered only the
/// digits' own shifted symbols (`!` through `)`) - so a password
/// containing any of `-_=+[]{}\\|;:'\",.<>/?~` could not be entered at all,
/// and the only way out of the lock screen was a keyboard the user did not
/// have. Every ASCII character now has a key, shifted or unshifted.
///
/// Original doc continues: every letter,
/// digit, the digit row's own shifted symbols (covering the punctuation a
/// real password most commonly needs), Backspace, Return, Shift, and
/// Space. Scoped deliberately: a touchscreen session with no physical
/// keyboard at all needs *a* way to type a real password, not every key
/// a full desktop keyboard has.
fn keyboard_rows() -> [Vec<KeySpec>; 5] {
    [
        vec![
            key("1", "!"),
            key("2", "@"),
            key("3", "#"),
            key("4", "$"),
            key("5", "%"),
            key("6", "^"),
            key("7", "&"),
            key("8", "*"),
            key("9", "("),
            key("0", ")"),
            key("-", "_"),
            key("=", "+"),
            wide_key("Back", "Back", "BackSpace", 1.6),
        ],
        vec![
            key("q", "Q"),
            key("w", "W"),
            key("e", "E"),
            key("r", "R"),
            key("t", "T"),
            key("y", "Y"),
            key("u", "U"),
            key("i", "I"),
            key("o", "O"),
            key("p", "P"),
            key("[", "{"),
            key("]", "}"),
            key("\\", "|"),
        ],
        vec![
            key("a", "A"),
            key("s", "S"),
            key("d", "D"),
            key("f", "F"),
            key("g", "G"),
            key("h", "H"),
            key("j", "J"),
            key("k", "K"),
            key("l", "L"),
            key(";", ":"),
            key("'", "\""),
            wide_key("Enter", "Enter", "Return", 1.6),
        ],
        vec![
            wide_key("Shift", "Shift", "Shift", 1.6),
            key("z", "Z"),
            key("x", "X"),
            key("c", "C"),
            key("v", "V"),
            key("b", "B"),
            key("n", "N"),
            key("m", "M"),
            key(",", "<"),
            key(".", ">"),
            key("/", "?"),
        ],
        vec![key("`", "~"), wide_key("Space", "Space", "space", 6.0)],
    ]
}

/// Extra space after each password dot. Without it a run of identical
/// bullets renders as one smeared blob rather than countable characters.
const DOT_TRACKING: f32 = 5.0;

const KEY_UNIT: i32 = 32;
const KEY_HEIGHT: i32 = 32;
const KEY_GAP: i32 = 6;

/// Draws the on-screen keyboard (`keyboard_rows`'s own layout) as
/// individually rounded keycaps on an otherwise transparent canvas --
/// same "floats over the blurred desktop" treatment `render_header_box`
/// gives the clock, not a second opaque panel underneath the password
/// box. Returns the rendered bitmap, its size, and every key's own
/// clickable rect (in this same buffer's local space) plus what it
/// types - `CompState::native_lock_click`'s own lookup table, always
/// rebuilt together with the bitmap so the two can never drift apart.
fn render_keyboard(native: &NativeLock, theme: &srdwm_core::LockConfig) -> (Vec<u8>, (i32, i32), Vec<VirtualKey>) {
    use crate::decoration::{blit_glyph_on_transparent, fill_rect, find_system_font, FONT_PIXELS};

    let rows = keyboard_rows();
    let row_width = |row: &[KeySpec]| -> i32 {
        let units: f32 = row.iter().map(|k| k.width).sum();
        (units * KEY_UNIT as f32).round() as i32 + KEY_GAP * (row.len() as i32 - 1)
    };
    let width = rows.iter().map(|r| row_width(r)).max().unwrap_or(0).max(1) as usize;
    let height = (rows.len() as i32 * KEY_HEIGHT + (rows.len() as i32 - 1) * KEY_GAP) as usize;
    let mut buf = vec![0u8; width * height * 4];
    let mut keys = Vec::new();
    let font = find_system_font();

    // Slightly brighter than the password box's own background - a key
    // cap needs to read as a distinct, pressable surface against the
    // blurred desktop it's floating over, the same reasoning `theme.
    // box_bg` itself needs to contrast with an arbitrary, unpredictable
    // background.
    let keycap_bg = crate::decoration::mix_rgb(theme.box_bg, theme.text_color, 0.12);

    for (row_idx, row) in rows.iter().enumerate() {
        let this_row_width = row_width(row);
        let mut x = (width as i32 - this_row_width) / 2;
        let y = row_idx as i32 * (KEY_HEIGHT + KEY_GAP);
        for spec in row {
            let w = (spec.width * KEY_UNIT as f32).round() as i32;
            fill_rect(&mut buf, width, height, x, y, x + w, y + KEY_HEIGHT, keycap_bg, 220);
            let label = if native.shift { spec.upper } else { spec.lower };
            // The character the OTHER shift state would type, drawn small
            // in the cap's top-right. A physical key is labelled with both
            // (`2` and `@` on one cap) and this was labelled with only the
            // active one, so there was no way to find a symbol without
            // pressing Shift and hunting for it. Reported as not being able
            // to see the alternative characters.
            //
            // Skipped when the two are the same character (every letter
            // key differs only by case, which the cap already shows, and a
            // named key like Enter has no alternate at all).
            let alternate = if native.shift { spec.lower } else { spec.upper };
            if spec.name.is_empty() && alternate != label && !alternate.is_empty() {
                if let Some(font) = &font {
                    let size = FONT_PIXELS * 0.62;
                    let alt_width: f32 = alternate.chars().map(|ch| font.rasterize(ch, size).0.advance_width).sum();
                    let mut pen_x = x as f32 + w as f32 - alt_width - 3.0;
                    for ch in alternate.chars() {
                        let (metrics, coverage) = font.rasterize(ch, size);
                        if metrics.width > 0 && metrics.height > 0 {
                            let glyph_x = pen_x + metrics.xmin as f32;
                            let glyph_y = y as f32 + 3.0 + size - metrics.height as f32 - metrics.ymin as f32;
                            // Dimmed toward the cap so it reads as
                            // secondary rather than competing with the
                            // character the key actually types right now.
                            let dim = crate::decoration::mix_rgb(theme.text_color, keycap_bg, 0.45);
                            blit_glyph_on_transparent(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, dim);
                        }
                        pen_x += metrics.advance_width;
                    }
                }
            }
            if let Some(font) = &font {
                let total_width: f32 = label.chars().map(|ch| font.rasterize(ch, FONT_PIXELS).0.advance_width).sum();
                let mut pen_x = x as f32 + (w as f32 - total_width) / 2.0;
                for ch in label.chars() {
                    let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
                    if metrics.width > 0 && metrics.height > 0 {
                        let glyph_x = pen_x + metrics.xmin as f32;
                        let glyph_y = y as f32 + KEY_HEIGHT as f32 / 2.0 + FONT_PIXELS / 2.0 - metrics.height as f32 - metrics.ymin as f32;
                        blit_glyph_on_transparent(&mut buf, width, height, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, theme.text_color);
                    }
                    pen_x += metrics.advance_width;
                }
            }
            keys.push(VirtualKey { rect: (x, y, w, KEY_HEIGHT), name: spec.name, utf8_lower: if spec.name.is_empty() { spec.lower } else { "" }, utf8_upper: if spec.name.is_empty() { spec.upper } else { "" } });
            x += w + KEY_GAP;
        }
    }

    (buf, (width as i32, height as i32), keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_stack_layout_centers_every_present_section_on_the_output() {
        let (header_pos, ui_pos, keyboard_pos) = lock_stack_layout((800, 600), Some((400, 100)), (300, 120), Some((500, 200)));
        let header_pos = header_pos.expect("header was passed as Some");
        let keyboard_pos = keyboard_pos.expect("keyboard was passed as Some");
        assert_eq!(header_pos.0, (800 - 400) / 2);
        assert_eq!(ui_pos.0, (800 - 300) / 2);
        assert_eq!(keyboard_pos.0, (800 - 500) / 2);
        // Stacked top to bottom in a fixed order (header, then box, then
        // keyboard), each separated by exactly one `SECTION_GAP`.
        assert!(header_pos.1 < ui_pos.1);
        assert!(ui_pos.1 < keyboard_pos.1);
        assert_eq!(ui_pos.1 - (header_pos.1 + 100), SECTION_GAP);
        assert_eq!(keyboard_pos.1 - (ui_pos.1 + 120), SECTION_GAP);
    }

    #[test]
    fn lock_stack_layout_omits_absent_sections_and_still_centers_what_remains() {
        let (header_pos, ui_pos, keyboard_pos) = lock_stack_layout((800, 600), None, (300, 120), None);
        assert!(header_pos.is_none());
        assert!(keyboard_pos.is_none());
        assert_eq!(ui_pos.0, (800 - 300) / 2);
    }

    #[test]
    fn shake_offset_is_zero_at_the_very_start_and_the_very_end() {
        assert_eq!(shake_offset(Duration::ZERO), 0.0);
        let end = shake_offset(SHAKE_DURATION);
        assert!(end.abs() < 0.001, "shake should have fully decayed by its own duration, got {end}");
    }

    #[test]
    fn shake_offset_stays_within_its_configured_amplitude() {
        for ms in 0..=(SHAKE_DURATION.as_millis() as u64) {
            let offset = shake_offset(Duration::from_millis(ms));
            assert!(offset.abs() <= SHAKE_AMPLITUDE + 0.001, "offset {offset} exceeded amplitude at {ms}ms");
        }
    }

    #[test]
    fn an_avatar_is_decoded_and_scaled_to_the_circle() {
        let dir = tempfile::tempdir().unwrap();
        let face = dir.path().join(".face");
        // A real encoded image on disk, not a stub - the point is that the
        // decode path works, and `~/.face` is conventionally a photo.
        // Saved with an explicit format: `.face` carries no extension for
        // `save` to infer from, which is exactly why `decode_avatar` sniffs
        // the content (`with_guessed_format`) rather than trusting the name.
        image::RgbImage::from_fn(120, 90, |x, _| image::Rgb([x as u8, 0x40, 0x80]))
            .save_with_format(&face, image::ImageFormat::Png)
            .unwrap();

        let radius = 28;
        let pixels = decode_avatar(&face, radius).expect("a real image must decode");
        let diameter = (radius * 2) as usize;
        assert_eq!(pixels.len(), diameter * diameter * 4, "scaled to fill the circle's bounding square");
    }

    /// The real file on this machine, when there is one - a JPEG, which is
    /// the format `~/.face` conventionally is and the reason a raster
    /// decoder was needed at all. Skipped where no avatar is set.
    #[test]
    fn the_real_user_avatar_decodes_if_one_is_set() {
        let Ok(home) = std::env::var("HOME") else { return };
        let user = std::env::var("USER").unwrap_or_default();
        let Some(path) = avatar_path(&home, &user) else { return };
        let radius = 28;
        let pixels = decode_avatar(&path, radius).unwrap_or_else(|| panic!("{path:?} exists but did not decode"));
        assert_eq!(pixels.len(), ((radius * 2) * (radius * 2) * 4) as usize);
    }

    #[test]
    fn a_missing_or_unreadable_avatar_falls_back_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(avatar_path(dir.path().to_str().unwrap(), "nobody-here").is_none(), "nothing to find");

        let junk = dir.path().join(".face");
        std::fs::write(&junk, b"this is not an image").unwrap();
        assert_eq!(decode_avatar(&junk, 28), None, "a corrupt file must fall back, not panic");
    }

    #[test]
    fn dot_face_is_preferred_over_the_other_locations() {
        let dir = tempfile::tempdir().unwrap();
        let face = dir.path().join(".face");
        let icon = dir.path().join(".face.icon");
        std::fs::write(&icon, b"x").unwrap();
        std::fs::write(&face, b"x").unwrap();
        assert_eq!(avatar_path(dir.path().to_str().unwrap(), "u"), Some(face));
    }

    /// A lock screen's on-screen keyboard is the only way in for a session
    /// with no physical keyboard, so a password character it cannot type is
    /// a lockout. Every printable ASCII character must be reachable.
    #[test]
    fn every_printable_ascii_character_can_be_typed() {
        let mut typable: std::collections::HashSet<char> = std::collections::HashSet::new();
        for row in keyboard_rows() {
            for key in row {
                for text in [key.lower, key.upper] {
                    // Space is a named key; its label is a word, not the
                    // character it produces.
                    if key.name == "space" {
                        typable.insert(' ');
                    } else if key.name.is_empty() {
                        typable.extend(text.chars());
                    }
                }
            }
        }
        let missing: Vec<char> = (0x20u8..0x7f).map(char::from).filter(|c| !typable.contains(c)).collect();
        assert!(missing.is_empty(), "these characters cannot be typed on the lock screen: {missing:?}");
    }

    #[test]
    fn render_keyboard_rows_stay_within_the_reported_bitmap_width() {
        let (_, (width, _), keys) = render_keyboard(&NativeLock::new(Default::default()), &srdwm_core::LockConfig::default());
        assert!(!keys.is_empty());
        for key in &keys {
            let (x, _, w, _) = key.rect;
            assert!(x >= 0 && x + w <= width, "key rect {:?} escapes bitmap width {width}", key.rect);
        }
    }

    #[test]
    fn render_keyboard_every_key_has_either_a_name_or_typed_characters() {
        let (_, _, keys) = render_keyboard(&NativeLock::new(Default::default()), &srdwm_core::LockConfig::default());
        for key in &keys {
            let has_name = !key.name.is_empty();
            let has_chars = !key.utf8_lower.is_empty() || !key.utf8_upper.is_empty();
            assert!(has_name || has_chars, "key with rect {:?} types nothing and names nothing", key.rect);
        }
    }
}
