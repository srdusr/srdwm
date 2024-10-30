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
/// - [`MenuSource::Unity`]: still a `GMenuModel` underneath - same
///   `org.gtk.Menus`/`Gio.DBusMenuModel`-compatible wire content as
///   [`MenuSource::Gtk`] - but from `appmenu-gtk-module`'s Unity-
///   compatibility shim (a plain `Gtk.Window` with no `GtkApplication`),
///   which serves it under one `unity.xxx`-prefixed action group at the
///   menu's own path instead of separate `app.xxx`/`win.xxx` groups.
///   Confirmed live by an AGS peer session reading the actual bus content
///   for this exact case: `_GTK_MENUBAR_OBJECT_PATH` and
///   `_UNITY_OBJECT_PATH` set to the *same* `org.gtk.Menus` object, real
///   content, `unity.File`/`unity.Edit` actions - not a different wire
///   protocol, just a different action-group prefix. A consumer that
///   already speaks `Gio.DBusMenuModel` for [`Self::Gtk`] needs nothing
///   more than reading this variant to also handle this one.
/// - [`MenuSource::DbusMenu`]: a genuinely different wire protocol,
///   `com.canonical.dbusmenu` - what a client sets `_UNITY_OBJECT_PATH`
///   for *without* any `_GTK_*` atom alongside it (the original pre-GTK3.4
///   Ubuntu Unity export this atom was created for, and what `appmenu-
///   qt5`'s classic Unity-registrar model still uses), or what the
///   Wayland-native `org_kde_kwin_appmenu` protocol always carries.
///   `Gio.DBusMenuModel` cannot read this at all - pointed at a
///   `com.canonical.dbusmenu` object it silently returns an empty model,
///   the same class of silent failure as every other menu-source bug this
///   session - a consumer needs an actual dbusmenu client for this one,
///   not a differently-prefixed `GMenuModel` read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MenuSource {
    #[default]
    Gtk,
    Unity,
    DbusMenu,
}

/// The decision every X11-property-reading global-menu source needs --
/// pulled out as a pure function, shared by the Wayland backend's
/// `xwayland.rs::read_global_menu` and the native X11 backend, so it's
/// unit-testable without a real X connection and can't drift into two
/// differently-behaving copies.
///
/// `gtk_menu_path` present (real `GtkApplication` or not) always means
/// `org.gtk.Menus` - that's the only thing `_GTK_MENUBAR_OBJECT_PATH`/
/// `_GTK_APP_MENU_OBJECT_PATH` ever address, GTK-module shim included (see
/// [`MenuSource::Unity`]'s own doc comment for the live evidence). Only a
/// bare `unity_path`, with no GTK atom at all, gets [`MenuSource::DbusMenu`]:
/// that's `_UNITY_OBJECT_PATH` doing the job it was actually created for --
/// the original pre-GTK3.4 Unity/`libdbusmenu` export, and what a non-GTK
/// client (`appmenu-qt5`'s classic model) still uses it for - rather than
/// `appmenu-gtk-module` setting it as a compatibility alias alongside a GTK
/// atom that already answers the question on its own.
///
/// `is_real_gtk_application` overrides "a GTK path exists" rather than the
/// reverse: a real `GMenuModel` export (`app.`/`win.`-prefixed actions)
/// only ever comes from a `GtkApplication`, which always also sets
/// `_GTK_APPLICATION_OBJECT_PATH`/`_GTK_WINDOW_OBJECT_PATH` (or their
/// `gtk_shell1` equivalents) - if both are absent despite a GTK menubar
/// path existing, `appmenu-gtk-module` is exporting a plain window's menu
/// through its Unity-compatibility shim instead: real content, at this
/// same path, but under `unity.`-prefixed actions. Getting this wrong means
/// every menu item renders permanently insensitive against action groups
/// the app never inserted - a silent failure that reads exactly like a
/// broken app, not a wiring bug. Confirmed live by an AGS peer session
/// reading the actual exported menu content off the bus for exactly this
/// case (`_GTK_MENUBAR_OBJECT_PATH` set, `app_path`/`window_path` both
/// empty, every action `unity.*`).
pub fn classify_menu_source(gtk_menu_path: Option<String>, is_real_gtk_application: bool, unity_path: Option<String>) -> (Option<String>, MenuSource) {
    match gtk_menu_path {
        Some(path) if !is_real_gtk_application => (Some(path), MenuSource::Unity),
        Some(path) => (Some(path), MenuSource::Gtk),
        None => match unity_path {
            Some(path) => (Some(path), MenuSource::DbusMenu),
            None => (None, MenuSource::Gtk),
        },
    }
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
    /// Titlebar/border-strip corner radius, in logical pixels. Copied from
    /// `ThemeConfig::default_corner_radius` at creation (see `WindowManager
    /// ::add_window`), same as `border_color`/`border_width`; a rule's own
    /// `corner_radius` action still wins afterward.
    pub corner_radius: u32,
    /// This window's own content opacity, `0.0`..=`1.0`. Only the content
    /// (the client's own surface tree) is affected - srdwm's own
    /// decoration (titlebar/border/shadow) always renders fully opaque
    /// regardless, the same way a native macOS/Windows translucent-window
    /// effect still keeps its frame legible. Set via `srd.window.
    /// set_opacity()` or a rule's `opacity` action.
    pub opacity: f32,
    /// Per-window override of `WindowManager::resize_margin`, `None` to
    /// just inherit it. Hyprland's `extend_border_grab_area` is per-window
    /// (a `windowrule`); this is the equivalent, set via a rule's
    /// `resize_margin` action or `srd.window.set_resize_margin()`.
    pub resize_margin: Option<i32>,
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
            corner_radius: 6,
            opacity: 1.0,
            resize_margin: None,
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
/// Default width, in pixels, of the resize grab band along each window
/// edge - `WindowManager::resize_margin`'s starting value, read from
/// `general.resize_margin`, and what every `hit_test` call in this file's
/// own tests still passes directly.
///
/// Originally 10px on the reasoning that a border only a couple of pixels
/// wide is genuinely hard to grab with a mouse - which is why Hyprland
/// ships `extend_border_grab_area` and why every desktop widens this
/// beyond the visible border. But the band is *inside* the window
/// (`resize_edge_at` measures from `frame`, the client's own content rect,
/// inward), and at 10px that traded too much: reported live as ordinary
/// clicks near any edge - a link near a browser's edge, a button near a
/// panel's edge - regularly landing as a resize-edge grab instead of
/// reaching the client at all, not just an occasional near-miss. Halved to
/// 6px, which is still comfortably grabbable (about the same as a native
/// X11 border on a legacy WM) while giving content much more of its own
/// edge back. Still configurable per the doc comment above if 6px turns
/// out to be too little in the other direction for someone.
pub const RESIZE_MARGIN: i32 = 6;
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
/// How much wider than [`RESIZE_MARGIN`] a corner's own diagonal-resize
/// zone reaches, as a multiplier on whatever margin is actually in effect
/// - see `ResizeEdge::resize_edge_at`'s doc comment for why corners need
/// more room than a straight edge at all, not just a proportionally bigger
/// dead-simple hit box.
pub const CORNER_MARGIN: i32 = 3;

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
    pub fn hit_test(frame: Rect, x: i32, y: i32, decorated: bool, border_width: u32, resize_margin: i32) -> Option<TitlebarHit> {
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
            // The titlebar's own top-left corner pixels are still the
            // window's outer corner - without this, a decorated window's
            // top-left diagonal resize was completely unreachable: every y
            // inside the titlebar band returned here unconditionally,
            // before `resize_edge_at` (checked below, for every other edge)
            // ever ran. A genuine small square right at the corner (both x
            // *and* y within it), not just "close on one axis" - otherwise
            // this would claim the whole left end of the drag area at any
            // height within the titlebar, not just its actual corner.
            //
            // The top-right corner deliberately does *not* get the same
            // treatment: it's where the close button already lives, and
            // every mainstream desktop's convention is that the corner of a
            // titlebar closes the window, not resizes it. Adding a
            // competing resize zone there would trade a real, expected
            // target (close) for a rarely-wanted one at exactly the spot a
            // miss is most costly.
            let corner_zone = CORNER_MARGIN * resize_margin;
            if x <= frame.x + corner_zone && y <= frame.y + corner_zone {
                return Some(TitlebarHit::Resize(ResizeEdge::TopLeft));
            }
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
        let edge = Self::resize_edge_at(frame, x, y, decorated, resize_margin)?;
        Some(TitlebarHit::Resize(edge))
    }

    fn resize_edge_at(frame: Rect, x: i32, y: i32, decorated: bool, resize_margin: i32) -> Option<ResizeEdge> {
        let m = resize_margin;
        let top_m = if decorated { m } else { UNDECORATED_TOP_RESIZE_MARGIN };

        // A corner resize gets a bigger, prioritized hit zone than a plain
        // single edge, checked first - a diagonal drag is a harder target
        // to land than a straight edge, and several desktops (GNOME, KDE)
        // give it noticeably more room for exactly that reason. Reported
        // live: corners felt like they had no priority over sides at all,
        // which tracked - a corner previously only registered in the exact
        // pixel square where both edges' own `resize_margin` zones happened
        // to overlap (6x6px at the default margin), nothing wider.
        // `corner_top_m` still respects the undecorated window's much
        // narrower top reach (`UNDECORATED_TOP_RESIZE_MARGIN`) rather than
        // widening it too - this must not reopen the "can't grab Firefox's
        // tab strip" bug near a top corner, only the horizontal reach grows
        // there.
        let corner_m = CORNER_MARGIN * m;
        let corner_top_m = if decorated { corner_m } else { UNDECORATED_TOP_RESIZE_MARGIN };
        let corner_left = x <= frame.x + corner_m;
        let corner_right = x >= frame.right() - corner_m;
        let corner_top = y <= frame.y + corner_top_m;
        let corner_bottom = y >= frame.bottom() - corner_m;
        if corner_left && corner_top {
            return Some(ResizeEdge::TopLeft);
        }
        if corner_right && corner_top {
            return Some(ResizeEdge::TopRight);
        }
        if corner_left && corner_bottom {
            return Some(ResizeEdge::BottomLeft);
        }
        if corner_right && corner_bottom {
            return Some(ResizeEdge::BottomRight);
        }

        let near_left = x <= frame.x + m;
        let near_right = x >= frame.right() - m;
        let near_top = y <= frame.y + top_m;
        let near_bottom = y >= frame.bottom() - m;
        match (near_left, near_right, near_top, near_bottom) {
            (true, false, false, false) => Some(ResizeEdge::Left),
            (false, true, false, false) => Some(ResizeEdge::Right),
            (false, false, true, false) => Some(ResizeEdge::Top),
            (false, false, false, true) => Some(ResizeEdge::Bottom),
            _ => None,
        }
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
    fn appmenu_gtk_module_shim_is_classified_as_unity_not_gtk() {
        // The exact live case that motivated this: a GTK menubar path
        // present, but no application/window object path - confirmed by
        // an AGS peer session reading the actual exported menu content off
        // the bus and finding `unity.`-prefixed actions despite the GTK
        // atom being what resolved the path.
        let (path, source) = classify_menu_source(Some("/org/appmenu/gtk/window/0".to_string()), false, None);
        assert_eq!(path.as_deref(), Some("/org/appmenu/gtk/window/0"), "the path itself is still correct - only the label was wrong");
        assert_eq!(source, MenuSource::Unity);
    }

    #[test]
    fn real_gtk_application_export_is_still_classified_as_gtk() {
        let (path, source) = classify_menu_source(Some("/org/gtk/menus/window/1".to_string()), true, None);
        assert_eq!(path.as_deref(), Some("/org/gtk/menus/window/1"));
        assert_eq!(source, MenuSource::Gtk);
    }

    #[test]
    fn plain_unity_object_path_with_no_gtk_atom_is_real_dbusmenu() {
        let (path, source) = classify_menu_source(None, false, Some("/com/canonical/menu/1".to_string()));
        assert_eq!(path.as_deref(), Some("/com/canonical/menu/1"));
        assert_eq!(source, MenuSource::DbusMenu);
    }

    #[test]
    fn neither_path_present_is_none() {
        let (path, source) = classify_menu_source(None, false, None);
        assert_eq!(path, None);
        assert_eq!(source, MenuSource::Gtk);
    }

    #[test]
    fn close_button_is_top_right_corner_of_titlebar() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 5, f.y + 5, true, 0, RESIZE_MARGIN);
        assert_eq!(hit, Some(TitlebarHit::Close));
    }

    #[test]
    fn maximize_is_left_of_close() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - TITLEBAR_HEIGHT as i32 - 5, f.y + 5, true, 0, RESIZE_MARGIN);
        assert_eq!(hit, Some(TitlebarHit::Maximize));
    }

    #[test]
    fn middle_of_titlebar_is_drag() {
        let f = frame();
        let (cx, _) = f.center();
        let hit = ResizeEdge::hit_test(f, cx, f.y + 5, true, 0, RESIZE_MARGIN);
        assert_eq!(hit, Some(TitlebarHit::Drag));
    }

    #[test]
    fn bottom_right_corner_is_resize() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 1, f.bottom() - 1, true, 0, RESIZE_MARGIN);
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
        let hit = ResizeEdge::hit_test(f, cx, f.y + 20, false, 0, RESIZE_MARGIN);
        assert_eq!(hit, None);
    }

    #[test]
    fn undecorated_window_still_resizes_from_every_edge_including_top() {
        let f = frame();
        let (cx, _) = f.center();
        let hit = ResizeEdge::hit_test(f, cx, f.y + 1, false, 0, RESIZE_MARGIN);
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
        assert_eq!(ResizeEdge::hit_test(f, cx, f.y + 5, false, 0, RESIZE_MARGIN), None, "5px in: past the narrow undecorated band, must reach the client");
        assert_eq!(
            ResizeEdge::hit_test(f, cx, f.y + 5, true, 0, RESIZE_MARGIN),
            Some(TitlebarHit::Drag),
            "decorated: 5px in is still well inside the titlebar band, not a resize edge"
        );
    }

    #[test]
    fn outside_frame_is_none() {
        let f = frame();
        assert_eq!(ResizeEdge::hit_test(f, 0, 0, true, 0, RESIZE_MARGIN), None);
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
        assert_eq!(ResizeEdge::hit_test(f, x, cy, true, 0, RESIZE_MARGIN), None, "sanity check: with no border, this point really is outside the window");
        assert_eq!(
            ResizeEdge::hit_test(f, x, cy, true, border_width, RESIZE_MARGIN),
            Some(TitlebarHit::Resize(ResizeEdge::Left)),
            "one pixel into the actual drawn border must still register as the left edge"
        );
        // Just past the border entirely (border_width + 1 outside frame) is
        // still nothing - the fix widens the dead zone's boundary, it
        // doesn't remove it.
        assert_eq!(ResizeEdge::hit_test(f, f.x - border_width as i32 - 1, cy, true, border_width, RESIZE_MARGIN), None);
    }

    #[test]
    fn corner_resize_reaches_further_than_a_plain_edge_would() {
        // Reported live: corners felt like they had no priority over sides.
        // At the default margin (6px) a corner previously only registered
        // in the exact 6x6 overlap of both edges' own zones; a click a
        // little further along either axis fell back to a single-edge
        // resize instead, even though it still read as "aiming for the
        // corner." This point is well past the plain 6px edge margin on
        // both axes but still within `CORNER_MARGIN`'s wider corner zone.
        let f = frame();
        let corner_reach = CORNER_MARGIN * RESIZE_MARGIN;
        assert!(corner_reach > RESIZE_MARGIN, "the whole point of this test is that corner reach exceeds a plain edge's");
        let x = f.x + corner_reach - 1;
        let y = f.bottom() - corner_reach + 1;
        assert_eq!(ResizeEdge::hit_test(f, x, y, true, 0, RESIZE_MARGIN), Some(TitlebarHit::Resize(ResizeEdge::BottomLeft)));
    }

    #[test]
    fn just_past_the_corner_zone_falls_back_to_a_single_edge() {
        let f = frame();
        let corner_reach = CORNER_MARGIN * RESIZE_MARGIN;
        // Still within the corner zone vertically but past it horizontally
        // - must read as a plain bottom edge, not a corner.
        let x = f.x + corner_reach + 5;
        let y = f.bottom() - 1;
        assert_eq!(ResizeEdge::hit_test(f, x, y, true, 0, RESIZE_MARGIN), Some(TitlebarHit::Resize(ResizeEdge::Bottom)));
    }

    #[test]
    fn decorated_window_top_left_corner_of_titlebar_resizes_not_drags() {
        // The titlebar band used to claim every y within it unconditionally
        // (drag, or a button near the right edge) - a decorated window's
        // top-left diagonal resize was completely unreachable, since
        // `resize_edge_at` never even ran for a y inside the titlebar.
        let f = frame();
        let corner_reach = CORNER_MARGIN * RESIZE_MARGIN;
        let hit = ResizeEdge::hit_test(f, f.x + corner_reach - 1, f.y + corner_reach - 1, true, 0, RESIZE_MARGIN);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::TopLeft)));
    }

    #[test]
    fn decorated_window_top_right_corner_still_closes_not_resizes() {
        // Deliberately the opposite of the top-left case: the close button
        // owns the top-right corner, matching every mainstream desktop's
        // convention, rather than competing with a resize zone at exactly
        // the spot a miss is most costly.
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 1, f.y + 1, true, 0, RESIZE_MARGIN);
        assert_eq!(hit, Some(TitlebarHit::Close));
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
