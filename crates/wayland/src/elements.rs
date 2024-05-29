//! The one render-element type for everything srdwm draws *itself*, on top
//! of client windows: its titlebars and the mouse pointer.
//!
//! `render_output` takes a single `custom_elements` slice, so these have to
//! be one type. They come from two different sources - an uploaded bitmap
//! (titlebars, the built-in cursor arrow) and a client's own surface (a
//! client-set cursor image) - hence the two variants.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem};

smithay::backend::renderer::element::render_elements! {
    pub(crate) OverlayElement<R> where
        R: ImportAll + ImportMem;
    /// A client's own surface, used for client-set cursor images.
    Surface=WaylandSurfaceRenderElement<R>,
    /// A bitmap srdwm rasterised: a titlebar, or the built-in cursor arrow.
    Memory=MemoryRenderBufferRenderElement<R>,
}
