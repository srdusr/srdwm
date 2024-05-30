//! The one render-element type for everything srdwm draws *itself*, on top
//! of client windows: its titlebars, borders, and the mouse pointer.
//!
//! `render_output` takes a single `custom_elements` slice, so these have to
//! be one type. They come from three different sources - an uploaded
//! bitmap (titlebars, the built-in cursor arrow), a client's own surface (a
//! client-set cursor image), and a native solid-colour fill (borders) --
//! hence the three variants.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{Color32F, ImportAll, ImportMem, Renderer};
use smithay::desktop::{layer_map_for_output, PopupManager, Space, Window as DWindow};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Point, Rectangle, Scale};

use srdwm_core::TITLEBAR_HEIGHT;

use crate::state::CompState;

smithay::backend::renderer::element::render_elements! {
    pub(crate) OverlayElement<R> where
        R: ImportAll + ImportMem;
    /// A client's own surface, used for client-set cursor images.
    Surface=WaylandSurfaceRenderElement<R>,
    /// A bitmap srdwm rasterised: a titlebar, or the built-in cursor arrow.
    Memory=MemoryRenderBufferRenderElement<R>,
    /// A native solid-colour fill - window borders. Deliberately not a
    /// stretched 1x1 `Memory` buffer (`border_strips`' previous approach):
    /// `PixmanRenderer` (the udev backend's software renderer) hardcodes
    /// `Repeat::None` on every imported texture, so sampling a 1x1 texture
    /// stretched across a larger destination has no valid neighbouring
    /// texels to fall back on and renders fully transparent - the border
    /// silently never appeared on real hardware, only on the winit/GLES
    /// backend (GPU texture sampling of a 1x1 texture returns that texel
    /// regardless of wrap mode, so it happened to work there by accident).
    /// `SolidColorRenderElement` goes through `Frame::draw_solid`, a native
    /// fill with no texture import or sampling involved at all, so this
    /// backend difference cannot affect it.
    Solid=SolidColorRenderElement,
}

/// One border strip as a native solid-colour fill, `origin`-translated to
/// output-local space - the same convention every other `custom_elements`
/// entry (cursor, decorations, popups) already uses. No renderer/texture
/// import needed at all (unlike the `Memory`-buffer approach this replaced).
///
/// Takes a *persistent* `SolidColorBuffer` (one kept per window per strip in
/// `CompState::border_side_buffers`, updated in place here) rather than
/// building a element with a fresh `Id`/`CommitCounter` every call, which an
/// earlier version of this function did on the reasoning that `Id`'s only
/// role is picking a *tighter* damage rect and a border strip is cheap to
/// redraw in full regardless. That undersold the actual cost: smithay's
/// `OutputDamageTracker::damage_output_internal` looks up each element's
/// previous state by `Id` and falls back to `.unwrap_or(true)` ("damage it")
/// when no match is found - a fresh `Id` every frame means *every* frame
/// finds no match, so every bordered window's border is marked damaged on
/// *every* frame forever, not occasionally with a wider rect. Confirmed by
/// reading `damage/mod.rs`'s `damage_output_internal` directly (0.7.0):
/// `element_last_state.map(|s| !s.instance_matches(...)).unwrap_or(true)`.
/// Downstream, `render_udev_frame`'s `has_damage` gate around the real DRM
/// page flip is then unconditionally true every ~16ms for as long as any
/// window with `border_width > 0` is on screen - effectively every window,
/// since that is the default - so the output never actually goes idle.
/// `SolidColorBuffer::update` only bumps its internal `CommitCounter` when
/// the size or colour actually changed, so reusing the same buffer (and
/// therefore the same stable `Id`) here is what lets the tracker correctly
/// see "nothing changed" and skip real work on a static screen.
pub(crate) fn border_side_render_element(buf: &mut SolidColorBuffer, strip: srdwm_core::Rect, color: (u8, u8, u8), origin: (i32, i32)) -> SolidColorRenderElement {
    let c = Color32F::new(color.0 as f32 / 255.0, color.1 as f32 / 255.0, color.2 as f32 / 255.0, 1.0);
    buf.update((strip.width as i32, strip.height as i32), c);
    let loc = Point::from((strip.x - origin.0, strip.y - origin.1));
    SolidColorRenderElement::from_buffer(buf, loc, 1.0, 1.0, Kind::Unspecified)
}

/// Splits a border strip into the sub-rectangles still visible after
/// subtracting every window rect stacked in front of it.
///
/// Border strips render via `custom_elements`, which `smithay::desktop::
/// space::render_output` always composites above *every* window's own
/// content (drawn separately, via `self.space`) - unconditionally, with
/// no way to interleave the two by real stacking order (confirmed by
/// reading `render_output`'s own source: `custom_elements` are pushed into
/// the final element list before `space_render_elements`, and this
/// backend's damage tracker treats earlier-pushed elements as topmost, the
/// same convention `cursor::render_elements`' own doc comment already
/// relies on). Without this, a background window's border rendered as a
/// solid line straight through a foreground window's content wherever the
/// two visually overlapped - confirmed live: a stack of cascaded
/// terminals showed every earlier window's right border strip as a set of
/// vertical lines cutting across the frontmost window's own content.
/// `occluders` should already be limited to windows stacked above the one
/// this border belongs to.
pub(crate) fn visible_border_fragments(strip: srdwm_core::Rect, occluders: &[srdwm_core::Rect]) -> Vec<srdwm_core::Rect> {
    strip.subtract_all(occluders)
}

/// Gets (growing if needed) the `index`th persistent buffer in a window's
/// border-fragment pool - see `CompState::border_side_buffers`' doc
/// comment for why this is a growable pool rather than a fixed-size array.
pub(crate) fn border_fragment_buffer(pool: &mut Vec<SolidColorBuffer>, index: usize) -> &mut SolidColorBuffer {
    if index >= pool.len() {
        pool.resize_with(index + 1, SolidColorBuffer::default);
    }
    &mut pool[index]
}

/// A mapped toplevel's surface and on-screen (global-space, band-adjusted)
/// position - everything [`popup_render_elements`] needs to find and place
/// that window's popups, pre-extracted from `CompState` by
/// [`popup_targets`] so the actual rendering call can run after
/// `self.udev`/`self.backend` is mutably borrowed for its renderer, the
/// same "gather immutable state first" split `render_udev_frame` already
/// uses for borders and decorations (see its own comment).
pub(crate) struct PopupTarget {
    surface: WlSurface,
    window_pos: (i32, i32),
}

/// Only windows `visible_windows()` would show are considered - a popup
/// belonging to a workspace-hidden or minimized parent has no business
/// appearing on screen either.
///
/// Also gathers every mapped layer-shell surface (bars, launchers) across
/// every output, not just toplevel windows - a popup can be parented to
/// either (`zwlr_layer_surface_v1.get_popup`, used for a bar's own
/// dropdown/context menu, is a completely separate request from
/// `xdg_surface.get_popup`, and this function previously only ever
/// accounted for the latter). `PopupManager` itself already tracked and
/// configured layer-surface-parented popups correctly (that path goes
/// through the same `XdgShellHandler::new_popup`/`commit()` regardless of
/// what the eventual parent turns out to be) - they just never had a
/// `PopupTarget` to render relative to, so they were fully functional and
/// completely invisible: a bar's dropdown menu would open, accept clicks,
/// and never draw a single pixel.
pub(crate) fn popup_targets(state: &CompState) -> Vec<PopupTarget> {
    let wm = state.wm.borrow();
    let current = wm.current_workspace();
    let windows = state.id_to_window.iter().filter_map(|(&id, dwindow)| {
        let w = wm.window(id)?;
        if w.minimized || w.workspace != current {
            return None;
        }
        let toplevel = dwindow.toplevel()?;
        let band = if w.decorated { TITLEBAR_HEIGHT as i32 } else { 0 };
        Some(PopupTarget { surface: toplevel.wl_surface().clone(), window_pos: (w.geometry.x, w.geometry.y + band) })
    });
    let layers = state.outputs.iter().flat_map(|entry| {
        let origin = entry.location;
        let map = layer_map_for_output(&entry.output);
        map.layers()
            .filter_map(|layer| {
                let geo = map.layer_geometry(layer)?;
                Some(PopupTarget { surface: layer.wl_surface().clone(), window_pos: (origin.x + geo.loc.x, origin.y + geo.loc.y) })
            })
            .collect::<Vec<_>>()
    });
    windows.chain(layers).collect()
}

/// Every currently-mapped `xdg_popup`'s surface tree, positioned relative to
/// its parent toplevel - tooltips, dropdown menus, right-click menus. Not
/// part of `space` (popups are never `space.map_element`'d, only their
/// parent toplevel is), so `render_output`'s automatic per-space-element
/// rendering never sees them; this is what makes them show up at all now
/// that `protocols.rs`'s `new_popup` actually configures them instead of
/// hanging the client forever.
///
/// `origin` is the output's location in global space, same as every other
/// `custom_elements` entry (cursor, borders, decorations) already
/// subtracts - positions here have to be output-local too, or a popup on
/// any output but the one at the global origin renders offset by that
/// output's own placement.
pub(crate) fn popup_render_elements<R>(targets: &[PopupTarget], renderer: &mut R, origin: (i32, i32)) -> Vec<OverlayElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let mut elements = Vec::new();
    for target in targets {
        for (popup, offset) in PopupManager::popups_for_surface(&target.surface) {
            // Both `Xdg` (tooltips, dropdown/context menus) and
            // `InputMethod` (an IME's composition/candidate window --
            // registered via `protocols.rs`'s `InputMethodHandler::
            // new_popup`) render identically here: `PopupManager` already
            // tracks both uniformly keyed by parent surface, and `offset`
            // is each kind's own on-screen position relative to
            // `target.window_pos` regardless of which one it is.
            let loc = (target.window_pos.0 - origin.0 + offset.x, target.window_pos.1 - origin.1 + offset.y);
            elements.extend(render_elements_from_surface_tree(renderer, popup.wl_surface(), loc, 1.0, 1.0, Kind::Unspecified));
        }
    }
    elements
}

/// Which of `space`'s mapped windows actually need a frame callback this
/// pass - the ones whose on-screen bounds overlap `damage`, in physical
/// output-space.
///
/// Both backends used to call `send_frame` on *every* mapped window
/// whenever the output had any damage at all, including damage from
/// nothing but the cursor moving. `has-damage` is an output-wide gate; it
/// says nothing about *where* the damage was, so a window nowhere near the
/// cursor was told just as urgently to redraw as one it was passing over.
/// Any client using the standard wait-for-frame-callback pattern (most of
/// them, confirmed live: wezterm-gui pinned at 140%+ CPU on a fully idle
/// terminal) had no reason not to redraw every single time, forever. This
/// narrows the callback to windows the damage rectangles actually
/// intersect, which is what makes a stationary cursor's damage stop
/// waking up windows it never touched.
///
/// Deliberately does *not* also cover the focused/hovered-window starvation
/// case (an idle window's own pending callback never getting answered
/// because nothing else ever damages its bounds) - an earlier version of
/// this function took an `always_notify` list for that, but folding it in
/// here meant it only ever ran on a tick that already had damage from
/// something else, i.e. never on the fully-idle tick it was meant for. See
/// the caller: that case is now handled by a second, always-unconditional
/// pass over the focused/hovered windows specifically, independent of
/// whether this function's damage-gated pass runs at all this tick.
pub(crate) fn windows_touched_by_damage<'a>(
    space: &'a Space<DWindow>,
    damage: &'a [Rectangle<i32, Physical>],
    scale: Scale<f64>,
) -> impl Iterator<Item = &'a DWindow> + 'a {
    space.elements().filter(move |w| {
        space
            .element_geometry(w)
            .map(|geo| {
                let phys = geo.to_physical_precise_round(scale);
                damage.iter().any(|d| d.overlaps(phys))
            })
            .unwrap_or(false)
    })
}
