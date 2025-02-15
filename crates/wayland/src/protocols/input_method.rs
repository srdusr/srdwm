//! `zwp_text_input_manager_v3` + `zwp_input_method_manager_v2`: lets a real
//! input method (fcitx5, ibus, any CJK/dead-key/emoji-picker IME) attach to
//! whichever surface has keyboard focus and draw its own candidate/
//! composition popup. Without these two globals a client that only speaks
//! text-input (most modern toolkits do, GTK4/Qt6 included) has no way to
//! tell the compositor "I have an editable text field, here is its cursor
//! rectangle" - every desktop app's search box, address bar, and chat
//! input silently loses IME support, not just an edge case.
//!
//! Focus tracking needs *no* wiring here at all: `CompState::KeyboardFocus`
//! is a plain `WlSurface`, and smithay's own blanket `impl KeyboardTarget
//! for WlSurface` already calls `seat.text_input().set_focus/.enter()/
//! .leave()` and `seat.input_method().activate_input_method()/
//! deactivate_input_method()` from inside `enter`/`leave` - which
//! `set_keyboard_focus`'s existing `keyboard.set_focus(...)` call already
//! triggers on every real focus change. The only things actually missing
//! were the two manager globals and this handler for the popup surface
//! lifecycle.

use smithay::desktop::PopupKind;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::input_method::PopupSurface as ImePopupSurface;

use crate::state::CompState;

impl smithay::wayland::input_method::InputMethodHandler for CompState {
    /// A candidate/composition window (an emoji picker, a CJK candidate
    /// list) just opened. Tracked as a regular [`PopupKind::InputMethod`]
    /// in the same [`PopupManager`](smithay::desktop::PopupManager) that
    /// already owns every `xdg_popup` - `elements::popup_render_elements`
    /// renders both kinds identically, so no separate render path is
    /// needed for this to actually become visible.
    fn new_popup(&mut self, surface: ImePopupSurface) {
        if let Err(e) = self.popups.track_popup(PopupKind::from(surface)) {
            log::warn!("input-method: failed to track popup: {e}");
        }
    }

    fn dismiss_popup(&mut self, surface: ImePopupSurface) {
        if let Some(parent) = surface.get_parent().map(|p| p.surface.clone()) {
            let _ = smithay::desktop::PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    /// The IME moved its own popup (e.g. following the text cursor as the
    /// user types) - `PopupSurface::location()` already reflects the new
    /// position; nothing else needs updating on this side, matching every
    /// other smithay-based compositor's own no-op here.
    fn popup_repositioned(&mut self, _surface: ImePopupSurface) {}

    /// Where the IME should anchor its popup, in the parent surface's own
    /// output-independent (logical, window-relative-origin) space - same
    /// geometry `elements::popup_targets` already computes for xdg popups,
    /// reused here rather than duplicated. A window not yet tracked (the
    /// activation raced ahead of its own mapping) gets a default/zero rect,
    /// same "no-op rather than an error" stance as `request_activation`
    /// above.
    fn parent_geometry(&self, parent: &WlSurface) -> smithay::utils::Rectangle<i32, smithay::utils::Logical> {
        let Some(&id) = self.surface_to_id.get(parent) else {
            return smithay::utils::Rectangle::default();
        };
        let (geometry, decorated) = {
            let wm = self.wm.borrow();
            let Some(w) = wm.window(id) else {
                return smithay::utils::Rectangle::default();
            };
            (w.geometry, w.decorated)
        };
        let band = if decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
        // `content_offset`/`effective_frame`: same corrections every other
        // real position/size computation in this codebase applies (see
        // `state/geometry.rs::effective_frame`'s doc comment) - missed
        // here originally, so an IME popup anchored against a CSD window's
        // raw, unshifted geometry instead of its real visible content,
        // same class of drift as the border/screenshot gaps fixed
        // elsewhere.
        let content_offset = self.id_to_window.get(&id).map(|w| w.geometry().loc).unwrap_or_default();
        // `frame.height` includes the titlebar band (see `effective_frame`'s
        // own doc comment) - subtracted back out here since this rect is
        // meant to cover the content area only, matching the original
        // (pre-fix) code's own intent for `w.geometry.height`.
        let frame = self.effective_frame(id, geometry);
        let content_height = (frame.height as i32 - band).max(0);
        smithay::utils::Rectangle::new((frame.x - content_offset.x, frame.y + band - content_offset.y).into(), (frame.width as i32, content_height).into())
    }
}
