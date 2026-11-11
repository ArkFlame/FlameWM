//! The colour-filter composite pass: the frame is rendered into an offscreen texture, then drawn
//! back through a texture program that applies the filter's 4x4 matrix to every pixel.
//!
//! Precedent: `.vendor/vela-main/compositor/src/color.c` (per-pixel output matrix) and the KDE
//! Plasma `KWin` colour-blindness effect (filter applied after the scene is composed). Choice only:
//! the shader and the pass are re-implemented here, nothing is copied.
//!
//! The pass is skipped entirely for `ColourFilter::None`, so the default path is unchanged.

use std::borrow::BorrowMut;
use std::error::Error;

use crate::render::CLEAR;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType, UniformValue,
};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, ExportMem, Frame, Offscreen, Renderer};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};

use crate::colour_filter::{ColourFilter, ColourMatrix};
use crate::render::Elem;

/// Fragment shader: un-premultiply, apply the 4x4 matrix to `(r, g, b, 1)`, clamp, re-premultiply.
const SHADER: &str = r"#version 100

//_DEFINES_

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

uniform sampler2D tex;
uniform float alpha;
uniform mat4 colour_matrix;
varying vec2 v_coords;

void main() {
    vec4 color = texture2D(tex, v_coords);
    vec3 rgb = color.a > 0.0 ? color.rgb / color.a : vec3(0.0);
    vec3 mapped = clamp((colour_matrix * vec4(rgb, 1.0)).rgb, 0.0, 1.0);
    gl_FragColor = vec4(mapped * color.a, color.a) * alpha;
}
";

/// Name and type of every uniform the program reads, declared to Smithay at compile time.
const UNIFORM: (&str, UniformType) = ("colour_matrix", UniformType::Matrix4x4);

/// Compiles the filter program. The caller caches it: a shader build per frame would stall.
pub fn compile(renderer: &mut GlowRenderer) -> Result<GlesTexProgram, GlesError> {
    let names = [UniformName::new(UNIFORM.0, UNIFORM.1)];
    let renderer: &mut GlesRenderer = renderer.borrow_mut();
    renderer.compile_custom_texture_shader(SHADER, &names)
}

/// The matrix uniform for `matrix`. Rows are the natural order of [`ColourMatrix`], so the value
/// is supplied row-major (`transpose: true`) to the column-major GLSL `mat4`.
fn matrix_uniform(matrix: &ColourMatrix) -> Uniform<'static> {
    let flat: [f32; 16] = [
        matrix[0][0],
        matrix[0][1],
        matrix[0][2],
        matrix[0][3],
        matrix[1][0],
        matrix[1][1],
        matrix[1][2],
        matrix[1][3],
        matrix[2][0],
        matrix[2][1],
        matrix[2][2],
        matrix[2][3],
        matrix[3][0],
        matrix[3][1],
        matrix[3][2],
        matrix[3][3],
    ];
    Uniform::new(
        UNIFORM.0,
        UniformValue::Matrix4x4 {
            matrices: vec![flat],
            transpose: true,
        },
    )
}

/// Readback of the filtered frame: `elements` go to an offscreen texture, which is drawn through
/// the filter program into a second offscreen texture and read back as RGBA rows, the same byte
/// layout as `offscreen_pixels`. Used by PPM dumps so they show what the filter produces.
pub fn filtered_pixels(
    renderer: &mut GlowRenderer,
    program: &GlesTexProgram,
    filter: ColourFilter,
    output: &smithay::output::Output,
    elements: &[Elem],
    size: Size<i32, Physical>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let matrix = filter
        .matrix()
        .ok_or("colour filter pass called without a filter")?;
    let buffer_size: Size<i32, Buffer> = Size::from((size.w, size.h));
    let mut source_tex: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    let mut tracker = OutputDamageTracker::from_output(output);
    {
        let mut target = renderer.bind(&mut source_tex)?;
        tracker.render_output(renderer, &mut target, 0, elements, CLEAR)?;
    }
    let mut out_tex: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    {
        let mut target = renderer.bind(&mut out_tex)?;
        let full: Rectangle<i32, Physical> = Rectangle::from_size(size);
        let source = Rectangle::from_size(buffer_size.to_f64());
        let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
        frame.clear(CLEAR.into(), &[full])?;
        {
            let gles: &mut GlesFrame<'_, '_> = frame.borrow_mut();
            let uniforms = [matrix_uniform(&matrix)];
            gles.render_texture_from_to(
                &source_tex,
                source,
                full,
                &[full],
                &[],
                Transform::Normal,
                1.0,
                Some(program),
                &uniforms,
            )?;
        }
        let _sync = frame.finish()?;
    }
    let target = renderer.bind(&mut out_tex)?;
    let mapping =
        renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Abgr8888)?;
    Ok(renderer.map_texture(&mapping)?.to_vec())
}

/// Renders `elements` into an offscreen texture the size of the output, then draws that texture
/// into the winit backend's framebuffer through the filter program. The caller submits the frame
/// with full damage: a partial repaint would leave unfiltered pixels on screen.
pub fn draw_filtered(
    backend: &mut WinitGraphicsBackend<GlowRenderer>,
    program: &GlesTexProgram,
    filter: ColourFilter,
    tracker: &mut OutputDamageTracker,
    elements: &[Elem],
    size: Size<i32, Physical>,
    clear: [f32; 4],
) -> Result<(), Box<dyn Error>> {
    let matrix = filter
        .matrix()
        .ok_or("colour filter pass called without a filter")?;
    let (renderer, mut framebuffer) = backend.bind()?;
    let buffer_size: Size<i32, Buffer> = Size::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    {
        let mut target = renderer.bind(&mut texture)?;
        tracker.render_output(renderer, &mut target, 0, elements, clear)?;
    }
    let full: Rectangle<i32, Physical> = Rectangle::from_size(size);
    let source = Rectangle::from_size(buffer_size.to_f64());
    let mut frame = renderer.render(&mut framebuffer, size, Transform::Normal)?;
    frame.clear(clear.into(), &[full])?;
    {
        let gles: &mut GlesFrame<'_, '_> = frame.borrow_mut();
        let uniforms = [matrix_uniform(&matrix)];
        gles.render_texture_from_to(
            &texture,
            source,
            full,
            &[full],
            &[],
            Transform::Normal,
            1.0,
            Some(program),
            &uniforms,
        )?;
    }
    // The swap in `submit` orders the GL work, so the sync point needs no separate wait here.
    let _sync = frame.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_is_row_major_and_named_as_the_shader_declares_it() {
        let uniform = matrix_uniform(&ColourFilter::Invert.matrix().unwrap());
        assert_eq!(uniform.name, "colour_matrix");
        assert!(SHADER.contains("uniform mat4 colour_matrix;"));
        match uniform.value {
            UniformValue::Matrix4x4 {
                matrices,
                transpose,
            } => {
                assert!(transpose, "row-major data needs transpose for GLSL mat4");
                assert_eq!(matrices[0][0], -1.0);
                assert_eq!(matrices[0][3], 1.0, "invert offset sits in the last column");
            }
            other => panic!("expected a 4x4 matrix, got {other:?}"),
        }
    }
}
