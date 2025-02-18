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
use smithay::desktop::{layer_map_for_output, utils::under_from_surface_tree, PopupManager, Space, Window as DWindow, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

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

/// A mapped window's own `WlSurface`, regardless of which protocol backs
/// it - a native Wayland `xdg_toplevel` or an XWayland client. `None` for
/// an X11 window between `MapRequest` and its first commit, which really
/// has no surface yet.
pub(crate) fn window_wl_surface(w: &DWindow) -> Option<WlSurface> {
    if let Some(top) = w.toplevel() {
        return Some(top.wl_surface().clone());
    }
    w.x11_surface().and_then(|x| x.wl_surface())
}

/// Renders one window's (or one layer-shell surface's) own content --
/// nothing else, not decoration, not popups - at `location` (output-local
/// physical space, the same convention every other `custom_elements` entry
/// already uses) and `alpha`.
///
/// Deliberately not `Window::render_elements` (`AsRenderElements`): that
/// bundles a window's own popups into the same call, which would double
/// them with [`popup_render_elements`]'s own separate pass over
/// [`popup_targets`]. `render_elements_from_surface_tree` walks only the
/// given surface's own (sub)surface tree - the same low-level call this
/// file's popup rendering and `cursor.rs`'s client-cursor-image path
/// already use safely, so reusing it here for a window's *main* content is
/// the same proven primitive, not a new one.
///
/// This is what lets content have its own `alpha`, at all: content used to
/// only ever render via `self.space` passed whole to `render_output`,
/// which takes one `alpha` for the entire frame's worth of space content,
/// not one per window - there was no way to make one window translucent
/// without also dimming everything else in `self.space`.
pub(crate) fn surface_content_elements<R>(renderer: &mut R, surface: &WlSurface, location: (i32, i32), alpha: f32) -> Vec<OverlayElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    render_elements_from_surface_tree(renderer, surface, location, 1.0, alpha, Kind::Unspecified)
}

/// Looks up (rebuilding first if stale) the Pixman-backend rounded-corner
/// masked copy of `surface`'s content - see `rounded_corners_pixman`'s
/// module doc comment for what this actually does and why it needs a cache
/// at all. `epoch` is the window's current `CompState::content_epoch`
/// value (bumped once per real commit, in `commit()`); `loc`/`size` are
/// `rounded_corners_pixman::masked_content_buffer`'s own tree-render
/// origin and off-screen buffer dimensions (the caller's already-computed
/// negated `content_offset` and content rect) - the cached entry is
/// rebuilt whenever any of `epoch`/`radius`/`loc`/`size` no longer match
/// what it was last built from, so a window that isn't currently
/// repainting, resizing, or having its shadow-margin geometry renegotiated
/// costs nothing here beyond one `HashMap` lookup per frame.
///
/// Free function taking the fields it needs directly, rather than a
/// `CompState` method, so it can be called from inside `udev/render.rs`'s
/// loop alongside the already-live `self.udev.as_mut()` borrow - see that
/// call site.
///
/// `None` either because the off-screen render itself failed (a genuine
/// renderer error - there is no longer any "this window isn't shaped
/// right for masking" restriction, see `masked_content_buffer`'s own doc
/// comment) or because it hasn't been attempted yet this call; either way
/// the caller's fallback is the same: render `surface`'s content unrounded
/// via [`surface_content_elements`].
/// Cache entry for [`rounded_content_buffer`]: `(content_epoch, radius_bits,
/// loc, size, masked_buffer)` - keyed by [`srdwm_core::WindowId`], one entry
/// per window. The four values ahead of the buffer are exactly what that
/// function's own staleness check compares against on every call; see its
/// doc comment for why each one has to be part of the key.
pub(crate) type RoundedContentCache = std::collections::HashMap<srdwm_core::WindowId, (u64, u32, (i32, i32), (i32, i32), smithay::backend::renderer::element::memory::MemoryRenderBuffer)>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn rounded_content_buffer<'a>(
    cache: &'a mut RoundedContentCache,
    renderer: &mut smithay::backend::renderer::pixman::PixmanRenderer,
    epoch: u64,
    id: srdwm_core::WindowId,
    surface: &WlSurface,
    loc: (i32, i32),
    size: (i32, i32),
    radius: f32,
    corners: crate::rounded_corners::RoundedCorners,
) -> Option<&'a smithay::backend::renderer::element::memory::MemoryRenderBuffer> {
    let radius_bits = radius.to_bits();
    let stale = cache.get(&id).map(|(built, r, l, s, _)| *built != epoch || *r != radius_bits || *l != loc || *s != size).unwrap_or(true);
    if stale {
        match crate::rounded_corners_pixman::masked_content_buffer(renderer, surface, loc, size, radius, corners) {
            Some(data) => {
                let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                    &data,
                    smithay::backend::allocator::Fourcc::Argb8888,
                    size,
                    1,
                    smithay::utils::Transform::Normal,
                    None,
                );
                cache.insert(id, (epoch, radius_bits, loc, size, buffer));
            }
            None => {
                cache.remove(&id);
            }
        }
    }
    cache.get(&id).map(|(_, _, _, _, b)| b)
}

/// Every mapped layer-shell surface on `output` whose [`Layer`] `include`
/// accepts, each rendered via [`surface_content_elements`] at full opacity
/// - layer-shell surfaces (bars, docks, wallpaper engines) don't have a
/// per-surface opacity concept the way `srd.rule`'s `opacity` gives
/// windows.
///
/// Order matches smithay's own `space_render_elements` (0.7.0): `.rev()`
/// on `map.layers()` before rendering, so surfaces sharing one `Layer`
/// keep the same relative stacking smithay's convenience wrapper gave
/// them, now that this function replaces it.
///
/// Takes no `origin`/global-position parameter, unlike every *other*
/// per-head element builder in `udev/render.rs` - deliberately: those all
/// convert a window's `geometry` (stored in *global*, whole-desktop space)
/// into this one head's local framebuffer space by subtracting the head's
/// own `origin`. `LayerMap::layer_geometry` is different - confirmed
/// against smithay 0.7.0's own source (`desktop/wayland/layer.rs`): its
/// `zone`/layer positions are built from the output's own mode size alone,
/// with no global offset baked in at all, so it is *already* head-local.
/// An earlier version of this function added `origin` to it anyway (to
/// "match" the other element builders' own pattern without checking
/// whether the input was actually the same kind of value) - harmless for
/// a single-output setup or this output's own primary/first head, where
/// `origin` is always `(0, 0)`, but on a second head at a real nonzero
/// `origin` (e.g. `(1920, 0)`) it shifted every layer-shell surface - a
/// wallpaper, a bar - clean off the right edge of that head's own
/// 1920px-wide local framebuffer, never drawn at all despite the surface
/// being genuinely mapped, configured, and holding real committed pixel
/// data the entire time. Reported live as a real second monitor showing
/// nothing but its own clear colour, confirmed root-caused by adding a
/// temporary diagnostic (`LAYER-ELEMENTS-DIAG`, since removed) that logged
/// `layer_count`/`has_buffer` per output - both outputs showed identical,
/// fully-populated state the whole time, which is what pointed at a
/// positioning bug downstream of element-gathering rather than anything
/// about the surfaces or the render/flip pipeline itself (the pipeline
/// itself was separately confirmed alive on this exact head by moving the
/// real cursor there and seeing it render correctly, a different code path
/// with no `layer_geometry` involved at all).
pub(crate) fn output_layer_elements<R>(renderer: &mut R, output: &Output, include: impl Fn(Layer) -> bool) -> Vec<OverlayElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let map = layer_map_for_output(output);
    let mut elements = Vec::new();
    for layer in map.layers().rev() {
        if !include(layer.layer()) {
            continue;
        }
        let Some(geo) = map.layer_geometry(layer) else { continue };
        elements.extend(surface_content_elements(renderer, layer.wl_surface(), (geo.loc.x, geo.loc.y), 1.0));
    }
    elements
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
        // Same `xdg_surface::set_window_geometry` offset every other
        // position computation in this codebase subtracts (see
        // `state/geometry.rs::sync_geometry`'s doc comment for the full
        // explanation) - missed here originally. A popup's positioner
        // places it relative to the parent's *window geometry* (its real
        // visible content, per the protocol's own text), not the parent's
        // raw, unshifted buffer origin, so leaving this out put every CSD
        // window's dropdowns/right-click menus at a content_offset-sized
        // remove from both where they were drawn *and* where clicks were
        // tested for them - self-consistently wrong, so a menu still drew
        // and could still be clicked, just visibly detached from the
        // window whose click opened it, and increasingly so the deeper a
        // submenu nested (each level re-adds the same offset).
        let content_offset = dwindow.geometry().loc;
        let window_pos = (w.geometry.x - content_offset.x, w.geometry.y + band - content_offset.y);
        Some(PopupTarget { surface: toplevel.wl_surface().clone(), window_pos })
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

/// Topmost currently-mapped popup (tooltip, dropdown, right-click menu)
/// under `pos`, if any - checked before every other kind of surface, the
/// same "draw on top of literally everything else" priority
/// `popup_render_elements` above already gives every popup (pushed into
/// `custom_elements` ahead of even `Overlay`/`Top` layer-shell surfaces in
/// both backends' render loops).
///
/// Without this, pointer hit-testing (`input::refresh_pointer_focus`) never
/// checked popups at all: `state.popups: PopupManager` is entirely separate
/// from `state.space` (a popup is never `space.map_element`'d - see this
/// module's own `popup_render_elements` doc comment) and from layer-shell's
/// `LayerMap`, so hit-testing that only walked those two structures was
/// blind to every open popup. `xdg_popup`'s implicit pointer grab
/// (`PopupPointerGrab`, smithay's own - see `protocols.rs`'s `grab`
/// handler) only checks that the *focus* handed to it by `pointer.motion()`
/// belongs to the same client as the grabbed popup; it never substitutes in
/// the popup's own surface itself. A focus computed with no popup check at
/// all resolved to whatever was visually *underneath* the popup instead --
/// typically the same client's own parent window, so the grab's same-client
/// check passed and let the motion straight through - meaning every click
/// or scroll over an open popup actually landed on the parent surface, at
/// coordinates that correspond to nothing real drawn there. Reads exactly
/// as "menus are hard to click or scroll in", even though neither the grab
/// itself nor the popup's own widgets were ever broken.
///
/// Real per-pixel hit-testing via `under_from_surface_tree`, not a
/// bounding-rect check against `PopupKind::geometry()` - respects each
/// surface's actual input region and walks subsurfaces, the same
/// precision `layer_surface_under`/`Window::surface_under` already give
/// every other surface kind.
pub(crate) fn popup_surface_under(state: &CompState, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
    for target in popup_targets(state).iter().rev() {
        let popups: Vec<_> = PopupManager::popups_for_surface(&target.surface).collect();
        for (popup, offset) in popups.into_iter().rev() {
            let origin = Point::<i32, Logical>::from(target.window_pos) + offset;
            if let Some(hit) = under_from_surface_tree(popup.wl_surface(), pos, origin, WindowSurfaceType::ALL) {
                return Some(hit);
            }
        }
    }
    None
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
///
/// `origin` is the rendering head's own position in the shared global
/// space - needed because `damage` comes straight from that head's own
/// `OutputDamageTracker`, which (like every other per-head render element
/// in `udev/render.rs`) operates entirely in that head's own *local*
/// framebuffer space, starting at `(0, 0)` regardless of where the head
/// actually sits in the multi-monitor desktop. `space.element_geometry`,
/// by contrast, is always in *global* space (`Space` tracks every window
/// across the whole desktop, not per-output). Comparing the two directly
/// - what this function used to do - only ever produced a real overlap
/// for a head whose own `origin` happened to be `(0, 0)`, i.e. the first/
/// primary monitor in a left-to-right layout; every window on any other
/// monitor could never be found "touched" by that monitor's own damage at
/// all, no matter how much of it was actually changing on screen. A
/// client relying on this path alone - a video window the user had
/// switched focus *away* from, since the separate always-unconditional
/// pass above only covers the focused/hovered window - never received
/// another frame callback once srdwm's own bootstrap-configure frame
/// callback was used up, and simply stopped rendering new frames forever:
/// reported live as a paused-looking video on a second monitor, audio
/// still playing underneath (a completely separate pipeline, unaffected).
/// Same root cause and same fix shape as `output_layer_elements`'s own
/// local/global mismatch, found earlier the same session.
pub(crate) fn windows_touched_by_damage<'a>(
    space: &'a Space<DWindow>,
    damage: &'a [Rectangle<i32, Physical>],
    origin: Point<i32, Logical>,
    scale: Scale<f64>,
) -> impl Iterator<Item = &'a DWindow> + 'a {
    let origin_phys = origin.to_physical_precise_round(scale);
    space.elements().filter(move |w| {
        space
            .element_geometry(w)
            .map(|geo| {
                let phys = geo.to_physical_precise_round(scale);
                let local = Rectangle::new(phys.loc - origin_phys, phys.size);
                damage.iter().any(|d| d.overlaps(local))
            })
            .unwrap_or(false)
    })
}
