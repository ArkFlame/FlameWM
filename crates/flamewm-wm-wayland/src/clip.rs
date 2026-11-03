//! Rounded client silhouette for managed window surfaces.
//!
//! An X11 window is shaped with `XShape`, so the server clips the client's own pixels to the
//! frame's rounded outline. A Wayland client owns its buffers and cannot be shaped from the
//! outside, so the same silhouette is produced at composition time: the surface element is drawn
//! through a texture-program override that multiplies its alpha by a per-corner coverage ramp.
//!
//! The radius policy is *not* decided here — [`crate::render::client_corner_radii`] resolves the
//! same skin radius the titlebar and the X11 shape use. This module only turns those radii into
//! element-local pixels, which is also where the element's own `v_coords` live: the fragment
//! shader undoes the buffer-texture → source-sub-rect → destination mapping to get pixels.

use std::borrow::BorrowMut;

use smithay::backend::renderer::Texture;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, WaylandSurfaceTexture,
};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Size, Transform};

/// Fragment program that keeps only the rounded silhouette of the texture it samples.
///
/// Smithay substitutes the exact marker line `//_DEFINES_` with the variant's `#define`s
/// (`EXTERNAL`, `NO_ALPHA`, `DEBUG_FLAGS`), so the spelling of that line is load-bearing: a
/// different marker silently compiles the wrong variant for opaque (XRGB) and external textures.
const SHADER: &str = r"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 buf_size;
uniform vec2 src_origin;
uniform vec2 src_size;
uniform vec2 geo_size;
uniform vec4 corner_radius;
uniform float y_flip;

// One corner's coverage: 1 inside the arc, 0 outside it, one pixel of ramp in between.
float corner_coverage(vec2 geo_px, vec2 center, float radius) {
    if (radius <= 0.0)
        return 1.0;
    return 1.0 - smoothstep(radius - 0.5, radius + 0.5, distance(geo_px, center));
}

void main() {
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    // v_coords spans the whole buffer texture; the element draws a src sub-rect of it into
    // geo_size physical pixels. Undo the sub-rect and the scale to get element-local pixels.
    vec2 uv = vec2(v_coords.x, mix(v_coords.y, 1.0 - v_coords.y, y_flip));
    vec2 geo_px = (uv * buf_size - src_origin) * (geo_size / src_size);

    float coverage =
        corner_coverage(geo_px, vec2(corner_radius.x, corner_radius.x), corner_radius.x) *
        corner_coverage(geo_px, vec2(geo_size.x - corner_radius.y, corner_radius.y), corner_radius.y) *
        corner_coverage(geo_px, vec2(geo_size.x - corner_radius.z, geo_size.y - corner_radius.z), corner_radius.z) *
        corner_coverage(geo_px, vec2(corner_radius.w, geo_size.y - corner_radius.w), corner_radius.w);

    color *= alpha * coverage;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
";

/// Every uniform the program reads, declared to Smithay when the shader is compiled.
///
/// The names, order and types are asserted against [`SHADER`] by the tests: a uniform Smithay does
/// not know is a render error, and a mistyped GLSL type is silently ignored by `glUniform*`.
const UNIFORMS: [(&str, UniformType); 6] = [
    ("buf_size", UniformType::_2f),
    ("src_origin", UniformType::_2f),
    ("src_size", UniformType::_2f),
    ("geo_size", UniformType::_2f),
    ("corner_radius", UniformType::_4f),
    ("y_flip", UniformType::_1f),
];

/// Compiles the clip program. The caller caches it: compiling per frame would be a shader build
/// for every window.
pub fn compile(renderer: &mut GlowRenderer) -> Result<GlesTexProgram, GlesError> {
    let names: Vec<UniformName<'static>> = UNIFORMS
        .iter()
        .map(|(name, type_)| UniformName::new(*name, *type_))
        .collect();
    let renderer: &mut GlesRenderer = renderer.borrow_mut();
    renderer.compile_custom_texture_shader(SHADER, &names)
}

/// The shader's `v_coords` → element-local pixel mapping, mirrored on the CPU so the mapping is
/// pinned by tests instead of by a display.
///
/// All sizes are in buffer or physical pixels as named; `buf_size` is the texture size, because
/// that is what Smithay normalizes `v_coords` by (`build_texture_mat`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    buf_size: (f32, f32),
    src_origin: (f32, f32),
    src_size: (f32, f32),
    geo_size: (f32, f32),
    y_inverted: bool,
}

impl Geometry {
    /// `src`/`dst` are the values `RenderElement::draw` was given for this element.
    #[must_use]
    pub fn new(
        buf_size: (f32, f32),
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        y_inverted: bool,
    ) -> Self {
        Self {
            buf_size,
            src_origin: (src.loc.x as f32, src.loc.y as f32),
            src_size: (src.size.w as f32, src.size.h as f32),
            geo_size: (dst.size.w as f32, dst.size.h as f32),
            y_inverted,
        }
    }

    /// Element-local pixel covered by the fragment at `uv`: the shader's `geo_px`.
    ///
    /// Test-only: the shader is the production implementation, this mirrors it so the mapping can
    /// be pinned without a GPU (see the tests at the bottom of this module).
    #[cfg(test)]
    #[must_use]
    pub fn geo_px(&self, uv: (f32, f32)) -> (f32, f32) {
        let y = if self.y_inverted { 1.0 - uv.1 } else { uv.1 };
        (
            (uv.0 * self.buf_size.0 - self.src_origin.0) * (self.geo_size.0 / self.src_size.0),
            (y * self.buf_size.1 - self.src_origin.1) * (self.geo_size.1 / self.src_size.1),
        )
    }

    /// Arc centers in element-local pixels, in `corner_radius` uniform order
    /// (top left, top right, bottom right, bottom left).
    #[cfg(test)]
    #[must_use]
    pub fn corner_centers(&self, radii: [u8; 4]) -> [(f32, f32); 4] {
        let (w, h) = self.geo_size;
        [
            (f32::from(radii[0]), f32::from(radii[0])),
            (w - f32::from(radii[1]), f32::from(radii[1])),
            (w - f32::from(radii[2]), h - f32::from(radii[2])),
            (f32::from(radii[3]), h - f32::from(radii[3])),
        ]
    }

    /// The uniforms the shader reads, ready for the frame's program override.
    #[must_use]
    pub fn uniforms(&self, radii: [u8; 4]) -> Vec<Uniform<'static>> {
        vec![
            Uniform::new("buf_size", self.buf_size),
            Uniform::new("src_origin", self.src_origin),
            Uniform::new("src_size", self.src_size),
            Uniform::new("geo_size", self.geo_size),
            Uniform::new("corner_radius", radii.map(f32::from)),
            Uniform::new("y_flip", if self.y_inverted { 1.0 } else { 0.0 }),
        ]
    }
}

/// A `wl_surface` element clipped to a rounded silhouette.
///
/// Unlike X11 there is no shape mask on the surface: the client's buffer keeps its square pixels
/// and only the composited output is rounded.
pub struct RoundedSurfaceElement {
    inner: WaylandSurfaceRenderElement<GlowRenderer>,
    program: GlesTexProgram,
    /// Physical pixels per corner, `corner_radius` uniform order. The element is built for the
    /// compositor's single output scale (1.0), the same scale its geometry is placed at.
    radii: [u8; 4],
}

impl RoundedSurfaceElement {
    /// Whether this element can be clipped at all.
    ///
    /// Single-pixel-buffer surfaces are drawn as a solid color, which bypasses the texture
    /// program, and a rotated buffer would need its rotation undone in the fragment shader; both
    /// keep their square silhouette instead of being clipped at the wrong corners.
    #[must_use]
    pub fn clippable(element: &WaylandSurfaceRenderElement<GlowRenderer>) -> bool {
        matches!(element.texture(), WaylandSurfaceTexture::Texture(_))
            && element.transform() == Transform::Normal
    }

    /// Wraps an element that [`Self::clippable`] accepted.
    #[must_use]
    pub fn new(
        inner: WaylandSurfaceRenderElement<GlowRenderer>,
        program: &GlesTexProgram,
        radii: [u8; 4],
    ) -> Self {
        Self {
            inner,
            program: program.clone(),
            radii,
        }
    }

    /// The src/dst mapping for one draw, or `None` if this texture cannot be clipped after all.
    fn clip_geometry(
        &self,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
    ) -> Option<Geometry> {
        let WaylandSurfaceTexture::Texture(texture) = self.inner.texture() else {
            return None;
        };
        let size = texture.size();
        Some(Geometry::new(
            (size.w as f32, size.h as f32),
            src,
            dst,
            texture.is_y_inverted(),
        ))
    }
}

impl Element for RoundedSurfaceElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let regions = self.inner.opaque_regions(scale);
        clipped_opaque_regions(&regions, self.inner.geometry(scale).size, self.radii)
            .into_iter()
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlowRenderer> for RoundedSurfaceElement {
    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }

    fn draw(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let Some(geometry) = self.clip_geometry(src, dst) else {
            return self.inner.draw(frame, src, dst, damage, opaque_regions);
        };
        {
            let gles: &mut GlesFrame<'_, '_> = frame.borrow_mut();
            gles.override_default_tex_program(self.program.clone(), geometry.uniforms(self.radii));
        }
        let drawn = self.inner.draw(frame, src, dst, damage, opaque_regions);
        // The override is frame state, not element state: leaving it set would clip every later
        // element in the same frame, including the successful path's own siblings.
        let gles: &mut GlesFrame<'_, '_> = frame.borrow_mut();
        gles.clear_tex_program_override();
        drawn
    }
}

/// `regions` clipped to `geo`, minus the square around every rounded corner.
///
/// The shader turns each corner square's outer half transparent, so claiming it opaque would let
/// the damage tracker draw that square with blending disabled and write the discarded pixels back.
#[must_use]
pub fn clipped_opaque_regions(
    regions: &[Rectangle<i32, Physical>],
    geo: Size<i32, Physical>,
    radii: [u8; 4],
) -> Vec<Rectangle<i32, Physical>> {
    let element = Rectangle::from_size(geo);
    let local: Vec<Rectangle<i32, Physical>> = regions
        .iter()
        .filter_map(|region| element.intersection(*region))
        .collect();
    let corners = corner_squares(geo, radii);
    if corners.is_empty() {
        return local;
    }
    Rectangle::subtract_rects_many_in_place(local, corners)
}

/// The square around each rounded corner of a `geo` element, in element-local pixels.
#[must_use]
pub fn corner_squares(geo: Size<i32, Physical>, radii: [u8; 4]) -> Vec<Rectangle<i32, Physical>> {
    let (w, h) = (geo.w, geo.h);
    let limit = w.min(h).max(0);
    radii
        .iter()
        .enumerate()
        .filter_map(|(corner, radius)| {
            let radius = i32::from(*radius).min(limit);
            if radius <= 0 {
                return None;
            }
            let (x, y) = match corner {
                0 => (0, 0),
                1 => (w - radius, 0),
                3 => (0, h - radius),
                _ => (w - radius, h - radius),
            };
            Some(Rectangle::new((x, y).into(), (radius, radius).into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(buf: (f32, f32), src: (f64, f64, f64, f64), dst: (i32, i32, i32, i32)) -> Geometry {
        Geometry::new(
            buf,
            Rectangle::new((src.0, src.1).into(), (src.2, src.3).into()),
            Rectangle::new((dst.0, dst.1).into(), (dst.2, dst.3).into()),
            false,
        )
    }

    /// A 1:1 element (buffer, src and destination all the same size) maps texture coordinates
    /// straight onto element pixels, which is what makes the arc centers land on the corners.
    #[test]
    fn one_to_one_element_maps_uv_straight_to_pixels() {
        let geo = geometry((200.0, 120.0), (0.0, 0.0, 200.0, 120.0), (40, 60, 200, 120));
        assert_eq!(geo.geo_px((0.0, 0.0)), (0.0, 0.0));
        assert_eq!(geo.geo_px((1.0, 1.0)), (200.0, 120.0));
        assert_eq!(geo.geo_px((0.5, 0.25)), (100.0, 30.0));
        assert_eq!(
            geo.corner_centers([24, 24, 24, 24]),
            [(24.0, 24.0), (176.0, 24.0), (176.0, 96.0), (24.0, 96.0)],
            "arc centers sit one radius inside each corner"
        );
    }

    /// A scaled element draws a src sub-rect of a larger buffer: the shader must undo both the
    /// sub-rect origin and the buffer→destination scale.
    #[test]
    fn scaled_element_undoes_source_origin_and_scale() {
        let geo = geometry(
            (400.0, 400.0),
            (100.0, 200.0, 200.0, 200.0),
            (0, 0, 100, 100),
        );
        assert_eq!(geo.geo_px((0.25, 0.5)), (0.0, 0.0));
        assert_eq!(geo.geo_px((0.75, 1.0)), (100.0, 100.0));
        assert_eq!(geo.geo_px((0.5, 0.75)), (50.0, 50.0));
        assert_eq!(
            geo.corner_centers([10, 20, 30, 40]),
            [(10.0, 10.0), (80.0, 20.0), (70.0, 70.0), (40.0, 60.0)],
            "each corner uses its own radius"
        );
    }

    /// Bottom-up GL textures are sampled with a flipped `v`, exactly like `build_texture_mat`.
    #[test]
    fn y_inverted_textures_flip_before_the_mapping() {
        let flipped = Geometry::new(
            (200.0, 120.0),
            Rectangle::new((0.0, 0.0).into(), (200.0, 120.0).into()),
            Rectangle::new((0, 0).into(), (200, 120).into()),
            true,
        );
        assert_eq!(flipped.geo_px((0.0, 0.0)), (0.0, 120.0));
        assert_eq!(flipped.geo_px((1.0, 1.0)), (200.0, 0.0));
    }

    #[test]
    fn uniforms_match_the_declared_names_and_glsl_types() {
        let geo = geometry((10.0, 20.0), (0.0, 0.0, 10.0, 20.0), (0, 0, 10, 20));
        let uniforms = geo.uniforms([24, 0, 24, 0]);
        assert_eq!(uniforms.len(), UNIFORMS.len());
        for (uniform, (name, type_)) in uniforms.iter().zip(UNIFORMS) {
            assert_eq!(uniform.name, name);
            assert_eq!(uniform.value.type_(), type_);
            let glsl = match type_ {
                UniformType::_1f => "float",
                UniformType::_2f => "vec2",
                UniformType::_4f => "vec4",
                other => panic!("no GLSL type for {other:?}"),
            };
            assert!(
                SHADER.contains(&format!("uniform {glsl} {name};")),
                "the shader must declare `uniform {glsl} {name};`"
            );
        }
    }

    /// Smithay only substitutes this exact marker; anything else compiles every variant without
    /// its `#define`, which silently darkens opaque clients instead of clipping them.
    #[test]
    fn shader_uses_the_variant_marker_smithay_substitutes() {
        assert!(SHADER.starts_with("#version 100"));
        assert!(SHADER.contains("\n//_DEFINES_\n"));
        assert!(SHADER.contains("uniform sampler2D tex;"));
        assert!(SHADER.contains("uniform float alpha;"));
        assert!(SHADER.contains("varying vec2 v_coords;"));
        assert!(SHADER.contains("#if defined(NO_ALPHA)"));
        assert!(SHADER.contains("#if defined(EXTERNAL)"));
        assert!(SHADER.contains("uniform float tint;"));
    }

    #[test]
    fn corner_squares_cover_only_rounded_corners() {
        let geo = Size::from((100, 80));
        assert!(corner_squares(geo, [0, 0, 0, 0]).is_empty());
        assert_eq!(
            corner_squares(geo, [0, 24, 0, 24]),
            vec![
                Rectangle::new((76, 0).into(), (24, 24).into()),
                Rectangle::new((0, 56).into(), (24, 24).into()),
            ]
        );
        // A radius larger than the element is clamped, never a rectangle outside it.
        for square in corner_squares(Size::from((10, 4)), [9, 9, 9, 9]) {
            assert!(square.size.w <= 4 && square.size.h <= 4, "{square:?}");
        }
    }

    #[test]
    fn clipped_opaque_regions_drop_the_corner_squares() {
        let geo = Size::from((100, 80));
        let full = [Rectangle::from_size(geo)];
        let clipped = clipped_opaque_regions(&full, geo, [24, 24, 24, 24]);
        let covers = |x: i32, y: i32| {
            clipped.iter().any(|rect| {
                x >= rect.loc.x
                    && x < rect.loc.x + rect.size.w
                    && y >= rect.loc.y
                    && y < rect.loc.y + rect.size.h
            })
        };
        for (x, y) in [(0, 0), (99, 0), (0, 79), (99, 79), (23, 23)] {
            assert!(!covers(x, y), "corner pixel ({x},{y}) must not be opaque");
        }
        for (x, y) in [(50, 40), (24, 0), (0, 24), (75, 55)] {
            assert!(covers(x, y), "interior pixel ({x},{y}) stays opaque");
        }
        // A square silhouette keeps the client's regions untouched.
        assert_eq!(clipped_opaque_regions(&full, geo, [0; 4]), full.to_vec());
        // Regions outside the element are dropped instead of clipping the arc incorrectly.
        let outside = [Rectangle::new((200, 200).into(), (10, 10).into())];
        assert!(clipped_opaque_regions(&outside, geo, [24; 4]).is_empty());
    }
}
