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

/// Whether `app_id` almost certainly belongs to an application that draws
/// its own header bar (a `GtkHeaderBar`/`Adw.HeaderBar` widget embedded
/// directly in its content, unconditionally) regardless of whatever
/// `xdg-decoration` mode actually gets negotiated - see
/// `crates/wayland/src/protocols.rs`'s `XdgDecorationHandler` doc comment
/// for why the protocol itself can't tell such an app apart from a normal
/// one: both Firefox and Nemo negotiate a decoration mode fine, and still
/// draw a second title row under srdwm's server-side one regardless.
/// Confirmed live for both (a screenshot showing two stacked bars) before
/// either got its own `decorated = false` entry in `rules.lua`.
///
/// `org.gnome.*` app ids are the one case general enough to catch here
/// instead of needing a `rules.lua` entry added for each one as it's
/// discovered live: the GNOME HIG mandates every one of GNOME's own apps
/// use an embedded header bar, with no exceptions, so the namespace alone
/// is enough to know in advance. Deliberately narrow: a third-party GTK4/
/// libadwaita app under `io.github.*`, or any other reverse-DNS scheme, is
/// left to `rules.lua`'s per-app list instead - those toolkits don't share
/// GNOME's HIG mandate, so guessing from the app id alone there would
/// misclassify plenty of ordinary, well-behaved server-side-decorated apps
/// that also happen to use a reverse-DNS-style id.
///
/// `org.pwmt.*` added the same way, on the same evidence: zathura
/// (`org.pwmt.zathura`) reported live as a double titlebar - confirmed in
/// a nested compositor, screenshotted, two stacked rows with visibly
/// different button styles (srdwm's own configured style on top, zathura's
/// own girara-drawn row underneath). PWMT's own small set of tools (girara-
/// based, zathura being the only one in common use) all share the same
/// always-draws-its-own-header behaviour Firefox/Nemo needed a rule for,
/// so the namespace is as safe a bet here as GNOME's own.
pub fn likely_draws_own_titlebar(app_id: &str) -> bool {
    let app_id = app_id.to_ascii_lowercase();
    app_id.starts_with("org.gnome.") || app_id.starts_with("org.pwmt.")
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
    /// Whether this window declared itself a dialog/utility window
    /// belonging to another one, not a normal top-level app window --
    /// a native `xdg_toplevel`'s own `parent()` (`set_parent`), or an
    /// XWayland `X11Surface`'s ICCCM `WM_TRANSIENT_FOR` hint
    /// (`is_transient_for()`). Backend-set (the wayland crate reads
    /// whichever real accessor applies, refreshed on every decoration
    /// redraw), same as `decorated` itself; `core` has no protocol
    /// concept of its own to derive this from. Requested directly: a
    /// dialog's titlebar should show only a close button, no traffic
    /// lights - see `hit_test`'s and `decoration::render_titlebar`'s own
    /// use of this for what actually changes.
    pub is_dialog: bool,
    /// Whether the client says it can actually be resized - `false` when
    /// it pinned its minimum and maximum size to the same value (a native
    /// `xdg_toplevel`'s `set_min_size`/`set_max_size`, or an XWayland
    /// window's ICCCM size hints). Backend-set on every decoration redraw,
    /// same as `is_dialog` above, for the same reason: `core` has no
    /// protocol of its own to read it from. Defaults to `true`, so a
    /// client that never declares limits - the common case - is treated
    /// as resizable.
    ///
    /// Consumed by the `dynamic` titlebar button mode: a window that
    /// cannot be resized cannot meaningfully be maximized either, so
    /// offering the button is offering a no-op. Every mainstream desktop
    /// (GNOME, KDE, Windows) hides or disables it in exactly this case.
    /// Asked for as titlebars with "buttons of the program/dynamic".
    pub resizable: bool,
    /// This window's own minimum size, in physical pixels. Defaults to the
    /// global [`crate::placement::MIN_WINDOW_WIDTH`]/`MIN_WINDOW_HEIGHT`.
    ///
    /// Two sources, in order of precedence: a `min_size` window rule, and
    /// the client's own declared minimum (`xdg_toplevel.set_min_size`, or
    /// an XWayland window's ICCCM size hints), which the backend fills in
    /// the same way it fills `resizable`.
    ///
    /// One global minimum for every window is wrong in both directions: it
    /// is far too small for an application that needs room to lay out at
    /// all, and too large for a small utility or palette window. Asked for
    /// as "different windows should have minimum sizes depending on what
    /// window is".
    pub min_size: (u32, u32),
    /// Set when `min_size` came from a `min_size` window rule rather than
    /// from the client or the default. The backend refreshes a client's
    /// declared minimum on every decoration redraw, and must not overwrite
    /// a deliberate rule with it.
    pub min_size_from_rule: bool,
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
    /// `(width, height)` ratio to hold while floating and being
    /// interactively resized (`ResizeEdge::apply_aspect_ratio`), `None` to
    /// resize freely. Set via a rule's `aspect_ratio` action (`"9:16"`) --
    /// the "phone monitor / special workspace" ask's own real, scoped
    /// answer: a VM/emulator/`scrcpy` window tagged this way keeps a
    /// phone-shaped frame through a drag, without srdwm needing to know
    /// anything about the specific app driving it (matches by `app_id`,
    /// the same rule mechanism `decorated`/`floating`/`pinned` already
    /// use - this is not Android-specific in any way). A real, if
    /// narrower, precedent for this already exists outside this project:
    /// ICCCM's `WM_NORMAL_HINTS` min/max aspect, which some X11 clients
    /// set themselves - this is the compositor-rule equivalent for
    /// clients (most Wayland ones, concretely) that don't.
    pub aspect_ratio: Option<(u32, u32)>,
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
    /// Set by `WindowManager::add_window` when `geometry`'s size just came
    /// from `SmartPlacement`'s own guessed default (`Window::new`'s
    /// `640x480`, or whatever a backend hardcodes before a client has said
    /// anything about its own preferred size) rather than a deliberate
    /// decision - a remembered size, a rule's explicit `geometry` action,
    /// or a maximize/phone-mode fill. `false` for all three of those, since
    /// there is nothing provisional about a size someone actually chose.
    ///
    /// A backend reads this once, right after `add_window` returns, to
    /// decide whether the *client's own* first real committed size should
    /// be allowed to win once it arrives (see `crates/wayland/src/state/
    /// geometry.rs`'s own use of this) - reported live as "windows always
    /// spawn small and square, not remembering placement or size": every
    /// new toplevel was forced, via its very first `xdg_toplevel.configure`,
    /// into this guessed placeholder size regardless of what the
    /// application itself would have preferred, which is why every app
    /// converged on the same generic footprint instead of its own natural
    /// one.
    pub size_is_provisional: bool,
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
            is_dialog: false,
            resizable: true,
            min_size: (crate::placement::MIN_WINDOW_WIDTH, crate::placement::MIN_WINDOW_HEIGHT),
            min_size_from_rule: false,
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
            aspect_ratio: None,
            // Always overwritten by `WindowManager::add_window` before this
            // is ever read for real (to the current workspace, or a rule's
            // own `workspace` action) - `1`, not `0`, only because
            // workspace ids are 1-based now (see `WindowManager::new`'s own
            // doc comment), so this placeholder still names a workspace
            // that could plausibly exist.
            workspace: 1,
            monitor: 0,
            rules_applied: false,
            anim_from: None,
            global_menu: None,
            size_is_provisional: false,
        }
    }
}

/// The height, in pixels, of the drawn title bar. Shared between backends so
/// hit-testing and rendering agree on the same band.
///
/// `32`, measured directly against a real, live Firefox window: a
/// side-by-side screenshot of both windows at identical scale, scanned
/// column-by-column for the pixel row where the titlebar's own background
/// colour gives way to the next row down (Firefox's tab strip), put that
/// boundary at row 32 sharp. An earlier value of `38` came from a Nemo
/// headerbar measured the same way (~40px) - Nemo's own headerbar carries
/// extra chrome (a search/menu button row) a bare titlebar doesn't, so it
/// isn't the right reference once Firefox is the thing actually being
/// matched. `ThemeConfig::default_corner_radius` moves with this (see its
/// own doc comment) to keep the same `radius / TITLEBAR_HEIGHT` ratio
/// rather than just looking proportionally smaller on top of already being
/// shorter.
pub const TITLEBAR_HEIGHT: u32 = 32;
/// The centre-to-centre spacing between titlebar buttons, and the size of
/// the square each one's own dot/click-box is drawn/hit-tested inside --
/// deliberately *not* the same value as `TITLEBAR_HEIGHT` (which used to
/// double as this too). Reported live: with the two tied together, growing
/// `TITLEBAR_HEIGHT` to `38` to match a real GTK headerbar's own *row*
/// height also silently grew the buttons themselves to a visibly bigger
/// scale than that same headerbar's own buttons - a real GTK/Firefox CSD
/// row reserves generous padding above and below a comparatively compact
/// button cluster, not one dimension sized off the other. `24`, matching a
/// real Firefox window's own measured button-to-button spacing (via
/// screenshot, at this system's own scale) - kept a separate constant
/// from `TITLEBAR_HEIGHT` specifically so the two can each move for their
/// own reason without dragging the other along. `decoration::button_box`
/// centres this smaller box vertically inside the taller `TITLEBAR_HEIGHT`
/// band for rendering; `ResizeEdge::hit_test` below has no matching
/// vertical narrowing to do - a click anywhere in the titlebar's own
/// height column-wise inside a button's `BUTTON_PITCH`-wide slice still
/// counts as that button, the same generous-vertical-target convention
/// every mainstream desktop already uses.
pub const BUTTON_PITCH: u32 = 24;
/// The gap, in pixels, between the titlebar's own edge (whichever side the
/// button cluster renders on) and the first button's own click/draw box --
/// measured directly against a live Firefox window: its visible dot's own
/// left edge sits 17px in from the window's real left edge, while the
/// button's own box margin (`decoration::BUTTON_MARGIN_LEFT`, applied to
/// every box the same way) only accounts for 4px of that. The remaining
/// `13` is this - a real macOS/GTK titlebar's own leading margin is
/// visibly bigger than the gap *between* buttons, not the same value
/// reused for both. Added once, before the first button's own `BUTTON_
/// PITCH`-spaced offset, on whichever edge `buttons_left` selects (`decoration
/// ::render_titlebar`'s `offset` calculation, and the matching `left`/
/// `right` base below in `hit_test` - the two have to move together, the
/// same "renders on one side, hit-tests on the other" trap every other
/// button-geometry constant here already has to avoid). Before this
/// existed, the dead strip between the titlebar's real edge and the first
/// visible dot silently counted as a hit on that button (`hit_test`'s
/// slice starts flush with the edge) - clicking blank titlebar background
/// right at the corner closed the window instead of dragging it.
pub const BUTTON_CLUSTER_MARGIN: u32 = 13;
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
/// Resize margin for an *undecorated* window, on every edge and corner --
/// narrower than [`RESIZE_MARGIN`] on purpose.
///
/// An undecorated (client-side-decorated) window has no srdwm-drawn band
/// anywhere for srdwm to treat as its own - every pixel right up to each
/// edge is the client's real content: Firefox's tab strip at the top, its
/// own window-control dots in a top corner, Nemo's tab-close X hard against
/// its right edge, a minimize button nowhere near any corner at all. The
/// full `RESIZE_MARGIN` (and, at a corner, `CORNER_MARGIN`'s further
/// multiple of it) was tuned for a window srdwm decorates itself, where none
/// of that applies - the whole titlebar band, buttons included, is checked
/// before `resize_edge_at` ever runs, so widening its own resize zone never
/// costs it a click. Applied to an undecorated window instead, that same
/// width competed with the client's own controls for the same pixels on
/// every edge, not just the top - first found as "can't drag-move Firefox
/// from its own top bar" (the original, narrower-top-only version of this
/// margin), then reported again, live, as real mouse clicks on Nemo's own
/// tab-close and minimize buttons - one hard against the right edge, the
/// other not even near a corner - landing "a distance" from the visible
/// button. Resize-from-every-edge still works (a deliberate trade-off - see
/// `undecorated_window_still_resizes_from_every_edge_including_top`'s own
/// comment - since an undecorated window is still a window), just from a
/// much narrower band on every side that a click meant for the client's own
/// content is very unlikely to land in by accident. No corner-widening for
/// an undecorated window at all: that widening exists purely to make a
/// diagonal drag easier to land on a window srdwm itself has no competing
/// content in, which is never true here.
pub const UNDECORATED_RESIZE_MARGIN: i32 = 3;

/// How far *outside* a window's own frame the resize grab zone reaches,
/// regardless of border width.
///
/// The inward margin has to stay narrow - see
/// [`UNDECORATED_RESIZE_MARGIN`] above for the clicks it was stealing from
/// Nemo's own buttons. Pixels outside the frame have no such problem:
/// there is no client content there to take a click from, so the band can
/// be as generous as it needs to be.
///
/// This used to be `border_width` alone, which meant a borderless window
/// had no outward band at all and its entire resize target was the narrow
/// inward margin - 3px on an undecorated window. Reported as resizing
/// working "from one direction" and feeling "very cheap": a 3px target is
/// missed far more often than it is hit, so it reads as an edge that
/// sometimes resizes rather than one that always does. Turning borders off
/// for the macOS look made it strictly worse, since the border had been
/// quietly providing the only outward reach.
///
/// macOS and GNOME both let a pointer grab slightly outside a window's
/// visible edge for the same reason.
pub const RESIZE_OUTSET: i32 = 5;
/// Top-edge resize margin for a *decorated* window's own titlebar band --
/// unlike [`UNDECORATED_RESIZE_MARGIN`], this has no client content to
/// avoid stealing a click from (the whole titlebar band is srdwm's own
/// drawn UI, not the client's), so it can just reuse [`RESIZE_MARGIN`]
/// outright rather than needing its own narrower value.
///
/// Reported live as a real gap, not a guess: a decorated window's titlebar
/// had *no* top-edge resize zone at all outside the two tiny diagonal
/// corners - every other pixel of the band, including the top row,
/// resolved to `Drag` unconditionally - while an undecorated window
/// (Firefox) could already be resized from its own top edge via
/// `UNDECORATED_RESIZE_MARGIN` above. "Can't resize tmux's window from
/// the top, but can in Firefox" was the exact live report. `hit_test`'s
/// own decorated-titlebar branch checks a button's x-range *before* this
/// margin, not after, so a button sitting within the first few rows of
/// the titlebar (true for every button, since `decoration::button_box`
/// spans nearly the full titlebar height) still always wins there --
/// this only ever applies to the button-free part of the band.
pub const DECORATED_TOP_RESIZE_MARGIN: i32 = RESIZE_MARGIN;
/// How much wider than [`RESIZE_MARGIN`] a corner's own diagonal-resize
/// zone reaches, as a multiplier on whatever margin is actually in effect
/// - see `ResizeEdge::resize_edge_at`'s doc comment for why corners need
/// more room than a straight edge at all, not just a proportionally bigger
/// dead-simple hit box.
///
/// Bumped from `3` (18px at the default `RESIZE_MARGIN`) to `5` (30px):
/// reported live as still too tight to land reliably, and there's real
/// room to widen it further than a plain edge ever could be - someone
/// reaching for a straight edge-resize instinctively aims for the middle
/// of that edge, not its corner, precisely to *avoid* accidentally
/// grabbing a corner instead. A bigger corner zone doesn't compete with
/// that instinct the way a bigger `RESIZE_MARGIN` would compete with
/// ordinary clicks near an edge (`RESIZE_MARGIN`'s own doc comment).
/// Applies to every corner `resize_edge_at` and the titlebar's own
/// non-button corner check use - not the button-side corner, which
/// stays deliberately narrower than this; see its own hit-test comment.
pub const CORNER_MARGIN: i32 = 5;

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
    #[allow(clippy::too_many_arguments)]
    pub fn hit_test(
        frame: Rect,
        x: i32,
        y: i32,
        decorated: bool,
        border_width: u32,
        resize_margin: i32,
        buttons_left: bool,
        order_override: Option<ButtonOrder>,
        // `Window::is_dialog`'s resolved value - see its own doc comment.
        // A dialog only ever shows/recognizes Close, never Minimize/
        // Maximize, regardless of `order_override`; must stay in exact
        // agreement with `decoration::render_titlebar`'s own `is_dialog`
        // parameter, the same "renders on one side, hit-tests on the
        // other" trap every other button-geometry value here already has
        // to avoid.
        is_dialog: bool,
        // Whether a Maximize button is shown at all - `theme.
        // dynamic_buttons && !window.resizable` resolves to `false`. Same
        // "must stay in exact agreement with `decoration::render_titlebar`"
        // contract as `is_dialog` directly above, and for the same reason:
        // the two sides compute button slots independently.
        show_maximize: bool,
    ) -> Option<TitlebarHit> {
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
        // Whichever reaches further: the drawn border, or the fixed outward
        // grab band. See `RESIZE_OUTSET`.
        let bw = (border_width as i32).max(RESIZE_OUTSET);
        let reach = bw as u32;
        let outer = Rect::new(frame.x - bw, frame.y - bw, frame.width + 2 * reach, frame.height + 2 * reach);
        if !outer.contains_point(x, y) {
            return None;
        }
        if decorated && y < frame.y + TITLEBAR_HEIGHT as i32 {
            // The titlebar's own outer corner (on whichever side doesn't
            // hold the buttons) is still the window's outer corner --
            // without this, a decorated window's diagonal resize there was
            // completely unreachable: every y inside the titlebar band
            // returned here unconditionally, before `resize_edge_at`
            // (checked below, for every other edge) ever ran. A genuine
            // small square right at the corner (both x *and* y within it),
            // not just "close on one axis" - otherwise this would claim
            // the whole drag area at any height within the titlebar, not
            // just its actual corner.
            //
            // The corner *with* the buttons deliberately does not get the
            // same treatment: every mainstream desktop's convention is
            // that the corner of a titlebar closes the window, not
            // resizes it - see `decoration::button_box`'s own doc
            // comment for the matching rendering-side placement this has
            // to agree with. Adding a competing resize zone there would
            // trade a real, expected target (close) for a rarely-wanted
            // one at exactly the spot a miss is most costly. `buttons_left`
            // flips which corner gets which treatment, not just where the
            // buttons render - the two have to move together.
            let corner_zone = CORNER_MARGIN * resize_margin;
            if !buttons_left && x <= frame.x + corner_zone && y <= frame.y + corner_zone {
                return Some(TitlebarHit::Resize(ResizeEdge::TopLeft));
            }
            if buttons_left && x >= frame.right() - corner_zone && y <= frame.y + corner_zone {
                return Some(TitlebarHit::Resize(ResizeEdge::TopRight));
            }
            // Box size is *not* bigger when left-aligned, even though the
            // visible dot is (see `decoration::BUTTON_MARGIN_LEFT`) - the
            // box is already capped at `BUTTON_PITCH` vertically by
            // `decoration::button_box`'s own centring, so a genuinely
            // bigger *box* would draw a dot that gets clipped top/bottom
            // against it. A bigger dot within the same click box gets the
            // requested "bigger" look with no such clipping risk, and
            // keeps this hit-test box in exact agreement with `decoration::
            // button_box`'s own size, not just its side. `BUTTON_PITCH`,
            // not `TITLEBAR_HEIGHT` - see that constant's own doc comment
            // for why the two aren't the same value.
            let button: i32 = BUTTON_PITCH as i32;
            // Closest-to-the-aligned-edge first - see `ButtonOrder`'s own
            // doc comment for why the two built-in defaults are genuinely
            // different relative orderings, not mirrors of each other,
            // and why an explicit override applies identically regardless
            // of side rather than trying to preserve that asymmetry.
            // A dialog always recognizes Close, full stop - not just
            // whichever button an `order_override` would otherwise put
            // first, or Minimize/Maximize could still end up the one hit-
            // testable button. Matches `decoration::render_titlebar`'s own
            // identical override for the same reason.
            let order: ButtonOrder = if is_dialog {
                [TitlebarButton::Close; 3]
            } else {
                order_override.unwrap_or(if buttons_left {
                    [TitlebarButton::Close, TitlebarButton::Minimize, TitlebarButton::Maximize]
                } else {
                    [TitlebarButton::Close, TitlebarButton::Maximize, TitlebarButton::Minimize]
                })
            };
            // Maximize dropped from the slot list entirely rather than
            // left in place and ignored: leaving a hole would put a dead
            // gap between the two remaining buttons, and the renderer
            // closes the gap, so hit-testing has to close it identically
            // or every button after it is offset by one slot.
            let order: Vec<TitlebarButton> = if show_maximize {
                order.to_vec()
            } else {
                order.iter().copied().filter(|b| *b != TitlebarButton::Maximize).collect()
            };
            // A dialog only ever recognizes the first slot, matching
            // `decoration::render_titlebar` only ever drawing the one
            // button there too.
            let button_count = if is_dialog { 1 } else { order.len() };
            if buttons_left {
                let left = frame.x + BUTTON_CLUSTER_MARGIN as i32;
                // `x >= left` excludes the dead `BUTTON_CLUSTER_MARGIN`
                // strip between the titlebar's real edge and the first
                // button - without this guard, `x < left + button` (the
                // first iteration below) is trivially true for any `x`
                // left of `left` too, so that whole blank strip silently
                // counted as a Close hit.
                if x >= left {
                    for (i, b) in order.iter().take(button_count).enumerate() {
                        if x < left + button * (i as i32 + 1) {
                            return Some(match b {
                                TitlebarButton::Close => TitlebarHit::Close,
                                TitlebarButton::Minimize => TitlebarHit::Minimize,
                                TitlebarButton::Maximize => TitlebarHit::Maximize,
                            });
                        }
                    }
                }
                // `x < left` is the same dead `BUTTON_CLUSTER_MARGIN` strip
                // excluded above, now put to use: it's already unclaimed by
                // any button's own hitbox, by construction, so its own top
                // `DECORATED_TOP_RESIZE_MARGIN` rows are a safe, if
                // deliberately narrow, diagonal-resize target right at the
                // button-side corner - narrower than the non-button
                // corner's own `corner_zone` above on purpose, since this
                // one has Close sitting immediately next to it and widening
                // it any further would start eating into that button's own
                // hitbox instead of just the dead space next to it.
                // Reported live: this corner had no resize target at all,
                // "even where decorations are corner i should still be able
                // to corner resize."
                if x < left && y <= frame.y + DECORATED_TOP_RESIZE_MARGIN {
                    return Some(TitlebarHit::Resize(ResizeEdge::TopLeft));
                }
                if y <= frame.y + DECORATED_TOP_RESIZE_MARGIN {
                    return Some(TitlebarHit::Resize(ResizeEdge::Top));
                }
                return Some(TitlebarHit::Drag);
            }
            let right = frame.right() - BUTTON_CLUSTER_MARGIN as i32;
            // Same guard, mirrored: `x <= right` excludes the dead strip
            // between the first (rightmost) button and the titlebar's real
            // right edge.
            if x <= right {
                for (i, b) in order.iter().take(button_count).enumerate() {
                    if x >= right - button * (i as i32 + 1) {
                        return Some(match b {
                            TitlebarButton::Close => TitlebarHit::Close,
                            TitlebarButton::Minimize => TitlebarHit::Minimize,
                            TitlebarButton::Maximize => TitlebarHit::Maximize,
                        });
                    }
                }
            }
            // Mirror of the `buttons_left` branch's own identical check
            // above - see its comment for the full reasoning.
            if x > right && y <= frame.y + DECORATED_TOP_RESIZE_MARGIN {
                return Some(TitlebarHit::Resize(ResizeEdge::TopRight));
            }
            if y <= frame.y + DECORATED_TOP_RESIZE_MARGIN {
                return Some(TitlebarHit::Resize(ResizeEdge::Top));
            }
            return Some(TitlebarHit::Drag);
        }
        let edge = Self::resize_edge_at(frame, x, y, decorated, resize_margin)?;
        Some(TitlebarHit::Resize(edge))
    }

    fn resize_edge_at(frame: Rect, x: i32, y: i32, decorated: bool, resize_margin: i32) -> Option<ResizeEdge> {
        // A corner resize gets a bigger, prioritized hit zone than a plain
        // single edge for a *decorated* window - a diagonal drag is a
        // harder target to land than a straight edge, and several desktops
        // (GNOME, KDE) give it noticeably more room for exactly that reason.
        // Reported live: corners felt like they had no priority over sides
        // at all, which tracked - a corner previously only registered in
        // the exact pixel square where both edges' own `resize_margin` zones
        // happened to overlap (6x6px at the default margin), nothing wider.
        //
        // None of that widening applies to an *undecorated* window, on any
        // edge or corner - see [`UNDECORATED_RESIZE_MARGIN`]'s own doc
        // comment for why a single small, uniform margin is what's actually
        // correct there.
        let (m, corner_m) = if decorated { (resize_margin, CORNER_MARGIN * resize_margin) } else { (UNDECORATED_RESIZE_MARGIN, UNDECORATED_RESIZE_MARGIN) };

        let corner_left = x <= frame.x + corner_m;
        let corner_right = x >= frame.right() - corner_m;
        let corner_top = y <= frame.y + corner_m;
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
        let near_top = y <= frame.y + m;
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

    /// Whether this edge has a left-hand component (`Left`, `TopLeft`,
    /// `BottomLeft`) - used by tiling's own master/stack ratio-drag
    /// detection (`WindowManager::tiling_ratio_drag`) to recognize a stack
    /// column window's left edge as the same shared boundary a master
    /// column window's own right edge is.
    pub fn has_left(self) -> bool {
        matches!(self, ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft)
    }

    /// [`Self::has_left`]'s mirror, for `Right`/`TopRight`/`BottomRight`.
    pub fn has_right(self) -> bool {
        matches!(self, ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight)
    }

    /// Re-derives one dimension of `delta_applied` (the result of
    /// `apply_delta`, already reflecting this drag's pointer motion) so
    /// the rect holds `ratio` (`width, height`) - `Window::aspect_ratio`'s
    /// own doc comment explains why this exists at all.
    ///
    /// A pure vertical edge (`Top`/`Bottom`) derives *width* from the new
    /// height: that is the one dimension the user is actually dragging on
    /// that edge, so deriving it back from a width that never changed
    /// would leave the edge under the cursor not tracking the cursor.
    /// Every other edge (a horizontal edge or a corner) derives *height*
    /// from width instead, for the mirrored reason - `Left`/`Right` only
    /// ever change width in `apply_delta` to begin with, and a corner's
    /// own diagonal drag has no single "the" dimension, so width (the
    /// axis every non-vertical-only edge here actually touches) is the
    /// one reasonable, consistent choice.
    ///
    /// `TopLeft`/`TopRight` additionally re-anchor `y` the same way
    /// `apply_delta` itself anchors height for those two edges (keep the
    /// *bottom* edge fixed) - otherwise a locked-ratio window dragged
    /// from its top would grow downward instead of upward, the one
    /// direction that edge is actually supposed to move.
    pub fn apply_aspect_ratio(self, delta_applied: Rect, ratio: (u32, u32), min_w: u32, min_h: u32) -> Rect {
        if ratio.0 == 0 || ratio.1 == 0 {
            return delta_applied;
        }
        let mut r = delta_applied;
        if matches!(self, ResizeEdge::Top | ResizeEdge::Bottom) {
            let new_w = ((r.height as u64 * ratio.0 as u64) / ratio.1 as u64).max(min_w as u64) as u32;
            r.width = new_w;
        } else {
            let new_h = ((r.width as u64 * ratio.1 as u64) / ratio.0 as u64).max(min_h as u64) as u32;
            if matches!(self, ResizeEdge::TopLeft | ResizeEdge::TopRight) {
                r.y = delta_applied.bottom() - new_h as i32;
            }
            r.height = new_h;
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

/// One of the three titlebar buttons, for [`ButtonOrder`] - a narrower
/// type than [`TitlebarHit`] on purpose: `TitlebarHit` also carries
/// `Drag`/`Resize`, neither of which is a button a layout can place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarButton {
    Close,
    Minimize,
    Maximize,
}

/// A custom `close,minimize,maximize`-style ordering for the three
/// titlebar buttons, overriding this project's own default order for
/// whichever side `buttons_left` already selects - KWin's `ButtonsOnLeft`/
/// `ButtonsOnRight`, GNOME/Adwaita's `decoration-layout`, and Openbox's
/// `titlelayout` each independently converged on exactly this "ordered
/// list of button names" idea, confirmed this session by reading each
/// project's own real config docs/source rather than assuming.
///
/// Positions are read closest-to-the-aligned-edge first, same as this
/// project's own two built-in defaults already are: on the left, index 0
/// sits at the window's own left edge; on the right, index 0 sits at the
/// window's own right edge. This project's two defaults (macOS-style
/// close-minimize-maximize on the left, Windows/GTK-style close-maximize-
/// minimize on the right) are genuinely different *relative* orderings,
/// not mirrors of each other - seem `hit_test`'s own doc comment on why
/// that's deliberate - so `None` (the default, no override configured)
/// keeps using whichever of those two already applies rather than this
/// type imposing one order on both sides.
pub type ButtonOrder = [TitlebarButton; 3];

/// Parses a `srd.set("theme.decorations.button_order", "...")` value like
/// `"close,minimize,maximize"` into a [`ButtonOrder`] - `None` if it
/// doesn't name each of the three buttons exactly once (a typo'd or
/// partial list falls back to this project's own built-in default rather
/// than silently hiding a button or drawing one twice).
pub fn parse_button_order(s: &str) -> Option<ButtonOrder> {
    let mut close = None;
    let mut minimize = None;
    let mut maximize = None;
    for (i, part) in s.split(',').map(str::trim).enumerate() {
        match part.to_ascii_lowercase().as_str() {
            "close" => close = Some(i),
            "minimize" | "minimise" => minimize = Some(i),
            "maximize" | "maximise" => maximize = Some(i),
            _ => return None,
        }
    }
    let (Some(c), Some(mn), Some(mx)) = (close, minimize, maximize) else { return None };
    let mut order = [TitlebarButton::Close; 3];
    for (slot, button) in [(c, TitlebarButton::Close), (mn, TitlebarButton::Minimize), (mx, TitlebarButton::Maximize)] {
        if slot >= 3 {
            return None;
        }
        order[slot] = button;
    }
    // Every slot must have been assigned exactly once - three distinct
    // source indices (0, 1, 2) covering three slots guarantees that; a
    // repeated button name (e.g. "close,close,maximize") would have
    // reused one slot index and left another unset, which range 0..3
    // alone can't catch.
    let mut seen = [false; 3];
    for i in [c, mn, mx] {
        if seen[i] {
            return None;
        }
        seen[i] = true;
    }
    Some(order)
}

/// [`parse_button_order`]'s exact inverse - `"close,minimize,maximize"`,
/// lowercase, comma-separated in the order the buttons actually render.
/// Exists for settings readback, same reasoning as `format_hex_color`:
/// a caller reading `button_order` back over IPC should get the identical
/// string shape `srd set button_order` itself accepts.
pub fn format_button_order(order: ButtonOrder) -> String {
    order
        .iter()
        .map(|b| match b {
            TitlebarButton::Close => "close",
            TitlebarButton::Minimize => "minimize",
            TitlebarButton::Maximize => "maximize",
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod button_order_tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_order() {
        assert_eq!(parse_button_order("close,minimize,maximize"), Some([TitlebarButton::Close, TitlebarButton::Minimize, TitlebarButton::Maximize]));
        assert_eq!(parse_button_order("maximize,minimize,close"), Some([TitlebarButton::Maximize, TitlebarButton::Minimize, TitlebarButton::Close]));
    }

    #[test]
    fn is_case_insensitive_and_trims_whitespace() {
        assert_eq!(parse_button_order(" Close, MINIMIZE ,Maximize"), Some([TitlebarButton::Close, TitlebarButton::Minimize, TitlebarButton::Maximize]));
    }

    #[test]
    fn accepts_the_british_spelling() {
        assert_eq!(parse_button_order("close,minimise,maximise"), Some([TitlebarButton::Close, TitlebarButton::Minimize, TitlebarButton::Maximize]));
    }

    #[test]
    fn rejects_a_missing_button() {
        assert_eq!(parse_button_order("close,minimize"), None);
    }

    #[test]
    fn rejects_a_duplicated_button() {
        assert_eq!(parse_button_order("close,close,maximize"), None);
    }

    #[test]
    fn format_button_order_round_trips_through_parse_button_order() {
        for order in [
            [TitlebarButton::Close, TitlebarButton::Minimize, TitlebarButton::Maximize],
            [TitlebarButton::Maximize, TitlebarButton::Minimize, TitlebarButton::Close],
        ] {
            assert_eq!(parse_button_order(&format_button_order(order)), Some(order));
        }
    }

    #[test]
    fn format_button_order_matches_the_exact_shape_set_accepts() {
        assert_eq!(format_button_order([TitlebarButton::Close, TitlebarButton::Minimize, TitlebarButton::Maximize]), "close,minimize,maximize");
    }

    #[test]
    fn rejects_an_unknown_token() {
        assert_eq!(parse_button_order("close,minimize,help"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn likely_draws_own_titlebar_matches_both_known_namespaces_case_insensitively() {
        assert!(likely_draws_own_titlebar("org.gnome.Nautilus"));
        assert!(likely_draws_own_titlebar("ORG.GNOME.TextEditor"));
        assert!(likely_draws_own_titlebar("org.pwmt.zathura"));
        assert!(likely_draws_own_titlebar("Org.Pwmt.Zathura"));
    }

    #[test]
    fn likely_draws_own_titlebar_does_not_misclassify_unrelated_reverse_dns_ids() {
        assert!(!likely_draws_own_titlebar("io.github.somebody.SomeApp"));
        assert!(!likely_draws_own_titlebar("org.mozilla.firefox"));
        assert!(!likely_draws_own_titlebar("firefox"));
        assert!(!likely_draws_own_titlebar(""));
    }

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
        // `BUTTON_CLUSTER_MARGIN` back from the edge, not just `- 5`: that
        // margin is a real dead strip now (see its own doc comment) - a
        // point only `5` in from the raw edge landed inside it, not on the
        // button.
        let hit = ResizeEdge::hit_test(f, f.right() - BUTTON_CLUSTER_MARGIN as i32 - 5, f.y + 5, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Close));
    }

    #[test]
    fn maximize_is_left_of_close() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - BUTTON_CLUSTER_MARGIN as i32 - BUTTON_PITCH as i32 - 5, f.y + 5, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Maximize));
    }

    #[test]
    fn a_dialog_only_ever_recognizes_close_not_minimize_or_maximize() {
        // The whole point of `is_dialog`: the same point that hits Maximize
        // for a normal window (see `maximize_is_left_of_close` just above)
        // must not hit anything at all for a dialog - that button was
        // never drawn there in the first place (`decoration::
        // render_titlebar`'s own `is_dialog` branch), so a phantom hit zone
        // there would be exactly the "click does nothing, or worse, hits
        // the wrong control" bug this codebase already fixed once for
        // undecorated windows.
        let f = frame();
        let maximize_spot = f.right() - BUTTON_CLUSTER_MARGIN as i32 - BUTTON_PITCH as i32 - 5;
        let hit = ResizeEdge::hit_test(f, maximize_spot, f.y + 5, true, 0, RESIZE_MARGIN, false, None, true, true);
        assert_ne!(hit, Some(TitlebarHit::Maximize), "a dialog must not have a Maximize hit zone at all");
        assert_ne!(hit, Some(TitlebarHit::Minimize), "a dialog must not have a Minimize hit zone at all");
        // The one real button (Close) must still be exactly where it always
        // is - `is_dialog` removes the other two, not shifts this one.
        let close_hit = ResizeEdge::hit_test(f, f.right() - BUTTON_CLUSTER_MARGIN as i32 - 5, f.y + 5, true, 0, RESIZE_MARGIN, false, None, true, true);
        assert_eq!(close_hit, Some(TitlebarHit::Close));
    }

    #[test]
    fn a_dialog_recognizes_close_even_with_a_button_order_override_that_does_not_start_with_it() {
        // An explicit `button_order` still must not be able to put
        // Minimize/Maximize where a dialog's one real button (Close) is --
        // see `hit_test`'s own doc comment on why `is_dialog` ignores
        // `order_override` outright rather than just capping how many of
        // it get used.
        let f = frame();
        let order = [TitlebarButton::Maximize, TitlebarButton::Minimize, TitlebarButton::Close];
        let hit = ResizeEdge::hit_test(f, f.right() - BUTTON_CLUSTER_MARGIN as i32 - 5, f.y + 5, true, 0, RESIZE_MARGIN, false, Some(order), true, true);
        assert_eq!(hit, Some(TitlebarHit::Close));
    }

    #[test]
    fn an_explicit_button_order_moves_the_hit_zones_to_match() {
        // Same point `maximize_is_left_of_close` above hits as Maximize
        // under the built-in default - an override putting minimize
        // there instead must change what a click there actually does,
        // not just what gets drawn.
        let f = frame();
        let order = [TitlebarButton::Close, TitlebarButton::Minimize, TitlebarButton::Maximize];
        let hit = ResizeEdge::hit_test(f, f.right() - BUTTON_CLUSTER_MARGIN as i32 - BUTTON_PITCH as i32 - 5, f.y + 5, true, 0, RESIZE_MARGIN, false, Some(order), false, true);
        assert_eq!(hit, Some(TitlebarHit::Minimize));
    }

    #[test]
    fn a_button_order_override_applies_the_same_way_on_either_side() {
        // The whole point of an explicit override: unlike the two built-
        // in defaults (genuinely different relative orderings per side,
        // see `ButtonOrder`'s own doc comment), a caller-specified order
        // reads closest-to-edge-first the same way whichever side it's
        // on.
        let f = frame();
        let order = [TitlebarButton::Maximize, TitlebarButton::Minimize, TitlebarButton::Close];
        let left_hit = ResizeEdge::hit_test(f, f.x + BUTTON_CLUSTER_MARGIN as i32 + 5, f.y + 5, true, 0, RESIZE_MARGIN, true, Some(order), false, true);
        let right_hit = ResizeEdge::hit_test(f, f.right() - BUTTON_CLUSTER_MARGIN as i32 - 5, f.y + 5, true, 0, RESIZE_MARGIN, false, Some(order), false, true);
        assert_eq!(left_hit, Some(TitlebarHit::Maximize));
        assert_eq!(right_hit, Some(TitlebarHit::Maximize));
    }

    #[test]
    fn middle_of_titlebar_is_drag() {
        // Past `DECORATED_TOP_RESIZE_MARGIN`, not just `f.y + 5` (this
        // test's original y) - once the titlebar's own thin top edge
        // gained a resize zone, a point that shallow no longer tests
        // "plain drag area" at all. See `decorated_window_very_top_edge_
        // of_titlebar_resizes_not_drags` for that zone's own coverage.
        let f = frame();
        let (cx, _) = f.center();
        let hit = ResizeEdge::hit_test(f, cx, f.y + DECORATED_TOP_RESIZE_MARGIN + 5, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Drag));
    }

    #[test]
    fn bottom_right_corner_is_resize() {
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 1, f.bottom() - 1, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::BottomRight)));
    }

    #[test]
    fn a_non_resizable_window_has_no_maximize_button_to_click() {
        // `show_maximize = false` - the dynamic button mode's one rule.
        let f = Rect::new(0, 0, 400, 300);
        // Buttons on the right, default order Close, Maximize, Minimize.
        // Slot 0 is Close either way.
        let slot = |i: i32| f.right() - BUTTON_CLUSTER_MARGIN as i32 - BUTTON_PITCH as i32 * i - 1;
        let hit = |x: i32, show_max: bool| ResizeEdge::hit_test(f, x, f.y + TITLEBAR_HEIGHT as i32 / 2, true, 0, RESIZE_MARGIN, false, None, false, show_max);

        assert_eq!(hit(slot(1), true), Some(TitlebarHit::Maximize), "resizable: slot 1 is Maximize");
        // With Maximize dropped, Minimize moves up into slot 1 - it must
        // not leave a dead gap there.
        assert_eq!(hit(slot(1), false), Some(TitlebarHit::Minimize), "non-resizable: Minimize closes the gap");
        assert_eq!(hit(slot(0), false), Some(TitlebarHit::Close), "Close stays put");
    }

    #[test]
    fn a_non_resizable_window_still_has_exactly_two_buttons() {
        let f = Rect::new(0, 0, 400, 300);
        let slot = |i: i32| f.right() - BUTTON_CLUSTER_MARGIN as i32 - BUTTON_PITCH as i32 * i - 1;
        let hit = |x: i32| ResizeEdge::hit_test(f, x, f.y + TITLEBAR_HEIGHT as i32 / 2, true, 0, RESIZE_MARGIN, false, None, false, false);
        // Slot 2 held Minimize when there were three; with two it is past
        // the cluster and must be a drag, not a phantom third button.
        assert_eq!(hit(slot(2)), Some(TitlebarHit::Drag), "no third button exists any more");
    }

    #[test]
    fn a_dialog_is_close_only_regardless_of_the_dynamic_button_mode() {
        let f = Rect::new(0, 0, 400, 300);
        let slot = |i: i32| f.right() - BUTTON_CLUSTER_MARGIN as i32 - BUTTON_PITCH as i32 * i - 1;
        for show_max in [true, false] {
            let hit = |x: i32| ResizeEdge::hit_test(f, x, f.y + TITLEBAR_HEIGHT as i32 / 2, true, 0, RESIZE_MARGIN, false, None, true, show_max);
            assert_eq!(hit(slot(0)), Some(TitlebarHit::Close), "show_maximize={show_max}");
            assert_eq!(hit(slot(1)), Some(TitlebarHit::Drag), "a dialog has no second button, show_maximize={show_max}");
        }
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
        let hit = ResizeEdge::hit_test(f, cx, f.y + 20, false, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, None);
    }

    #[test]
    fn undecorated_window_still_resizes_from_every_edge_including_top() {
        let f = frame();
        let (cx, _) = f.center();
        let hit = ResizeEdge::hit_test(f, cx, f.y + 1, false, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::Top)));
    }

    #[test]
    fn undecorated_top_resize_band_is_much_narrower_than_decorated() {
        // Regression test: an undecorated window's own header (Firefox's
        // tab strip, concretely) has no srdwm-drawn titlebar to grab, so a
        // click meant to drag-move it via the client's own `xdg_toplevel.
        // move` has to actually reach the client - the full `RESIZE_MARGIN`
        // (10px) swallowed most of a natural grab point near the top as a
        // resize instead. A *decorated* window's own top band gained a
        // matching (if wider) resize margin of its own since this test was
        // first written - see `DECORATED_TOP_RESIZE_MARGIN`'s own doc
        // comment - so this now checks *past* both margins, where the two
        // must still agree (undecorated: reaches the client; decorated:
        // plain drag), rather than claiming the decorated band has no top
        // resize zone at all, which is no longer true.
        let f = frame();
        let (cx, _) = f.center();
        assert_eq!(ResizeEdge::hit_test(f, cx, f.y + 5, false, 0, RESIZE_MARGIN, false, None, false, true), None, "5px in: past the narrow undecorated band, must reach the client");
        assert_eq!(
            ResizeEdge::hit_test(f, cx, f.y + DECORATED_TOP_RESIZE_MARGIN + 5, true, 0, RESIZE_MARGIN, false, None, false, true),
            Some(TitlebarHit::Drag),
            "decorated: past its own (wider) top resize margin, still plain drag"
        );
    }

    #[test]
    fn undecorated_corner_resize_is_not_widened_either() {
        // Same regression as the top-margin test above, but for a corner --
        // a real live report was a click on Nemo's own tab-close button,
        // hard against the window's right edge, near the top, landing on a
        // phantom resize instead of the client. This point is well past
        // `UNDECORATED_RESIZE_MARGIN` (3px) on both axes but was still
        // within the old, decorated-window-sized corner zone
        // (`CORNER_MARGIN * RESIZE_MARGIN` = 18px) before this fix.
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 10, f.y + 10, false, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, None, "past the narrow undecorated corner margin on both axes, must reach the client");
    }

    #[test]
    fn undecorated_bottom_right_corner_also_uses_the_narrow_margin() {
        // The top corners aren't the only ones a CSD client can draw real
        // content near - nothing about `CORNER_MARGIN`'s widening should
        // survive for an undecorated window at any corner.
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 10, f.bottom() - 10, false, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, None, "past the narrow undecorated corner margin on both axes, must reach the client");
    }

    #[test]
    fn undecorated_window_resizes_from_right_and_bottom_edges_within_the_narrow_margin() {
        // Confirms the narrow margin is still a real, working resize zone on
        // every edge, not just the top - this fix must not trade "buttons
        // near an edge are clickable" for "can't resize from that edge at
        // all".
        let f = frame();
        let (_, cy) = f.center();
        let (cx, _) = f.center();
        assert_eq!(ResizeEdge::hit_test(f, f.right() - 1, cy, false, 0, RESIZE_MARGIN, false, None, false, true), Some(TitlebarHit::Resize(ResizeEdge::Right)));
        assert_eq!(ResizeEdge::hit_test(f, cx, f.bottom() - 1, false, 0, RESIZE_MARGIN, false, None, false, true), Some(TitlebarHit::Resize(ResizeEdge::Bottom)));
    }

    #[test]
    fn outside_frame_is_none() {
        let f = frame();
        assert_eq!(ResizeEdge::hit_test(f, 0, 0, true, 0, RESIZE_MARGIN, false, None, false, true), None);
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
        // A border wider than the fixed outward band, so this test is
        // actually exercising the border and not `RESIZE_OUTSET`.
        let border_width = (RESIZE_OUTSET + 3) as u32;
        let x = f.x - 1;
        assert_eq!(
            ResizeEdge::hit_test(f, x, cy, true, border_width, RESIZE_MARGIN, false, None, false, true),
            Some(TitlebarHit::Resize(ResizeEdge::Left)),
            "one pixel into the actual drawn border must register as the left edge"
        );
        assert_eq!(
            ResizeEdge::hit_test(f, f.x - border_width as i32 + 1, cy, true, border_width, RESIZE_MARGIN, false, None, false, true),
            Some(TitlebarHit::Resize(ResizeEdge::Left)),
            "the far side of a wide border is still the window's edge"
        );
        // Past the border entirely: the grab zone has a boundary, it is
        // just further out than the frame.
        assert_eq!(ResizeEdge::hit_test(f, f.x - border_width as i32 - 1, cy, true, border_width, RESIZE_MARGIN, false, None, false, true), None);
    }

    /// A borderless window still has somewhere to grab. Before
    /// `RESIZE_OUTSET` the outward band was `border_width` alone, so
    /// turning borders off left only the inward margin - 3px on an
    /// undecorated window, which is missed more often than hit.
    #[test]
    fn a_borderless_window_can_still_be_grabbed_from_outside_its_edge() {
        let f = frame();
        let (_, cy) = f.center();
        for dx in 1..=RESIZE_OUTSET {
            assert_eq!(
                ResizeEdge::hit_test(f, f.x - dx, cy, false, 0, RESIZE_MARGIN, false, None, false, true),
                Some(TitlebarHit::Resize(ResizeEdge::Left)),
                "{dx}px outside a borderless window's left edge must still resize"
            );
        }
        assert_eq!(
            ResizeEdge::hit_test(f, f.x - RESIZE_OUTSET - 1, cy, false, 0, RESIZE_MARGIN, false, None, false, true),
            None,
            "one pixel past the band is not the window"
        );
    }

    #[test]
    fn the_outward_band_reaches_every_edge_not_just_the_left() {
        let f = frame();
        let (cx, cy) = f.center();
        let probe = |x, y| ResizeEdge::hit_test(f, x, y, false, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(probe(f.right() + RESIZE_OUTSET - 1, cy), Some(TitlebarHit::Resize(ResizeEdge::Right)));
        assert_eq!(probe(cx, f.y - RESIZE_OUTSET + 1), Some(TitlebarHit::Resize(ResizeEdge::Top)));
        assert_eq!(probe(cx, f.bottom() + RESIZE_OUTSET - 1), Some(TitlebarHit::Resize(ResizeEdge::Bottom)));
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
        assert_eq!(ResizeEdge::hit_test(f, x, y, true, 0, RESIZE_MARGIN, false, None, false, true), Some(TitlebarHit::Resize(ResizeEdge::BottomLeft)));
    }

    #[test]
    fn just_past_the_corner_zone_falls_back_to_a_single_edge() {
        let f = frame();
        let corner_reach = CORNER_MARGIN * RESIZE_MARGIN;
        // Still within the corner zone vertically but past it horizontally
        // - must read as a plain bottom edge, not a corner.
        let x = f.x + corner_reach + 5;
        let y = f.bottom() - 1;
        assert_eq!(ResizeEdge::hit_test(f, x, y, true, 0, RESIZE_MARGIN, false, None, false, true), Some(TitlebarHit::Resize(ResizeEdge::Bottom)));
    }

    #[test]
    fn decorated_window_top_left_corner_of_titlebar_resizes_not_drags() {
        // The titlebar band used to claim every y within it unconditionally
        // (drag, or a button near the right edge) - a decorated window's
        // top-left diagonal resize was completely unreachable, since
        // `resize_edge_at` never even ran for a y inside the titlebar.
        let f = frame();
        let corner_reach = CORNER_MARGIN * RESIZE_MARGIN;
        let hit = ResizeEdge::hit_test(f, f.x + corner_reach - 1, f.y + corner_reach - 1, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::TopLeft)));
    }

    #[test]
    fn decorated_window_top_right_corner_still_closes_not_resizes() {
        // Deliberately the opposite of the top-left case: the close button
        // owns the top-right corner, matching every mainstream desktop's
        // convention, rather than competing with a resize zone at exactly
        // the spot a miss is most costly.
        let f = frame();
        // Just inside `BUTTON_CLUSTER_MARGIN`, not the raw corner pixel --
        // the raw corner itself now sits in that real dead strip (see its
        // own doc comment), which correctly falls through to drag/resize,
        // not Close.
        let hit = ResizeEdge::hit_test(f, f.right() - BUTTON_CLUSTER_MARGIN as i32 - 1, f.y + 1, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Close));
    }

    #[test]
    fn button_side_corner_still_resizes_from_its_own_dead_strip() {
        // Reported live: "even where decorations are corner i should still
        // be able to corner resize, just its... hitbox... does not get in
        // the way of the close icon." The raw corner pixel sits in the
        // `BUTTON_CLUSTER_MARGIN` dead strip outside Close's own hitbox
        // (see `decorated_window_top_right_corner_still_closes_not_resizes`
        // just above), which used to fall through to plain `Top`/`Drag`
        // with no diagonal target at all. Both axes right at the true
        // corner - well within the dead strip horizontally (`- 1` from the
        // frame's own edge, nowhere near where Close's hitbox starts) and
        // within `DECORATED_TOP_RESIZE_MARGIN` vertically.
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 1, f.y + 1, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::TopRight)));
    }

    #[test]
    fn button_side_corner_below_its_own_resize_row_still_drags() {
        // The dead strip's own top rows are the new corner-resize target
        // (`button_side_corner_still_resizes_from_its_own_dead_strip`), but
        // the rest of that same narrow column, below
        // `DECORATED_TOP_RESIZE_MARGIN`, is still ordinary titlebar drag --
        // this only ever claims the true corner, not the whole column
        // beside Close.
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 1, f.y + DECORATED_TOP_RESIZE_MARGIN + 5, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Drag));
    }

    #[test]
    fn button_side_corner_still_resizes_from_its_own_dead_strip_when_buttons_are_left() {
        // Mirror of `button_side_corner_still_resizes_from_its_own_dead_
        // strip` with `buttons_left: true` - Close now owns the top-left
        // corner, so the dead strip and its own corner-resize target move
        // to the frame's left edge instead.
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.x + 1, f.y + 1, true, 0, RESIZE_MARGIN, true, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::TopLeft)));
    }

    #[test]
    fn decorated_window_very_top_edge_of_titlebar_resizes_not_drags() {
        // The actual regression: reported live as "can't resize tmux's
        // window from the top, but can in Firefox" - a decorated window's
        // titlebar band claimed *every* button-free pixel as `Drag`
        // unconditionally, with no top-edge resize zone at all outside the
        // two tiny diagonal corners, unlike an undecorated window's own
        // (narrower) top-edge margin. `x` is the titlebar's horizontal
        // middle, clear of both the corner zones and either side's button
        // boxes, so this is testing the plain top edge specifically.
        let f = frame();
        let x = f.x + f.width as i32 / 2;
        let hit = ResizeEdge::hit_test(f, x, f.y + DECORATED_TOP_RESIZE_MARGIN - 1, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Resize(ResizeEdge::Top)));
    }

    #[test]
    fn decorated_window_titlebar_below_the_top_margin_still_drags() {
        // Sanity check for the fix above: only the thin top margin itself
        // gained a resize zone - the rest of the titlebar (where most
        // real drags actually start) must still read as `Drag`, not have
        // silently grown a resize zone everywhere.
        let f = frame();
        let x = f.x + f.width as i32 / 2;
        let hit = ResizeEdge::hit_test(f, x, f.y + DECORATED_TOP_RESIZE_MARGIN + 5, true, 0, RESIZE_MARGIN, false, None, false, true);
        assert_eq!(hit, Some(TitlebarHit::Drag));
    }

    #[test]
    fn decorated_window_top_edge_over_a_button_still_hits_the_button() {
        // The other half of the fix: the new top-margin resize zone must
        // not swallow clicks meant for a button just because that button
        // also happens to sit within the first few rows of the titlebar --
        // exactly the "buttons... not swallowing" risk this was written to
        // avoid. Right-aligned close button's box starts at `right - 30`;
        // well inside it, at the very top row.
        let f = frame();
        let hit = ResizeEdge::hit_test(f, f.right() - 15, f.y + 1, true, 0, RESIZE_MARGIN, false, None, false, true);
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

    #[test]
    fn aspect_ratio_derives_height_from_width_on_a_horizontal_edge() {
        let r = Rect::new(0, 0, 900, 300);
        let out = ResizeEdge::Right.apply_aspect_ratio(r, (9, 16), 1, 1);
        assert_eq!(out, Rect::new(0, 0, 900, 1600));
    }

    #[test]
    fn aspect_ratio_derives_width_from_height_on_a_pure_vertical_edge() {
        // Bottom only ever changes height in `apply_delta` - deriving
        // width back from a height that never changed would leave the
        // edge under the cursor not tracking the cursor, the actual bug
        // this split exists to avoid.
        let r = Rect::new(0, 0, 300, 1600);
        let out = ResizeEdge::Bottom.apply_aspect_ratio(r, (9, 16), 1, 1);
        assert_eq!(out, Rect::new(0, 0, 900, 1600));
    }

    #[test]
    fn aspect_ratio_on_top_left_keeps_the_bottom_right_corner_fixed() {
        // TopLeft's own `apply_delta` anchor is the bottom-right corner
        // (dragging up-left grows the window while its bottom-right stays
        // put); the aspect-ratio pass must keep that same corner fixed
        // when it re-derives height, or a locked-ratio window dragged
        // from its top would visibly grow the wrong way.
        //
        // Simulates a diagonal drag already processed by `apply_delta`:
        // dragged left by 100 (width 400 -> 500, x 0 -> -100) and up by
        // 300 (height 900 -> 1200, y 0 -> -300).
        let delta_applied = Rect::new(-100, -300, 500, 1200);
        let out = ResizeEdge::TopLeft.apply_aspect_ratio(delta_applied, (9, 16), 1, 1);
        // height is derived from the (unchanged-by-this-pass) width: 500 * 16 / 9 = 888 (floor).
        assert_eq!(out.height, 888);
        // The bottom-right corner - not `y` itself - is what must be
        // preserved, and matches the *original* rect's own bottom (900)
        // too, since `apply_delta`'s own TopLeft anchor already keeps
        // bottom fixed at 900 before this pass ever runs.
        assert_eq!(out.y + out.height as i32, delta_applied.bottom());
        assert_eq!(delta_applied.bottom(), 900);
        assert_eq!(out.bottom(), 900);
    }

    #[test]
    fn aspect_ratio_never_shrinks_below_the_given_minimum() {
        let r = Rect::new(0, 0, 10, 10);
        let out = ResizeEdge::Right.apply_aspect_ratio(r, (9, 16), 50, 50);
        assert!(out.height >= 50);
    }

    #[test]
    fn a_zero_component_ratio_is_a_no_op_not_a_division_by_zero() {
        let r = Rect::new(0, 0, 300, 900);
        assert_eq!(ResizeEdge::Right.apply_aspect_ratio(r, (0, 16), 1, 1), r);
        assert_eq!(ResizeEdge::Right.apply_aspect_ratio(r, (9, 0), 1, 1), r);
    }
}
