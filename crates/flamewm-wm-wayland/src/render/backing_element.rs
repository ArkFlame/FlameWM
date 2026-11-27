//! A low-resolution blurred backdrop drawn at full size with a rounded silhouette.
//!
//! The capture is blurred at a quarter of the output resolution, so the GPU's linear filter does the
//! upscale (the CPU used to write a full-size bilinear copy) and the `clip` program rounds the corners
//! with the same per-corner coverage the surface clip uses.

use std::borrow::BorrowMut;

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesTexProgram};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use crate::clip::{Geometry, clipped_opaque_regions};

/// Memory buffer scaled by the GPU to its element size and clipped to rounded corners.
pub struct RoundedMemoryElement {
    inner: MemoryRenderBufferRenderElement<GlowRenderer>,
    program: GlesTexProgram,
    /// Corner radii in physical pixels, `corner_radius` uniform order.
    radii: [u8; 4],
    /// Size of the uploaded texture, which `v_coords` is normalised by.
    buf_size: (f32, f32),
}

impl RoundedMemoryElement {
    #[must_use]
    pub fn new(
        inner: MemoryRenderBufferRenderElement<GlowRenderer>,
        program: &GlesTexProgram,
        radii: [u8; 4],
        buf_size: (usize, usize),
    ) -> Self {
        Self {
            inner,
            program: program.clone(),
            radii,
            buf_size: (buf_size.0 as f32, buf_size.1 as f32),
        }
    }
}

impl Element for RoundedMemoryElement {
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

impl RenderElement<GlowRenderer> for RoundedMemoryElement {
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
        let geometry = Geometry::new(self.buf_size, src, dst, false);
        {
            let gles: &mut GlesFrame<'_, '_> = frame.borrow_mut();
            gles.override_default_tex_program(self.program.clone(), geometry.uniforms(self.radii));
        }
        let drawn = self.inner.draw(frame, src, dst, damage, opaque_regions);
        // Frame state, not element state: leaving it set would clip every later element.
        let gles: &mut GlesFrame<'_, '_> = frame.borrow_mut();
        gles.clear_tex_program_override();
        drawn
    }
}

/// The source rectangle, in texels of the stored capture, that a `w` x `h` logical element samples.
///
/// Output pixel `x` reads the stored position `(x * scale + 0.5) / factor` (the mapping the CPU
/// `crop_capture` used), so the rectangle starts half a stored texel's worth before the first output
/// pixel centre and spans `w * scale / factor` texels.
#[must_use]
pub fn source_rect(
    w: usize,
    h: usize,
    scale: f64,
    factor: usize,
) -> Rectangle<f64, smithay::utils::Logical> {
    let step = scale / factor as f64;
    let start = 0.5 / factor as f64 - 0.5 * step;
    Rectangle::new(
        (start, start).into(),
        (w as f64 * step, h as f64 * step).into(),
    )
}

#[cfg(test)]
mod tests {
    use super::source_rect;

    #[test]
    fn source_rect_places_each_output_centre_where_the_cpu_crop_sampled() {
        for scale in [1.0_f64, 1.25, 2.0] {
            let rect = source_rect(100, 40, scale, 4);
            for x in [0_usize, 1, 37, 99] {
                // GPU: output pixel centre -> texel position inside the source rectangle.
                let gpu = rect.loc.x + (x as f64 + 0.5) / 100.0 * rect.size.w;
                // CPU: stored position `(x * scale + 0.5) / 4`, as in `crop_capture`.
                let cpu = (x as f64 * scale + 0.5) / 4.0;
                let diff = (gpu - cpu).abs();
                // The CPU maps pixel x (not its centre) to physical x * scale: half a pixel of
                // `scale / 4` stored texels of difference at most.
                assert!(
                    diff <= 0.5 * scale / 4.0 + 1e-9,
                    "scale {scale} x {x}: {diff}"
                );
            }
            assert!((rect.size.h - 40.0 * scale / 4.0).abs() < 1e-9);
        }
    }
}
