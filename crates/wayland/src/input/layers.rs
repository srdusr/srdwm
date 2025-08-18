//! `zwlr_layer_shell_v1` pointer hit-testing (bars, docks, launchers) and
//! the layer-driven maximize-geometry computation both backends' `monitors()`
//! need.

use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::{Anchor, ExclusiveZone, Layer, LayerSurfaceCachedState};

use crate::state::CompState;

/// Topmost layer-shell surface (if any) under `pos`, checked in the same
/// above-everything-else stacking order `space_render_elements` renders
/// `Overlay`/`Top` layers in (bars, launchers, notifications, lock UIs).
/// `Background`/`Bottom` layers (wallpapers) deliberately aren't checked
/// here: nothing in scope for the daily-driver gate needs pointer input
/// routed to them, and space windows should stay clickable over a
/// wallpaper.
/// `pos` is in the global space; layer geometry is relative to its own
/// output, so the pointer is translated into output-local coordinates
/// before hit-testing and the result translated back out.
/// Only checked for `Overlay`/`Top` before a window hit-test, and again for
/// `Bottom`/`Background` after one comes up empty - see the two call
/// sites in `handle_pointer_button`/`handle_pointer_position` for why it's
/// split rather than one four-layer loop here. A `Bottom`/`Background`
/// surface (a desktop-icons layer, a wallpaper daemon that wants clicks) is
/// meant to sit *behind* normal windows, so a window covering that point
/// should still get the click; `Overlay`/`Top` (an on-screen keyboard, a
/// bar, a dock) are meant to sit in front of everything, windows included.
///
/// Was `Overlay`/`Top` only, full stop - a `Bottom`-layer surface was
/// silently unclickable no matter what, since nothing else in
/// `handle_pointer_button` ever checked layers at all. Not the cause of
/// the live "clicking the dock does nothing" report (confirmed: that dock
/// uses `Layer::Top`, which was already checked), but a real, separate gap
/// found while chasing it - worth closing regardless of whether anything
/// currently deployed sits at `Bottom`/`Background` yet.
pub(super) fn layer_surface_under_layers(state: &CompState, pos: Point<f64, Logical>, layers: [Layer; 2]) -> Option<(WlSurface, Point<i32, Logical>)> {
    let entry = state.output_at(pos)?;
    let origin = entry.location;
    // `pos`/`origin` are physical (this compositor's own convention
    // throughout, including `OutputEntry::location`'s own `Logical`-typed-
    // but-physical-valued field - see its own doc comment), but `LayerMap
    // ::layer_geometry` below is not: confirmed against smithay 0.7.0's
    // own source (`desktop/wayland/layer.rs::arrange`), it divides the
    // output's physical mode by its own scale before arranging layers, so
    // it - and `LayerSurface::surface_under`'s own coordinate space,
    // which reads surface-local points in that same system - is genuinely
    // logical. Comparing a physical point against that without converting
    // silently clips off however much a non-1.0 scale shrinks the surface
    // by: confirmed live (with a peer session's own independent
    // measurement) on a 0.843-scale output, a bottom-anchored dock sits
    // entirely past the physical pointer's own reachable range - always
    // unclickable, not just at the edges - while a top-anchored bar on
    // the same output only loses its own right-hand end, which is what
    // made this look like "the dock is broken" rather than a scale bug
    // affecting every layer surface on that output.
    let scale = entry.output.current_scale().fractional_scale();
    let local_physical = pos - origin.to_f64();
    let local: Point<f64, Logical> = (local_physical.x / scale, local_physical.y / scale).into();
    let map = layer_map_for_output(&entry.output);
    for layer_kind in layers {
        // Not `map.layer_under(layer_kind, local)` - that hands back only
        // the single topmost surface whose *bounding box* contains `local`,
        // and if that one surface's own input region excludes the point
        // (its `surface_under` below returns `None`), the old code gave up
        // on this whole layer-kind rather than trying whatever real,
        // clickable surface is stacked underneath it. A bbox-only pick is
        // exactly wrong the moment two surfaces on the same layer-kind
        // overlap - a transparent, mapped-but-mostly-empty surface (a
        // backdrop-dismiss popup, concretely: `Overview`'s own bbox-wide
        // fallback region was exactly this shape before it was fixed
        // AGS-side) sitting in front of a real one in z-order would
        // silently swallow every click and even every hover/motion event
        // meant for the surface underneath, with no way to reach it at
        // all. Walking every candidate on this layer-kind, topmost first
        // (`.rev()`, matching `layer_under`'s own z-order convention), and
        // falling through to the next when a candidate's real input region
        // doesn't cover the point, is what `layer_under` alone can't do.
        for layer in map.layers_on(layer_kind).rev() {
            let Some(geo) = map.layer_geometry(layer) else { continue };
            if !geo.to_f64().contains(local) {
                continue;
            }
            // The `layer_surfaces_shown_once` fix (state/layers.rs) stops a
            // reused `wl_surface`'s stale layer-shell entry from outliving
            // its role destroy - previously live-reproduced as a full-
            // monitor click-catcher popup whose hit-tested geometry came
            // back wider than the real output after several open/close
            // cycles. A per-motion-event diagnostic log verifying this
            // used to sit here; removed after it was found to be a real,
            // significant cost on the hot pointer-motion path (querying
            // compositor cached state and formatting/writing a log line
            // on every single pixel of every mouse move), reported live as
            // general input sluggishness, not just log noise.
            if let Some((surface, surface_loc)) = layer.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL) {
                // `geo.loc + surface_loc` is still logical (same space as
                // `local` above) - scaled back to physical here so the
                // returned point matches every caller's own expected space
                // (`pos`'s own convention, and `origin`'s, already
                // physical).
                let logical = geo.loc + surface_loc;
                let physical: Point<i32, Logical> = ((logical.x as f64 * scale).round() as i32, (logical.y as f64 * scale).round() as i32).into();
                return Some((surface, origin + physical));
            }
        }
    }
    None
}

pub(super) fn layer_surface_under(state: &CompState, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
    layer_surface_under_layers(state, pos, [Layer::Overlay, Layer::Top])
}

/// The `Bottom`/`Background` half of the same lookup - see
/// `layer_surface_under_layers`'s doc comment for the ordering rationale.
pub(super) fn background_layer_surface_under(state: &CompState, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<i32, Logical>)> {
    layer_surface_under_layers(state, pos, [Layer::Bottom, Layer::Background])
}

/// `full` with only a top-anchored layer surface's exclusive zone (a menu
/// bar) subtracted back out - see `Monitor::maximize_geometry`'s own doc
/// comment for why maximize needs this third rect, distinct from both
/// `geometry` (every zone subtracted) and `full_geometry` (none). Shared by
/// both backends' `monitors()`, same as everything else in this module.
/// Deliberately re-derived from the layer list rather than reusing
/// `non_exclusive_zone()`: that smithay helper folds every anchor
/// together with no way to ask it to skip one edge - see below for which
/// edges this now shrinks for and why.
///
/// Shrinks for a reservation on *any* edge (top, bottom, left, or right),
/// not top only - reported live as a maximized window's own bottom edge
/// and border ending up underneath a bottom-anchored dock, indistinguishable
/// from the dock not rendering at all. An earlier version of this
/// function shrank only for a top-anchored bar, on the reasoning that
/// maximize should be able to "go past" a dock while fullscreen (which
/// already ignores every zone, via `full_geometry`) covers the case that
/// wants the screen entirely to itself - but no other edge actually
/// benefits from that distinction the way a top menu bar does, and
/// respecting every edge here is what every mainstream desktop's own
/// maximize convention already does. Fullscreen is unaffected - it never
/// called this function, and still doesn't.
pub(crate) fn maximize_geometry_for(output: &Output, full: srdwm_core::Rect) -> srdwm_core::Rect {
    let mut rect = full;
    // `exclusive_zone`/`margin` are logical (a layer-shell client reports
    // its own reservation the same way every other layer-shell geometry
    // is expressed), while `full` is physical pixels - same unit
    // mismatch `Platform::monitors()` needed fixing for, and the same
    // fix: scale the logical amount into physical pixels before touching
    // a physical rect with it. Left unconverted, a scaled output's
    // maximize target shrank by the wrong number of physical rows/columns
    // for its own bar/dock (too few at scale < 1.0, too many above 1.0).
    let scale = output.current_scale().fractional_scale();
    for layer in layer_map_for_output(output).layers() {
        let data = with_states(layer.wl_surface(), |states| *states.cached_state.get::<LayerSurfaceCachedState>().current());
        let ExclusiveZone::Exclusive(amount) = data.exclusive_zone else { continue };
        let scaled = |margin: i32| ((amount as f64 + margin as f64) * scale).round().max(0.0) as i32;
        if data.anchor.contains(Anchor::TOP) && !data.anchor.contains(Anchor::BOTTOM) {
            let shrink = scaled(data.margin.top);
            rect.y += shrink;
            rect.height = rect.height.saturating_sub(shrink as u32);
        }
        if data.anchor.contains(Anchor::BOTTOM) && !data.anchor.contains(Anchor::TOP) {
            let shrink = scaled(data.margin.bottom);
            rect.height = rect.height.saturating_sub(shrink as u32);
        }
        if data.anchor.contains(Anchor::LEFT) && !data.anchor.contains(Anchor::RIGHT) {
            let shrink = scaled(data.margin.left);
            rect.x += shrink;
            rect.width = rect.width.saturating_sub(shrink as u32);
        }
        if data.anchor.contains(Anchor::RIGHT) && !data.anchor.contains(Anchor::LEFT) {
            let shrink = scaled(data.margin.right);
            rect.width = rect.width.saturating_sub(shrink as u32);
        }
    }
    rect
}
