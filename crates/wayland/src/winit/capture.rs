use super::*;

impl WaylandPlatform {

    /// Re-renders the current scene into an offscreen GLES renderbuffer and
    /// serves the queued screencopy captures from it. See the call site for
    /// why capture cannot read the window surface directly.
    pub(super) fn capture_offscreen(&mut self, captures: Vec<screencopy::PendingCapture>) -> PlatformResult<()> {
        use smithay::backend::renderer::{Bind, Offscreen};

        let size = self.output.current_mode().map(|m| m.size).unwrap_or_default();
        if size.w <= 0 || size.h <= 0 {
            return Ok(());
        }
        let renderer = self.backend.renderer();
        let mut target: smithay::backend::renderer::gles::GlesRenderbuffer =
            renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into()).map_err(err)?;
        let mut framebuffer = renderer.bind(&mut target).map_err(err)?;

        // Not full parity with the on-screen render loop above (no border/
        // shadow strips here, same as before this function's content/opacity
        // fix) - a real, pre-existing gap in what a screenshot shows on
        // this backend, flagged rather than grown further in this pass.
        // Content (with each window's own `opacity`, unlike the
        // `self.state.space`-based single-alpha call this replaced) and the
        // bar/dock now render into the capture, at least: a screenshot used
        // to only ever show titlebars for windows that had one, and never
        // any layer-shell surface at all.
        let hide_top_layers = self.wm.borrow().visible_windows_front_to_back().any(|w| w.fullscreen);
        let mut custom_elements: Vec<crate::elements::OverlayElement<GlesRenderer>> = Vec::new();
        // Popups first, so they land above everything else - exactly the
        // order both on-screen render loops already use.
        //
        // This pass had no popup step at all, so no tooltip, dropdown or
        // right-click menu could ever appear in a screenshot taken on this
        // backend, no matter how correctly it was drawn on screen. That is
        // a capture-only gap, not a rendering one: the DRM backend serves
        // screencopy out of its own on-screen frame (`udev/render.rs`
        // drains `screencopy_pending` and hands `service_pending` the same
        // framebuffer it just drew), so it never had the gap; this backend
        // renders the scene a second time into an offscreen buffer, and
        // that second scene was missing a tier.
        //
        // It cost real time to find. The whole point of the nested backend
        // is validating behaviour with `grim`, and this made `grim` state
        // the opposite of the truth about every popup: a menu that drew
        // perfectly on screen photographed as absent, which reads exactly
        // like the client never opened one.
        let popup_targets = crate::elements::popup_targets(&self.state);
        custom_elements.extend(crate::elements::popup_render_elements(&popup_targets, renderer, (0, 0)));
        if !hide_top_layers {
            custom_elements.extend(crate::elements::output_layer_elements(renderer, &self.output, |layer| matches!(layer, Layer::Top | Layer::Overlay)));
        }
        // Front-to-back, so a window already pushed occludes everything a
        // later one draws - accumulated for the shadow clip below, exactly
        // as both on-screen render loops do it.
        let monitor_bounds: Vec<srdwm_core::Rect> = self.wm.borrow().monitors().iter().map(|m| m.full_geometry).collect();
        let mut occluders: Vec<srdwm_core::Rect> = Vec::new();
        for id in self.wm.borrow().visible_windows_front_to_back().map(|w| w.id).collect::<Vec<_>>() {
            let Some(w) = self.wm.borrow().window(id).cloned() else { continue };
            if let Some(deco) = self.state.decorations.get(&id) {
                if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(renderer, (w.geometry.x as f64, w.geometry.y as f64), deco, None, None, None, Kind::Unspecified) {
                    custom_elements.push(crate::elements::OverlayElement::Memory(elem));
                }
            }
            if let Some(dwindow) = self.state.id_to_window.get(&id) {
                if let Some(surface) = crate::elements::window_wl_surface(dwindow) {
                    let band = if w.decorated { srdwm_core::TITLEBAR_HEIGHT as i32 } else { 0 };
                    // `content_offset`: same `xdg_surface.set_window_geometry`
                    // subtraction every other render/capture path in this
                    // codebase already does (`udev/render.rs`, `winit/
                    // render.rs`, `udev/capture.rs`) - missed here
                    // specifically. A CSD client's invisible shadow margin
                    // landed at `w.geometry.x, w.geometry.y + band` instead
                    // of its real visible content, so a screenshot taken on
                    // this backend showed the same content_offset-sized gap
                    // the on-screen render loops already had fixed.
                    let content_offset = dwindow.geometry().loc;
                    let pos = (w.geometry.x - content_offset.x, w.geometry.y + band - content_offset.y);
                    custom_elements.extend(crate::elements::surface_content_elements(renderer, &surface, pos, w.opacity));
                }
            }
            // The drop shadow, last in this window's own group so it sits
            // under its own decoration and content but still over every
            // window behind it. `shadow_buffers` only holds an entry for a
            // window that is meant to have one at all (see
            // `state/lifecycle.rs`), so no separate floating/maximized
            // check belongs here.
            //
            // Shadows were the other half of this pass's missing tier: a
            // screenshot taken on this backend showed no shadow on any
            // window, which is exactly the thing a shadow bug gets reported
            // and re-checked by. Border strips are still absent - a real
            // remaining gap, called out here rather than left silent.
            if let Some(shadow) = self.state.shadow_buffers.get(&id) {
                let full = crate::decoration::shadow_rect(w.geometry);
                let rect = crate::decoration::shadow_rect_clipped(w.geometry, &monitor_bounds);
                for fragment in crate::elements::visible_border_fragments(rect, &occluders) {
                    let src = Rectangle::new(
                        Point::from(((fragment.x - full.x) as f64, (fragment.y - full.y) as f64)),
                        Size::from((fragment.width as f64, fragment.height as f64)),
                    );
                    match MemoryRenderBufferRenderElement::from_buffer(renderer, (fragment.x as f64, fragment.y as f64), shadow, None, Some(src), None, Kind::Unspecified) {
                        Ok(elem) => custom_elements.push(crate::elements::OverlayElement::Memory(elem)),
                        Err(e) => log::warn!("screencopy: failed to import shadow buffer for window {id}: {e}"),
                    }
                }
            }
            occluders.push(w.geometry);
        }
        custom_elements.extend(crate::elements::output_layer_elements(renderer, &self.output, |layer| matches!(layer, Layer::Background | Layer::Bottom)));

        // A throwaway damage tracker, so this pass always draws the whole
        // scene (age 0) and never perturbs the on-screen tracker's history.
        let mut tracker = OutputDamageTracker::from_output(&self.output);
        tracker
            .render_output(renderer, &mut framebuffer, 0, &custom_elements, [0.05, 0.05, 0.08, 1.0])
            .map_err(err)?;

        screencopy::service_pending(captures, renderer, &framebuffer);
        Ok(())
    }
}
