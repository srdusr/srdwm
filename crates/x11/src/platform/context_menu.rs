//! Right-click titlebar window menu - the X11 half of `srdwm_core::
//! context_menu`. The Wayland backend renders this into a shared
//! compositor buffer; X11 has no equivalent (there is no single "every
//! surface" compositor here, just ordinary X windows), so this draws it
//! into its own small override-redirect popup window with the same GC/
//! font `redraw_decoration` already uses for the titlebar itself.
//!
//! Row set and hit-testing come from `srdwm_core::context_menu`, unchanged
//! from the Wayland backend - see that module's own doc comment for why
//! it moved there. Foreign-toplevel state broadcasting
//! (`foreign_toplevel::send_state` on the Wayland side) has no X11
//! equivalent - that protocol is Wayland-only - so `run_context_menu_
//! action` below is otherwise the same action set with that one line
//! dropped.

use super::*;
use srdwm_core::context_menu::{ContextMenu, MenuAction};
use x11rb::protocol::xproto::{ChangeGCAux, CoordMode, Point};

impl X11Platform {
    /// Opens the menu for `window` with its top-left corner at `pos`
    /// (root-window pixels, wherever the right-click landed), clamped so
    /// it never opens off the right/bottom edge of the screen. Grabs the
    /// pointer for the duration: X11 has no single compositor-level input
    /// dispatch to intercept every click the way the Wayland backend's
    /// `input/pointer.rs` does, so an active grab on the root window is
    /// what makes "click anywhere else dismisses the menu" work at all --
    /// without it, a click over some other client's window would go
    /// straight to that client and never reach us.
    pub(super) fn open_context_menu(&mut self, window: WindowId, pos: (i32, i32)) -> PlatformResult<()> {
        let Some(mut menu) = ({
            let wm = self.wm.borrow();
            ContextMenu::open(&wm, window, pos)
        }) else {
            return Ok(());
        };

        let screen = &self.conn.setup().roots[0];
        let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);
        let x = menu.pos.0.max(0).min((sw - menu.width as i32).max(0));
        let y = menu.pos.1.max(0).min((sh - menu.height()).max(0));
        menu.pos = (x, y);

        let popup = self.conn.generate_id().map_err(err)?;
        let aux = CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::EXPOSURE)
            .background_pixel(screen.white_pixel);
        self.conn
            .create_window(COPY_DEPTH_FROM_PARENT, popup, self.root, x as i16, y as i16, menu.width as u16, menu.height() as u16, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            .map_err(err)?;
        self.conn.map_window(popup).map_err(err)?;
        self.conn.configure_window(popup, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).map_err(err)?;

        // `owner_events: false` - every button event while this grab is
        // active is reported against `grab_window` (root) regardless of
        // which window the pointer is physically over, carrying the same
        // absolute `root_x`/`root_y` the normal per-window path already
        // uses for hit-testing. Best-effort: a failed grab (another
        // client already holds one, vanishingly rare in practice) still
        // leaves the menu visible and closeable by re-clicking its own
        // rows, just without the "click elsewhere" dismissal.
        let _ = self
            .conn
            .grab_pointer(false, self.root, EventMask::BUTTON_PRESS, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, x11rb::CURRENT_TIME)
            .map_err(err)?
            .reply();
        self.conn.flush().map_err(err)?;

        self.context_menu = Some((menu, popup));
        self.redraw_context_menu()
    }

    /// Repaints every row into the popup window - called once on open
    /// (there's no live hover highlight here either, same as the Wayland
    /// backend's own `render_context_menu`) and again on `Expose` (the
    /// popup has no backing store, so anything that uncovers it needs a
    /// real repaint, unlike the Wayland side where the buffer is already
    /// composited).
    pub(super) fn redraw_context_menu(&mut self) -> PlatformResult<()> {
        let Some((menu, popup)) = &self.context_menu else { return Ok(()) };
        let popup = *popup;
        let (width, total_height) = (menu.width, menu.height());
        let theme = self.wm.borrow().theme;
        let bg = rgb_to_pixel(theme.titlebar_bg);
        let fg = rgb_to_pixel(theme.titlebar_fg_focused);

        self.conn.change_gc(self.gc, &ChangeGCAux::new().foreground(bg)).map_err(err)?;
        self.conn.poly_fill_rectangle(popup, self.gc, &[Rectangle { x: 0, y: 0, width: width as u16, height: total_height as u16 }]).map_err(err)?;
        self.conn.change_gc(self.gc, &ChangeGCAux::new().foreground(fg).font(self.font)).map_err(err)?;

        // Per-row heights, not a uniform `row_height * i` - a `Separator`/
        // `Header` row is shorter than a real item (`ContextMenu::row_
        // height_for`), same variable-height model the Wayland backend's
        // own `render_context_menu` now uses; `menu.row_y(i)` is exactly
        // how that side computes each row's own top too, so the two
        // backends can't drift onto different geometry for the same menu
        // data. No dimmed-text treatment for a `Header` row here (this
        // backend draws everything through one single-colour `GC`, no
        // per-pixel blending the way the Wayland renderer's own `mix_rgb`
        // has) - feature parity (non-interactive, correctly sized) over
        // pixel parity, matching this backend's existing bar elsewhere.
        for (i, (label, action)) in menu.items.iter().enumerate() {
            let row_y = menu.row_y(i);
            let row_height = menu.row_height_for(i) as i32;
            if matches!(action, MenuAction::Separator) {
                let mid = row_y + row_height / 2;
                self.conn.poly_line(CoordMode::ORIGIN, popup, self.gc, &[Point { x: 8, y: mid as i16 }, Point { x: width as i16 - 8, y: mid as i16 }]).map_err(err)?;
                continue;
            }
            let baseline = row_y + row_height * 3 / 4;
            self.conn.image_text8(popup, self.gc, 10, baseline as i16, label.as_bytes()).map_err(err)?;
        }
        self.conn.flush().map_err(err)?;
        Ok(())
    }

    /// Closes the currently-open menu, if any - a no-op otherwise, so
    /// every dismissal path (a row picked, a click outside, the window it
    /// belongs to closing underneath it) can call this unconditionally.
    pub(super) fn close_context_menu(&mut self) -> PlatformResult<()> {
        if let Some((_, popup)) = self.context_menu.take() {
            self.conn.ungrab_pointer(x11rb::CURRENT_TIME).map_err(err)?;
            self.conn.destroy_window(popup).map_err(err)?;
            self.conn.flush().map_err(err)?;
        }
        Ok(())
    }

    /// Runs whichever action a click on `row` selected - the X11
    /// equivalent of the Wayland backend's `state/menu.rs::run_context_
    /// menu_action`, same action set minus the Wayland-only foreign-
    /// toplevel broadcast.
    pub(super) fn run_context_menu_action(&mut self, window: WindowId, action: MenuAction) -> PlatformResult<()> {
        match action {
            MenuAction::Minimize => {
                self.wm.borrow_mut().minimize_window(window);
                if let Some(frame) = self.frame_for(window) {
                    self.conn.unmap_window(frame).map_err(err)?;
                }
            }
            MenuAction::ToggleMaximize => {
                self.wm.borrow_mut().toggle_maximize(window);
                self.sync_geometry(window)?;
            }
            MenuAction::ToggleFullscreen => {
                self.wm.borrow_mut().toggle_fullscreen(window);
                self.sync_geometry(window)?;
            }
            MenuAction::ToggleFloating => {
                self.wm.borrow_mut().toggle_floating(window);
                self.sync_geometry(window)?;
            }
            MenuAction::ToggleAlwaysOnTop => {
                self.wm.borrow_mut().toggle_always_on_top(window);
            }
            MenuAction::MoveToWorkspace(workspace) => {
                self.wm.borrow_mut().move_window_to_workspace(window, workspace);
            }
            // Same reasoning as the Wayland backend's own `redraw_every_
            // decoration` (`crates/wayland/src/state/menu.rs`): `srd set
            // button_style`/`button_side` are deliberately scoped to
            // "takes effect on windows created after this call" because
            // `crates/platform` is backend-agnostic and has no redraw hook
            // to call - this menu action runs as this backend's own
            // code, already holding `&mut self`, so it can and should
            // repaint every open titlebar immediately instead.
            MenuAction::CycleButtonStyle => {
                let mut wm = self.wm.borrow_mut();
                wm.theme.traffic_light_buttons = !wm.theme.traffic_light_buttons;
                drop(wm);
                self.redraw_all_decorations()?;
            }
            MenuAction::CycleButtonSide => {
                let mut wm = self.wm.borrow_mut();
                wm.theme.buttons_left = !wm.theme.buttons_left;
                drop(wm);
                self.redraw_all_decorations()?;
            }
            MenuAction::Close => self.request_close(window)?,
            MenuAction::Separator | MenuAction::Header => {}
        }
        self.conn.flush().map_err(err)?;
        Ok(())
    }
}
