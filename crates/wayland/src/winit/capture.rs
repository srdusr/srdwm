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
        if !hide_top_layers {
            custom_elements.extend(crate::elements::output_layer_elements(renderer, &self.output, (0, 0), |layer| matches!(layer, Layer::Top | Layer::Overlay)));
        }
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
                    custom_elements.extend(crate::elements::surface_content_elements(renderer, &surface, (w.geometry.x, w.geometry.y + band), w.opacity));
                }
            }
        }
        custom_elements.extend(crate::elements::output_layer_elements(renderer, &self.output, (0, 0), |layer| matches!(layer, Layer::Background | Layer::Bottom)));

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
