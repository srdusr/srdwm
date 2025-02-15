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
            self.lock.native = Some(NativeLock {
                pending_capture,
                backgrounds: HashMap::new(),
                ui_buffer: None,
                ui_size: (0, 0),
                username: current_username(),
                password: String::new(),
                failed_attempts: 0,
                show_error: false,
                checking: false,
                caps_lock: false,
                auth_rx: None,
            });
            self.lock.locked = true;
            self.set_keyboard_focus(None);
            return;
        }
        self.lock.native = Some(NativeLock {
            pending_capture,
            backgrounds: HashMap::new(),
            ui_buffer: None,
            ui_size: (0, 0),
            username: current_username(),
            password: String::new(),
            failed_attempts: 0,
            show_error: false,
            checking: false,
            caps_lock: false,
            auth_rx: None,
        });
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

    /// The password-entry box, rebuilding it first if the visible state
    /// changed since the last render. `None` while a native lock isn't
    /// actually engaged yet (still waiting on captures) - nothing should
    /// render the UI box before `state.lock.locked` is true regardless.
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
        }
        native.ui_buffer.as_ref().map(|b| (b, native.ui_size))
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
            }
        }
    }
}

/// Render elements for a native-locked output: the blurred background (if
/// this output's capture is ready) with the password box centered over
/// it. Mirrors `lock::lock_render_elements`'s shape/signature so both
/// backends can call whichever mode applies with the same pattern.
/// Takes the background/UI buffers by reference rather than `&mut
/// CompState`, same reasoning `lock::lock_render_elements`'s own doc
/// comment gives for taking a bare surface instead: both backends' render
/// loops call this while already holding a field-specific `&mut` borrow
/// (`self.udev`/the winit backend's own renderer), not a whole-`self`
/// one, so a caller has to extract these two *before* that borrow starts
/// (`CompState::native_lock_background`/`native_lock_ui`, cloned - both
/// are cheap `MemoryRenderBuffer` clones, not a deep pixel copy) and pass
/// the clones in.
pub(crate) fn native_lock_render_elements<R>(
    background: Option<&MemoryRenderBuffer>,
    ui: Option<(&MemoryRenderBuffer, (i32, i32))>,
    output_size: (i32, i32),
    renderer: &mut R,
) -> Vec<MemoryRenderBufferRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let mut elements = Vec::new();
    if let Some((ui, ui_size)) = ui {
        let pos = (((output_size.0 - ui_size.0) / 2) as f64, ((output_size.1 - ui_size.1) / 2) as f64);
        match MemoryRenderBufferRenderElement::from_buffer(renderer, pos, ui, None, None, None, Kind::Unspecified) {
            Ok(elem) => elements.push(elem),
            Err(e) => log::warn!("native lock: failed to import UI buffer: {e}"),
        }
    }
    if let Some(bg) = background {
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

/// Draws the centered password box: rounded background, a title line, a
/// row of dots (one per character typed, never the character itself),
/// and - depending on `LockConfig`/current state - a caps-lock note and
/// a failed-attempt message. Same rasterization primitives `decoration.rs`
/// already uses for the titlebar/context-menu/flyout (`find_system_font`/
/// `blit_glyph`/`rgb_to_bgra`), promoted to `pub(crate)` there rather than
/// duplicated here.
fn render_ui_box(native: &NativeLock, theme: &srdwm_core::LockConfig) -> (Vec<u8>, (i32, i32)) {
    use crate::decoration::{blit_glyph, find_system_font, rgb_to_bgra, round_bottom_corners, round_top_corners, FONT_PIXELS, TEXT_LEFT_PADDING};

    const WIDTH: usize = 360;
    const HEIGHT: usize = 170;
    let mut buf = vec![0u8; WIDTH * HEIGHT * 4];
    let bg = rgb_to_bgra(theme.box_bg, 255);
    for px in buf.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }

    let font = find_system_font();
    let text_color = if native.show_error { theme.error_color } else { theme.text_color };

    let mut draw_line = |text: &str, y: f32, color: (u8, u8, u8)| {
        let Some(font) = &font else { return };
        let baseline = y;
        let mut pen_x = TEXT_LEFT_PADDING;
        for ch in text.chars() {
            if ch.is_control() {
                continue;
            }
            let (metrics, coverage) = font.rasterize(ch, FONT_PIXELS);
            if metrics.width > 0 && metrics.height > 0 {
                let glyph_x = pen_x + metrics.xmin as f32;
                let glyph_y = baseline - metrics.height as f32 - metrics.ymin as f32;
                blit_glyph(&mut buf, WIDTH, HEIGHT, glyph_x.round() as i32, glyph_y.round() as i32, &metrics, &coverage, theme.box_bg, color);
            }
            pen_x += metrics.advance_width;
            if pen_x as usize >= WIDTH {
                break;
            }
        }
    };

    draw_line(if native.username.is_empty() { "Locked" } else { &native.username }, 40.0, theme.text_color);

    let dots: String = std::iter::repeat_n(theme.dot_char, native.password.chars().count()).collect();
    draw_line(&dots, 90.0, text_color);

    let mut status_y = 130.0;
    if theme.show_caps_lock && native.caps_lock {
        draw_line("Caps Lock is on", status_y, theme.error_color);
        status_y += 20.0;
    }
    if theme.show_failed_attempts && native.show_error {
        let message = if native.failed_attempts > 1 { format!("{} ({} attempts)", theme.fail_message, native.failed_attempts) } else { theme.fail_message.clone() };
        draw_line(&message, status_y, theme.error_color);
    }

    // Border, drawn last so it isn't overdrawn by any fill above --
    // same convention `render_context_menu`/`render_snap_flyout` use.
    // 2px, matching `ThemeConfig::default_border_width` - a 1px line at
    // this box's size read as a thin, easy-to-miss hairline rather than a
    // deliberate frame around the box.
    const BORDER: usize = 2;
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
    round_top_corners(&mut buf, WIDTH, HEIGHT, theme.corner_radius, theme.corner_radius as i32);
    round_bottom_corners(&mut buf, WIDTH, HEIGHT, theme.corner_radius);

    (buf, (WIDTH as i32, HEIGHT as i32))
}
