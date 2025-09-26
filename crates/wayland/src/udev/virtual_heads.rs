//! Fully virtual "fake" monitors: a real, independent `wl_output` with no
//! DRM connector/CRTC/hardware behind it at all - the actual "multiple
//! monitors on one physical screen, or none" ask (distinct from `srd.
//! monitor.split`, which divides one *real* output's own placement
//! rectangle rather than creating a second, genuinely independent output;
//! see that feature's own doc comment). A real, if narrower, prior-art
//! comparison point: niri ships a `Headless` backend (`backend/
//! headless.rs`, cloned at `~/reference-wms/niri`) that also creates a
//! real `Output` with no hardware behind it - but its own `render()`
//! never actually composites anything, purely a no-render stub for that
//! project's own test suite. This is a genuine, visible one: it actually
//! renders whatever is placed on it, on demand.
//!
//! **Scope, stated plainly rather than silently assumed away**: a fake
//! monitor has no real display attached, so there is no "look at your
//! other monitor" the way a second physical panel gives you for free.
//! Its content is exposed the same way any output's content already is
//! to an external tool - `zwlr_screencopy_manager_v1` (already real,
//! already tested: `grim`, `wf-recorder`, a purpose-built viewer, or a
//! remote-desktop/streaming pipeline can all read it) - rendered fresh
//! on every capture request rather than continuously, since nothing
//! needs to *present* a frame nobody is watching every 16ms the way a
//! real, scanned-out head does. Three things a real head has that this
//! deliberately does not, for this first phase: no layer-shell chrome
//! (a bar/dock could bind to it and would be composited if it did, but
//! nothing here spawns one automatically), no participation in the
//! native lock's per-output "every output presented a cleared frame"
//! confirmation (`self.outputs`, `lock.rs`'s own doc comment) - a
//! monitor nothing can physically see doesn't need confirming, and
//! including it would wait forever on a frame this module never drives
//! unprompted - and no `wlr-output-management-v1` listing (a display-
//! settings panel won't offer to reposition it). All three are additive
//! if ever wanted; none block basic use (bind it, place windows on it,
//! read it back).
//!
//! Placement, positioning, per-monitor workspace assignment, and window
//! rehoming on removal all reuse the exact machinery a real hotplugged
//! monitor already goes through unmodified - `WindowManager::
//! set_monitors` already rehomes a window whose monitor vanished onto
//! whatever monitor remains (see that function's own doc comment), the
//! same safety net a real unplug relies on.

use super::*;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{Bind, Offscreen};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::Transform;

/// One fake monitor: real enough to have its own `wl_output` global, a
/// place in `WindowManager::monitors()`, and windows genuinely assigned
/// to it - just never scanned out to any real display. See this
/// module's own doc comment for the full scope.
pub(crate) struct VirtualHead {
    pub(crate) name: String,
    pub(crate) output: Output,
    pub(crate) global: GlobalId,
    pub(crate) size: (i32, i32),
    pub(crate) location: Point<i32, Logical>,
}

impl CompState {
    /// Creates a new fake monitor named `name` at `width`x`height`,
    /// placed immediately to the right of every existing head (real or
    /// fake) - the same plain left-to-right default a genuinely new
    /// real head gets, since there is no previous session's position to
    /// restore for something that was never plugged in.
    pub(crate) fn create_virtual_head(&mut self, name: String, width: i32, height: i32) -> Result<(), String> {
        if width <= 0 || height <= 0 {
            return Err("width and height must both be positive".to_string());
        }
        let Some(udev) = self.udev.as_ref() else { return Err("fake monitors need the udev (real-hardware) backend".to_string()) };
        if udev.heads.iter().any(|h| h.output.name() == name) || udev.virtual_heads.iter().any(|h| h.name == name) {
            return Err(format!("an output named {name} already exists"));
        }
        let max_x = udev
            .heads
            .iter()
            .map(|h| h.location.x + h.size.0)
            .chain(udev.virtual_heads.iter().map(|h| h.location.x + h.size.0))
            .max()
            .unwrap_or(0);
        let location: Point<i32, Logical> = (max_x, 0).into();

        let output = Output::new(name.clone(), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "srdwm".into(), model: "virtual".into() });
        let mode = OutputMode { size: (width, height).into(), refresh: 60_000 };
        output.change_current_state(Some(mode), Some(Transform::Normal), Some(smithay::output::Scale::Fractional(1.0)), Some((location.x, location.y).into()));
        output.set_preferred(mode);
        let global = output.create_global::<CompState>(&self.dh);

        self.udev.as_mut().unwrap().virtual_heads.push(VirtualHead { name, output, global, size: (width, height), location });
        // Payload discarded unread - `main.rs`'s own handler for this
        // event just re-queries the whole monitor list, same as a real
        // hotplug (see `reprobe_outputs`'s matching push).
        self.pending.borrow_mut().push(CoreEvent::MonitorAdded(srdwm_core::Monitor::new(0, "", srdwm_core::Rect::new(0, 0, 0, 0))));
        Ok(())
    }

    /// Removes the fake monitor named `name`: destroys its `wl_output`
    /// global and drops it from the virtual-head list. Any window still
    /// assigned to it is rehomed by the very next `monitors()` re-query's
    /// call into `WindowManager::set_monitors` - the same safety net a
    /// real monitor unplug already relies on, not a separate code path
    /// invented for this.
    pub(crate) fn remove_virtual_head(&mut self, name: &str) -> Result<(), String> {
        let Some(udev) = self.udev.as_mut() else { return Err("no udev backend".to_string()) };
        let Some(index) = udev.virtual_heads.iter().position(|h| h.name == name) else {
            return Err(format!("no fake monitor named {name}"));
        };
        let head = udev.virtual_heads.remove(index);
        self.dh.remove_global::<CompState>(head.global);
        self.pending.borrow_mut().push(CoreEvent::MonitorRemoved(0));
        Ok(())
    }

    /// Services every currently-pending `zwlr_screencopy_frame_v1`
    /// capture that targets a fake monitor - called once per poll,
    /// *before* `render_udev_frame` drains `self.screencopy_pending` for
    /// real heads, so a request for a fake monitor is never left sitting
    /// in that queue waiting for a real page-flip that will never come
    /// (see `render_udev_frame`'s own "left over, wasn't ready this
    /// pass" comment for the hang this would otherwise reproduce, this
    /// time permanently rather than just until the next real flip).
    ///
    /// Renders on demand rather than continuously: nothing scans this
    /// output out anywhere, so there is no reason to recomposite it
    /// every frame when no capture is currently pending. Reuses exactly
    /// `udev/capture.rs::capture_workspace`'s own off-screen-render
    /// technique (gather element list, `create_buffer`+`bind`, a fresh
    /// `OutputDamageTracker`, `render_output`) - the difference is this
    /// hands the freshly-rendered framebuffer straight to `screencopy::
    /// service_pending` instead of reading it back to a PPM file, and
    /// selects windows by `Window::monitor` (this fake monitor's own id)
    /// rather than by workspace, since a fake monitor is a genuinely
    /// independent screen with its own windows, not a mirror of
    /// whichever workspace a reference monitor happens to show.
    pub(crate) fn service_virtual_head_captures(&mut self) {
        if self.screencopy_pending.is_empty() {
            return;
        }
        let Some(udev) = self.udev.as_ref() else { return };
        if udev.virtual_heads.is_empty() {
            return;
        }
        // Every fake monitor's own `Output` identity, name, origin and
        // size - `PendingCapture::output` is already a real smithay
        // `Output` (not a per-client `WlOutput` resource), so matching it
        // against a `VirtualHead`'s own `Output` is a plain equality
        // check, the same identity `render_udev_frame`'s per-head capture
        // split already uses for real heads.
        struct HeadSummary {
            output: Output,
            name: String,
            origin: (i32, i32),
            size: (i32, i32),
        }
        let heads: Vec<HeadSummary> =
            udev.virtual_heads.iter().map(|h| HeadSummary { output: h.output.clone(), name: h.name.clone(), origin: (h.location.x, h.location.y), size: h.size }).collect();

        // Grouped by which virtual head each capture targets, moved (not
        // cloned - `PendingCapture` holds live protocol objects with no
        // `Clone` impl, and none is needed here) out of the shared queue
        // in one pass; anything left over (a real head's own capture)
        // goes straight back for `render_udev_frame`'s own drain.
        let mut by_head: std::collections::HashMap<String, Vec<crate::screencopy::PendingCapture>> = std::collections::HashMap::new();
        let mut rest = Vec::new();
        for capture in std::mem::take(&mut self.screencopy_pending) {
            match heads.iter().find(|h| h.output == capture.output) {
                Some(h) => by_head.entry(h.name.clone()).or_default().push(capture),
                None => rest.push(capture),
            }
        }
        self.screencopy_pending = rest;

        for head in heads {
            let Some(pending) = by_head.remove(&head.name) else { continue };
            let monitor_id = self.wm.borrow().monitors().iter().find(|m| (m.geometry.x, m.geometry.y) == head.origin).map(|m| m.id);
            let Some(monitor_id) = monitor_id else {
                crate::screencopy::fail_pending(pending);
                continue;
            };
            if let Err(e) = self.render_virtual_head(monitor_id, head.origin, head.size, pending) {
                log::warn!("fake monitor: render for capture failed: {e}");
            }
        }
    }

    fn render_virtual_head(&mut self, monitor_id: srdwm_core::MonitorId, origin: (i32, i32), size: (i32, i32), pending: Vec<crate::screencopy::PendingCapture>) -> Result<(), String> {
        let ids: Vec<srdwm_core::WindowId> = self.wm.borrow().visible_windows_front_to_back().filter(|w| w.monitor == monitor_id).map(|w| w.id).collect();
        let Some(udev) = self.udev.as_mut() else { return Err("no udev backend".to_string()) };
        let mut elements: Vec<crate::elements::OverlayElement<PixmanRenderer>> = Vec::new();
        for id in ids {
            let Some(w) = self.id_to_window.get(&id) else { continue };
            let Some(surface) = crate::elements::window_wl_surface(w) else { continue };
            let Some(geom) = self.wm.borrow().window(id).map(|w| w.geometry) else { continue };
            let content_offset = w.geometry().loc;
            let loc = (geom.x - origin.0 - content_offset.x, geom.y - origin.1 - content_offset.y);
            elements.extend(render_elements_from_surface_tree::<_, crate::elements::OverlayElement<PixmanRenderer>>(
                &mut udev.renderer,
                &surface,
                loc,
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }

        let (w, h) = size;
        let mut target = udev.renderer.create_buffer(Fourcc::Xrgb8888, (w, h).into()).map_err(|e| format!("create_buffer: {e}"))?;
        let mut framebuffer = udev.renderer.bind(&mut target).map_err(|e| format!("bind: {e}"))?;
        let mut tracker = OutputDamageTracker::new((w, h), 1.0, Transform::Normal);
        // Flat dark fill, not a real wallpaper - a fake monitor has no
        // `zwlr_layer_shell_v1` background client bound to it in this
        // phase (see this module's own doc comment on scope), so there
        // is nothing to composite one from; a solid color reads as
        // "empty desktop", not "broken", the same reasoning `udev/
        // capture.rs`'s own module doc comment already gives for why an
        // empty capture must not render as literal black.
        tracker.render_output(&mut udev.renderer, &mut framebuffer, 0, &elements, [0.05, 0.05, 0.08, 1.0]).map_err(|e| format!("render_output: {e:?}"))?;

        crate::screencopy::service_pending(pending, &mut udev.renderer, &framebuffer);
        Ok(())
    }
}
