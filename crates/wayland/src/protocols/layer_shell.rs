//! `zwlr_layer_shell_v1`: panels, bars, launchers, and other output-anchored
//! shell surfaces (AGS's own bar and popups, notably).

use smithay::desktop::{layer_map_for_output, LayerSurface as DesktopLayerSurface};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::shell::wlr_layer::{Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState};

use crate::state::CompState;

impl WlrLayerShellHandler for CompState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, wl_output: Option<WlOutput>, _layer: Layer, namespace: String) {
        // Logged before anything else can early-return or panic: the
        // question this answers (see docs/PANEL_SUPPORT_TODO.md) is
        // whether this handler is reached AT ALL for a later
        // `get_layer_surface` request in a create -> commit -> destroy ->
        // commit-again -> create sequence, or whether the client's
        // dispatch is already dead by then and this never runs.
        log::debug!("layer-shell: new_layer_surface entered, surface={:?} namespace={namespace:?} output_named={}", surface.wl_surface().id(), wl_output.is_some());
        // A client may name the output it wants (a bar on a specific
        // monitor); if it doesn't, or names one we don't drive, it lands on
        // the primary output.
        let output = wl_output
            .as_ref()
            .and_then(|wl| self.output_for_wl(wl))
            .map(|e| e.output.clone())
            .or_else(|| self.primary_output().cloned());
        let Some(output) = output else {
            log::warn!("wayland: layer surface requested but no output exists yet");
            return;
        };
        // Paired with the debug log in `ensure_layer_initial_configure`'s
        // early return - see docs/PANEL_SUPPORT_TODO.md's P0. This is the
        // other half of "did map_layer actually succeed, and on which
        // output": logged unconditionally (not just on the error paths
        // that already existed) so a real reproduction shows both sides of
        // the handoff instead of just the failure.
        let surface_id = surface.wl_surface().id();
        let layer_surface = DesktopLayerSurface::new(surface, namespace);
        let result = layer_map_for_output(&output).map_layer(&layer_surface);
        match &result {
            Ok(()) => log::debug!("layer-shell: mapped surface {surface_id:?} onto output {}", output.name()),
            Err(e) => log::warn!("wayland: failed to map layer surface {surface_id:?}: {e}"),
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        // See the matching top-of-function log in `new_layer_surface`.
        log::debug!("layer-shell: layer_destroyed entered, surface={:?}", surface.wl_surface().id());
        // Marks this surface for the pre-commit-hook workaround in
        // `new_surface` - see that function's doc comment for the bug
        // this exists to route around.
        self.dead_layer_surfaces.insert(surface.wl_surface().clone());
        // GTK (confirmed live via an AGS peer session's WAYLAND_DEBUG trace)
        // reuses the same `wl_surface` for the next `get_layer_surface` role
        // rather than creating a fresh one - so without this, a "shown at
        // least once" flag from *this* role would leak onto the next one
        // and make `sync_layer_visibility` treat that new role's own
        // ack-configure commit as eligible to hide again, the same bug
        // `layer_surfaces_shown_once` exists to prevent, just reintroduced
        // for exactly the reused-surface case that matters here.
        self.layer_surfaces_shown_once.remove(surface.wl_surface());
        // The surface belongs to exactly one output's map, but which one is
        // the client's choice, so unmap from whichever holds it.
        for output in self.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(layer) = found {
                // Same zone-change recompute `ensure_layer_initial_configure`
                // already does on every commit that changes a layer's
                // exclusive zone (state/layers.rs) - but this is the *only* place
                // that ever runs for a surface that goes away without one
                // last commit. `unmap_layer` alone doesn't trigger it:
                // reported live (by the AGS peer session) as a bar unmapping
                // for fullscreen yet `srd monitors` still reporting the
                // bar's old reserved_top for as long as fullscreen lasted --
                // harmless there only because fullscreen targets
                // `full_geometry`, which ignores the reservation anyway, but
                // wrong for anything that reads `usable`/`geometry` while a
                // bar is unmapped without exiting cleanly (a crash, not just
                // AGS's cooperative fullscreen hide).
                let zone_before = map.non_exclusive_zone();
                map.unmap_layer(&layer);
                let zone_after = map.non_exclusive_zone();
                if zone_after != zone_before {
                    self.pending.borrow_mut().push(srdwm_core::Event::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
                }
                break;
            }
        }
        // A lock/launcher surface holding exclusive keyboard focus just
        // vanished (crash, or a normal close) - don't leave focus dangling
        // on a dead surface.
        //
        // `sync_keyboard_focus`, not a bare `set_keyboard_focus(None)`: an
        // `OnDemand` layer surface (a launcher/quicksettings/datemenu
        // popup, per `wlr-layer-shell`) claiming focus on click
        // (`input.rs`'s `on_demand` branch) goes straight through
        // `set_keyboard_focus` without ever touching `WindowManager::
        // focused` - core has no concept of a layer surface to focus, so
        // it still correctly points at whatever real toplevel was focused
        // before the popup opened. Hardcoding `None` here threw that away
        // regardless, leaving nothing focused until the user happened to
        // click a window again - reported live (an AGS peer session's
        // user) as "focus never returns after using the bar". `sync_
        // keyboard_focus` reads that still-correct core state and restores
        // real Wayland focus to it, falling through to `None` only if core
        // genuinely has nothing focused either.
        if self.seat.get_keyboard().and_then(|k| k.current_focus()).as_ref() == Some(surface.wl_surface()) {
            crate::input::sync_keyboard_focus(self);
        }
    }
}
