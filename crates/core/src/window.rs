use crate::geometry::Rect;

pub type WindowId = u64;

/// A window's exported application/window menu, as a D-Bus *address* --
/// bus name plus object paths - never the menu's actual content.
///
/// The content is a `GMenuModel` already exported over `org.gtk.Menus`/
/// `org.gtk.Actions`, which GTK4 consumes natively (`Gio.DBusMenuModel`,
/// `Gtk.PopoverMenuBar.new_from_model()`) with full submenus, toggles,
/// accelerators and icons - carrying the model itself over a Wayland
/// protocol instead would mean hand-marshalling and hand-rendering it for
/// strictly worse fidelity. These four strings are the only part a
/// compositor can supply that a client-side global-menu shell can't get
/// any other way: on XWayland this is `_GTK_UNIQUE_BUS_NAME`/
/// `_GTK_MENUBAR_OBJECT_PATH`/`_GTK_APPLICATION_OBJECT_PATH`/
/// `_GTK_WINDOW_OBJECT_PATH`; on Wayland-native surfaces it's GTK's own
/// private `gtk_shell1` protocol's `gtk_surface1.set_dbus_properties`
/// request, which carries the identical four fields under different
/// names. See `crates/wayland/src/xwayland.rs` and `gtk_shell.rs` for
/// where each backend actually populates this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalMenu {
    pub bus_name: String,
    /// The app or window's menu bar, whichever the client exported --
    /// `menubar_path` (a full menu bar) if set, else `app_menu_path` (just
    /// the single app-level menu older/simpler clients export instead).
    /// Which of the two (or the pre-`_GTK_*` Unity path) actually won is
    /// [`Self::source`] - load-bearing, not cosmetic: see its own doc
    /// comment.
    pub menu_path: Option<String>,
    pub app_path: Option<String>,
    pub window_path: Option<String>,
    pub source: MenuSource,
}

/// Which export flavour [`GlobalMenu::menu_path`] actually came from --
/// the two address their actions under different D-Bus action-group
/// prefixes, and getting this wrong doesn't fail loudly: the menu still
/// renders, every item just comes up permanently insensitive, which reads
/// exactly like an app that exported a broken menu rather than a
/// consumer that resolved the wrong prefix.
///
/// - [`MenuSource::Gtk`]: a real `GMenuModel`. Items reference actions as
///   `app.xxx`/`win.xxx`; a consumer must insert two action groups, under
///   prefixes `"app"` and `"win"`, from [`GlobalMenu::app_path`]/
///   [`GlobalMenu::window_path`] respectively.
/// - [`MenuSource::Unity`]: the older Ubuntu Unity-era export
///   (`_UNITY_OBJECT_PATH`, still relevant for some Qt platform-theme
///   builds). Items reference actions as `unity.xxx`, all against one
///   group at the menu's own path - a consumer inserts a single group
///   under prefix `"unity"` instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MenuSource {
    #[default]
    Gtk,
    Unity,
}

/// State of a single managed window. This is platform-independent: backends
/// (X11, Wayland, ...) own the real surface/client handle and keep a `Window`
/// in sync with it via `srdwm_core::WindowManager`.
#[derive(Debug, Clone)]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub app_id: String,
    /// X11 `WM_CLASS`'s *instance* half (`WM_CLASS` is `"instance\0class\0"`;
    /// `app_id` above holds the class half, matching Wayland's `app_id`
    /// concept). Always empty on Wayland/XWayland, which has no equivalent.
    pub instance: String,
    pub geometry: Rect,
    /// Geometry to restore to when un-maximizing.
    pub restore_geometry: Option<Rect>,
    pub decorated: bool,
    /// `decorated`'s value from just before entering fullscreen, restored
    /// on exit - see `WindowManager::toggle_fullscreen`'s doc comment on
    /// why this can't just hardcode `true` back.
    pub restore_decorated: Option<bool>,
    pub floating: bool,
    pub minimized: bool,
    /// Whether this window belongs to the scratchpad pool - see
    /// `WindowManager::scratchpad_add`/`scratchpad_show`'s doc comments.
    /// Persists across show/hide toggles (`minimized` is what actually
    /// gates visibility); a window never sets this itself, only `srd.window.
    /// scratchpad()`/the equivalent keybinding does.
    pub scratchpad: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub always_on_top: bool,
    pub border_color: (u8, u8, u8),
    pub border_width: u32,
    pub workspace: usize,
    pub monitor: u32,
    /// Whether `WindowManager`'s class/title-matched rules have already
    /// been evaluated (and, if matched, applied) for this window.
    ///
    /// `add_window` matches rules once, at creation, but a native Wayland
    /// client's `title`/`app_id` are still empty at that moment (they
    /// arrive on a later commit - see the Wayland backend's
    /// `sync_toplevel_metadata` doc comment); matching then would silently
    /// fail every class-based rule. Left `false` so a backend can retry
    /// the match once real identity is known, without ever re-matching
    /// after that (rule actions apply once, not on every subsequent title
    /// change).
    pub rules_applied: bool,
    /// Set by `WindowManager::toggle_maximize`/`toggle_fullscreen` to the
    /// geometry `self.geometry` just moved *from*, whenever that move
    /// should be animated. A backend's `sync_geometry` takes (reads and
    /// clears) this once per change to start a tween toward the new
    /// `geometry`; left `None` for changes that must track 1:1 instead
    /// (interactive drag/resize), which never set it.
    pub anim_from: Option<Rect>,
    /// This window's global-menu D-Bus address, if the client has exported
    /// one. See [`GlobalMenu`]'s own doc comment.
    pub global_menu: Option<GlobalMenu>,
}

impl Window {
    pub fn new(id: WindowId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            app_id: String::new(),
            instance: String::new(),
            geometry: Rect::new(0, 0, 640, 480),
            restore_geometry: None,
            decorated: true,
            restore_decorated: None,
            floating: false,
            minimized: false,
            scratchpad: false,
            maximized: false,
            fullscreen: false,
            always_on_top: false,
            border_color: (136, 192, 208), // Nord accent, matches legacy theme default
            border_width: 2,
            workspace: 0,
            monitor: 0,
            rules_applied: false,
            anim_from: None,
            global_menu: None,
        }
    }
}

/// The height, in pixels, of the drawn title bar. Shared between backends so
/// hit-testing and rendering agree on the same band.
pub const TITLEBAR_HEIGHT: u32 = 30;
/// Width of the resize grab band along each window edge.
///
/// 10px rather than a hairline: this is grabbed with a mouse, and a border
/// only a couple of pixels wide is genuinely hard to hit - which is why
/// Hyprland ships `extend_border_grab_area` and why every desktop widens
/// this beyond the visible border. The band is inside the window, so it
/// costs a few pixels of client edge; that is the right trade for making
/// resize reliably grabbable without a keyboard.
pub const RESIZE_MARGIN: i32 = 10;
/// Top-edge resize margin for an *undecorated* window specifically --
/// narrower than [`RESIZE_MARGIN`] on purpose.
///
/// An undecorated (client-side-decorated) window has no titlebar band for
/// srdwm to treat as a drag handle - Firefox's own tab strip, concretely --
/// so the client's own header area sits directly at `frame.y` with nothing
/// srdwm-drawn to grab. The client detects a drag on its own header and
/// asks to be moved via `xdg_toplevel.move`, but only for clicks that
/// actually reach it as a normal button press; the full 10px `RESIZE_MARGIN`
/// swallowed every click within the first 10 rows of the window - including
/// most of a typical natural grab point near the top of a tab strip - as a
/// top-edge resize instead, so the client's own move request never fired.
/// Reported live as "can't drag-move Firefox from its own top bar."
/// Resize-from-the-top-edge still works (a deliberate earlier trade-off --
/// see `undecorated_window_still_resizes_from_every_edge_including_top`'s
/// own comment - since an undecorated window is still a window), just from
/// a much narrower band that a click meant to grab the tab strip is very
/// unlikely to land in by accident.
pub const UNDECORATED_TOP_RESIZE_MARGIN: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeEdge {
    /// Determine which titlebar button (if any) a point within the titlebar
    /// band falls on. Buttons are laid out right-aligned: close, maximize, minimize.
    ///
    /// `decorated` must reflect the window's *actual* current state, not
    /// just whether it usually draws one: `frame` always reserves
    /// `TITLEBAR_HEIGHT` at the top regardless of whether anything is drawn
    /// there (placement never shrinks a window's allocated geometry just
    /// because a rule or CSD negotiation later turns decoration off - see
    /// `sync_geometry`'s own doc comment on that split). Applying the
    /// titlebar-band/button logic unconditionally meant an *undecorated*
    /// window's own content in that top band - Firefox's tab strip and URL
    /// bar, concretely, once `decorated = false` actually started applying
    /// to it - silently ate every click there as a phantom
    /// drag/close/maximize/minimize hit instead of ever reaching the
    /// client. Resize-from-edge still applies either way: an undecorated
    /// window is still a window, and dragging its (invisible) edge to
    /// resize is still expected to work.
    pub fn hit_test(frame: Rect, x: i32, y: i32, decorated: bool, border_width: u32) -> Option<TitlebarHit> {
        // Border strips render *outside* `frame` (`decoration::
        // border_strips`, `border_width` pixels past each edge) - without
        // widening the containment check to match, those visible pixels
        // were a dead zone: `frame.contains_point` rejected them outright,
        // so hovering the border itself (not just just inside it) showed no
        // resize cursor and couldn't be grabbed, even though it's what
        // visually reads as the window's actual edge. `resize_edge_at`
        // itself needs no matching change - its margin comparisons
        // (`x <= frame.x + m`, etc.) already treat anything at or outside
        // `frame`'s own edge as maximally "near", border pixels included.
        let bw = border_width as i32;
        let outer = Rect::new(frame.x - bw, frame.y - bw, frame.width + 2 * border_width, frame.height + 2 * border_width);
        if !outer.contains_point(x, y) {
            return None;
        }
        if decorated && y < frame.y + TITLEBAR_HEIGHT as i32 {
            const BUTTON: i32 = TITLEBAR_HEIGHT as i32;
            let right = frame.right();
            if x >= right - BUTTON {
                return Some(TitlebarHit::Close);
            }
            if x >= right - BUTTON * 2 {
                return Some(TitlebarHit::Maximize);
            }
            if x >= right - BUTTON * 3 {
                return Some(TitlebarHit::Minimize);
            }
            return Some(TitlebarHit::Drag);
        }
        let edge = Self::resize_edge_at(frame, x, y, decorated)?;
        Some(TitlebarHit::Resize(edge))
    }

    fn resize_edge_at(frame: Rect, x: i32, y: i32, decorated: bool) -> Option<ResizeEdge> {
        let m = RESIZE_MARGIN;
        let top_m = if decorated { m } else { UNDECORATED_TOP_RESIZE_MARGIN };
        let near_left = x <= frame.x + m;
        let near_right = x >= frame.right() - m;
        let near_top = y <= frame.y + top_m;
        let near_bottom = y >= frame.bottom() - m;
        Some(match (near_left, near_right, near_top, near_bottom) {
            (true, _, true, _) => ResizeEdge::TopLeft,
            (_, true, true, _) => ResizeEdge::TopRight,
            (true, _, _, true) => ResizeEdge::BottomLeft,
            (_, true, _, true) => ResizeEdge::BottomRight,
            (true, false, false, false) => ResizeEdge::Left,
            (false, true, false, false) => ResizeEdge::Right,
            (false, false, true, false) => ResizeEdge::Top,
            (false, false, false, true) => ResizeEdge::Bottom,
            _ => return None,
        })
    }

    /// Apply a pointer delta to `original` geometry along this edge, honoring
    /// the given minimum size.
    pub fn apply_delta(self, original: Rect, dx: i32, dy: i32, min_w: u32, min_h: u32) -> Rect {
        let mut r = original;
        let min_w = min_w as i32;
        let min_h = min_h as i32;
        match self {
            ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
                let new_w = (original.width as i32 - dx).max(min_w);
                r.x = original.right() - new_w;
                r.width = new_w as u32;
            }
            _ => {}
        }
        match self {
            ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => {
                r.width = (original.width as i32 + dx).max(min_w) as u32;
            }
            _ => {}
        }
        match self {
            ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
                let new_h = (original.height as i32 - dy).max(min_h);
                r.y = original.bottom() - new_h;
                r.height = new_h as u32;
            }
            _ => {}
        }
        match self {
            ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight => {
                r.height = (original.height as i32 + dy).max(min_h) as u32;
            }
            _ => {}
        }
        r
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarHit {
    Drag,
    Close,
    Maximize,
    Minimize,
    Resize(ResizeEdge),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Rect {
        Rect::new(100, 100, 400, 300)
    }

    #[test]
    fn close_button_is_top_right_corner_of_titlebar() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 5, f.y + 5, true, 0);
        assert_eq!(hit, Some(TitlebarHit::Close));
    }

    #[test]
    fn maximize_is_left_of_close() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - TITLEBAR_HEIGHT as i32 - 5, f.y + 5, true, 0);
        assert_eq!(hit, Some(TitlebarHit::Maximize));
    }

    #[test]
    fn middle_of_titlebar_is_drag() {
        let f = frame();
        let (cx, _) = f.center();
        let hit = ResizeEdge::hit_test(f, cx, f.y + 5, true, 0);
        assert_eq!(hit, Some(TitlebarHit::Drag));
    }

    #[test]
    fn bottom_right_corner_is_resize() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 1, f.bottom() - 1, true, 0);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::BottomRight)));
    }

    /// The bug this guards against: an undecorated window's own content in
    /// its top `TITLEBAR_HEIGHT` band (Firefox's tab strip/URL bar, once
    /// `decorated = false` actually applies to it) was silently swallowed
    /// as a phantom drag hit instead of ever reaching the client, since the
    /// titlebar-band check used to run unconditionally.
    #[test]
    fn undecorated_window_has_no_titlebar_band() {
        let f = frame();
        let (cx, _) = f.center();
        // Inside the old phantom titlebar band (< TITLEBAR_HEIGHT) but
        // outside RESIZE_MARGIN, so a real resize edge can't also explain a
        // `None` here - undecorated, this must not be treated as
        // decoration (or a resize edge) at all, just plain content.
        let hit = ResizeEdge::hit_test(f, cx, f.y + 20, false, 0);
        assert_eq!(hit, None);
    }

    #[test]
    fn undecorated_window_still_resizes_from_every_edge_including_top() {
        let f = frame();
        let (cx, _) = f.center();
        let hit = ResizeEdge::hit_test(f, cx, f.y + 1, false, 0);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::Top)));
    }

    #[test]
    fn undecorated_top_resize_band_is_much_narrower_than_decorated() {
        // Regression test: an undecorated window's own header (Firefox's
        // tab strip, concretely) has no srdwm-drawn titlebar to grab, so a
        // click meant to drag-move it via the client's own `xdg_toplevel.
        // move` has to actually reach the client - the full `RESIZE_MARGIN`
        // (10px) swallowed most of a natural grab point near the top as a
        // resize instead. A *decorated* window's top band is unaffected --
        // it already has TITLEBAR_HEIGHT worth of unambiguous drag space
        // above where `RESIZE_MARGIN` even starts to matter.
        let f = frame();
        let (cx, _) = f.center();
        assert_eq!(ResizeEdge::hit_test(f, cx, f.y + 5, false, 0), None, "5px in: past the narrow undecorated band, must reach the client");
        assert_eq!(
            ResizeEdge::hit_test(f, cx, f.y + 5, true, 0),
            Some(TitlebarHit::Drag),
            "decorated: 5px in is still well inside the titlebar band, not a resize edge"
        );
    }

    #[test]
    fn outside_frame_is_none() {
        let f = frame();
        assert_eq!(ResizeEdge::hit_test(f, 0, 0, true, 0), None);
    }

    #[test]
    fn border_pixels_are_hoverable_not_a_dead_zone() {
        // Regression test: `decoration::border_strips` draws the border
        // `border_width` pixels *outside* `frame`, but hit-testing only
        // checked `frame` itself - so the visible border was a dead zone
        // that showed no resize cursor and couldn't be grabbed, even
        // though it's what visually reads as the window's edge.
        let f = frame();
        let (_, cy) = f.center();
        let border_width = 2;
        // One pixel into the border strip, past the left edge.
        let x = f.x - 1;
        assert_eq!(ResizeEdge::hit_test(f, x, cy, true, 0), None, "sanity check: with no border, this point really is outside the window");
        assert_eq!(
            ResizeEdge::hit_test(f, x, cy, true, border_width),
            Some(TitlebarHit::Resize(ResizeEdge::Left)),
            "one pixel into the actual drawn border must still register as the left edge"
        );
        // Just past the border entirely (border_width + 1 outside frame) is
        // still nothing - the fix widens the dead zone's boundary, it
        // doesn't remove it.
        assert_eq!(ResizeEdge::hit_test(f, f.x - border_width as i32 - 1, cy, true, border_width), None);
    }

    #[test]
    fn resize_right_edge_grows_width_only() {
        let r = Rect::new(0, 0, 200, 100);
        let out = ResizeEdge::Right.apply_delta(r, 50, 999, 50, 50);
        assert_eq!(out, Rect::new(0, 0, 250, 100));
    }

    #[test]
    fn resize_left_edge_moves_x_and_shrinks_width() {
        let r = Rect::new(100, 0, 200, 100);
        let out = ResizeEdge::Left.apply_delta(r, 30, 0, 50, 50);
        assert_eq!(out, Rect::new(130, 0, 170, 100));
    }

    #[test]
    fn resize_respects_minimum_size() {
        let r = Rect::new(0, 0, 100, 100);
        let out = ResizeEdge::Right.apply_delta(r, -500, 0, 50, 50);
        assert_eq!(out.width, 50);
    }
}
