//! Real rounded corners on a window's own client content - GLES backend
//! only (the winit nested backend; the udev backend's `PixmanRenderer` is
//! software-only and has no shader stage to hook this into at all).
//!
//! `decoration.rs`'s `round_top_corners` only ever clipped the compositor's
//! *own* titlebar/border bitmap - its own doc comment says why: clipping
//! arbitrary client surface content needs a real per-pixel mask, "a much
//! bigger change than this cosmetic pass". This is that bigger change, for
//! the one backend that can actually do it cheaply: a custom GLES fragment
//! shader (`smithay::backend::renderer::gles::element::TextureShaderElement`,
//! compiled once via `GlesRenderer::compile_custom_texture_shader`) that
//! masks a window's texture against a rounded-rect signed-distance field
//! while sampling it, the same technique every compositor with GPU-side
//! rounded corners uses (cosmic-comp, niri). No smithay convenience wrapper
//! does this - `CropRenderElement` only crops to a rectangle - so this
//! builds the `TextureRenderElement` by hand from the surface's own
//! committed texture/view/damage state (`RendererSurfaceState`, the same
//! data `WaylandSurfaceRenderElement::from_state` reads, just via the
//! public accessors instead of that private constructor).
//!
//! Scope, deliberately: only a window's *main* surface. A window with
//! subsurfaces (a video overlay, a game's separately-committed content
//! layer) won't have them rendered at all through this path - rounding a
//! main surface correctly while leaving unclipped subsurfaces poking out
//! past the curve would look worse than not rounding, and walking a whole
//! subsurface tree through a shape mask is real additional work belonging
//! to a follow-up, not this pass. `elements::surface_content_elements`
//! (plain, unrounded, subsurface-aware) stays the fallback - see this
//! module's own call site in `winit.rs`.

use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::element::TextureShaderElement;
use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType};
use smithay::backend::renderer::utils::{with_renderer_surface_state, RendererSurfaceState};
use smithay::backend::renderer::Renderer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Point;

// `winit.rs`'s `custom_elements` element type. `crate::elements::
// OverlayElement<GlesRenderer>` already covers everything the render loop
// draws (cursor, decoration, borders, popups, plain content, layer-shell
// surfaces) but can't also carry `TextureShaderElement`: that type only
// implements `RenderElement<GlesRenderer>`, not the generic `RenderElement<R>`
// every `OverlayElement<R>` variant needs (`OverlayElement<PixmanRenderer>`,
// used identically by `udev.rs`, would stop compiling the moment a
// GLES-only variant were added to the shared enum). Wrapping the whole
// existing enum as one variant here, concrete to `GlesRenderer` from the
// start (`<=GlesRenderer>`, not `<R>`), sidesteps that without touching
// the shared type at all - `udev.rs` never sees this module.
smithay::backend::renderer::element::render_elements! {
    pub(crate) WinitElement<=GlesRenderer>;
    Base=crate::elements::OverlayElement<GlesRenderer>,
    Rounded=TextureShaderElement,
}

/// `//_DEFINES_` is smithay's own template placeholder (see `texture_program`
/// in its `shaders` module) - `#define NO_ALPHA`/`EXTERNAL`/`DEBUG_FLAGS`
/// get substituted there for whichever variant is actually compiled.
/// Everything else here mirrors the bundled `texture.frag` (uniform names
/// `tex`/`alpha`, varying `v_coords`) plus this module's own two: `size`
/// (the element's own logical size in pixels, for turning `v_coords`
/// - 0..1 across the quad - back into a pixel position) and `radius`.
///
/// `corners` (top-left, top-right, bottom-right, bottom-left; `1.0` = round,
/// `0.0` = square) lets a decorated window round only its bottom two - the
/// top two are already hidden under the titlebar band's own rounded
/// bitmap, so rounding them here too would just be redundant work, not a
/// visual difference.
///
/// The mask itself is a standard rounded-rect SDF: clamp the pixel position
/// into the rect inset by `radius` on every side, then measure distance
/// from that clamped point. Zero everywhere but the four corner zones (the
/// clamp is a no-op there, so `dist` is always `0`, `mask` always `1`);
/// inside a corner zone it's the distance to that corner's own circle
/// center, smoothed over roughly 2 physical pixels so the edge reads as an
/// anti-aliased curve instead of `round_top_corners`' deliberate hard
/// cutoff (a GPU shader can afford the smoothstep; the CPU bitmap path that
/// cuts corners elsewhere in this codebase can't justify one for a purely
/// cosmetic pass).
const FRAGMENT_SHADER: &str = r#"
#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

uniform vec2 size;
uniform float radius;
uniform vec4 corners;

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

    vec2 pos = v_coords * size;
    bool is_left = pos.x < size.x * 0.5;
    bool is_top = pos.y < size.y * 0.5;
    float corner_flag = is_top
        ? (is_left ? corners.x : corners.y)
        : (is_left ? corners.w : corners.z);

    if (corner_flag > 0.5 && radius > 0.0) {
        vec2 clamped = clamp(pos, vec2(radius), size - vec2(radius));
        float dist = distance(pos, clamped);
        float mask = 1.0 - smoothstep(radius - 1.0, radius + 1.0, dist);
        color *= mask;
    }

    gl_FragColor = color;
}
"#;

/// Which corners a call to [`rounded_content_element`] should actually
/// round - see this module's doc comment on why a decorated window only
/// wants its bottom two.
#[derive(Clone, Copy)]
pub(crate) struct RoundedCorners {
    pub top_left: bool,
    pub top_right: bool,
    pub bottom_right: bool,
    pub bottom_left: bool,
}

impl RoundedCorners {
    pub(crate) const ALL: Self = Self { top_left: true, top_right: true, bottom_right: true, bottom_left: true };
    pub(crate) const BOTTOM_ONLY: Self = Self { top_left: false, top_right: false, bottom_right: true, bottom_left: true };

    fn as_uniform_value(self) -> (f32, f32, f32, f32) {
        let f = |b: bool| if b { 1.0 } else { 0.0 };
        (f(self.top_left), f(self.top_right), f(self.bottom_right), f(self.bottom_left))
    }
}

/// Compiles [`FRAGMENT_SHADER`] once. Cheap to call more than once (it's a
/// real GL program compile+link each time), so the caller caches the
/// result - see `CompState`'s own field for where.
pub(crate) fn compile(renderer: &mut GlesRenderer) -> Result<GlesTexProgram, GlesError> {
    renderer.compile_custom_texture_shader(
        FRAGMENT_SHADER,
        &[UniformName::new("size", UniformType::_2f), UniformName::new("radius", UniformType::_1f), UniformName::new("corners", UniformType::_4f)],
    )
}

/// Builds a rounded-corner-masked render element for `surface`'s own main
/// content - not subsurfaces, not popups, see this module's doc comment
/// for the scope this is deliberately narrower than
/// `elements::surface_content_elements`'s.
///
/// `None` if the surface has no committed buffer yet (nothing to round) or
/// isn't backed by a GLES texture at all (a single-pixel-buffer solid-color
/// surface, `wp_single_pixel_buffer_v1` - rare, and trivially "already a
/// rectangle" regardless, so not worth a second code path here).
pub(crate) fn rounded_content_element(
    renderer: &mut GlesRenderer,
    program: &GlesTexProgram,
    surface: &WlSurface,
    location: (i32, i32),
    alpha: f32,
    radius: f32,
    corners: RoundedCorners,
) -> Option<TextureShaderElement> {
    smithay::wayland::compositor::with_states(surface, |states| {
        smithay::backend::renderer::utils::import_surface(renderer, states).ok()?;
        Some(())
    })?;

    let (texture, view, buffer_scale, buffer_transform, damage_snapshot) = with_renderer_surface_state(surface, |state: &mut RendererSurfaceState| {
        let texture = state.texture::<smithay::backend::renderer::gles::GlesTexture>(renderer.context_id())?.clone();
        let view = state.view()?;
        Some((texture, view, state.buffer_scale(), state.buffer_transform(), state.damage()))
    })??;

    let id = smithay::backend::renderer::element::Id::from_wayland_resource(surface);
    let location: Point<f64, smithay::utils::Physical> = Point::from((location.0 as f64 + view.offset.x as f64, location.1 as f64 + view.offset.y as f64));
    let inner = TextureRenderElement::from_texture_with_damage(
        id,
        renderer.context_id(),
        location,
        texture,
        buffer_scale,
        buffer_transform,
        Some(alpha),
        Some(view.src),
        Some(view.dst),
        None,
        damage_snapshot,
        Kind::Unspecified,
    );

    let (w, h) = (view.dst.w as f32, view.dst.h as f32);
    let (c0, c1, c2, c3) = corners.as_uniform_value();
    let uniforms = vec![
        Uniform::new("size", (w, h)),
        Uniform::new("radius", radius.min(w / 2.0).min(h / 2.0)),
        Uniform::new("corners", (c0, c1, c2, c3)),
    ];
    Some(TextureShaderElement::new(inner, program.clone(), uniforms))
}
