//! Backend-neutral pixel operations. CPU and GPU renderers implement [`PixelBackend`]; X11 and
//! Wayland call the same operations and never copy shared rendering logic per backend.

/// Which pixel backend the `renderAcceleration` setting requests. `Cpu` is the default; `Gpu` is
/// opt-in and every GPU failure falls back to the CPU backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderAcceleration {
    #[default]
    Cpu,
    Gpu,
}

impl RenderAcceleration {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("gpu") {
            Self::Gpu
        } else {
            Self::Cpu
        }
    }

    #[must_use]
    pub const fn as_setting(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }
}

/// Device-pixel rounded rectangle with its origin at the top-left of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundedRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub radius: u32,
}

impl RoundedRect {
    /// Clamps the corner radius to half the shorter side, the largest radius that still fits.
    /// Backends receive normalized rectangles only.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            radius: self.radius.min(self.width.min(self.height) / 2),
            ..self
        }
    }

    /// The rectangle `thickness` pixels inside this one, with the corner radius reduced by the
    /// same amount. A stroke of this rectangle is this rectangle minus its inset.
    #[must_use]
    pub fn inset(self, thickness: u32) -> Self {
        let double = thickness.saturating_mul(2);
        Self {
            x: self.x.saturating_add(thickness as i32),
            y: self.y.saturating_add(thickness as i32),
            width: self.width.saturating_sub(double),
            height: self.height.saturating_sub(double),
            radius: self.radius.saturating_sub(thickness),
        }
    }
}

/// Pixel operations on straight RGBA8 buffers.
///
/// Fills are hard-edged: a pixel is either inside the rounded rectangle or not. Compositing is
/// `out.rgb = src.rgb * a + dst.rgb * (1 - a)` and `out.a = a + dst.a * (1 - a)` with
/// `a = color[3] / 255`, so the CPU and GPU backends compute the same result. Implementations must
/// leave `pixels` untouched when they return `Err`, so a caller can run another backend on the
/// same input.
pub trait PixelBackend {
    fn blur_rgba(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        radius: f32,
    ) -> Result<(), String>;

    /// Composites `color` through the coverage of `outer` minus `hole`. Both rectangles must be
    /// normalized by the caller.
    fn fill_rounded_rect(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    ) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_values_parse_case_insensitively_and_default_to_cpu() {
        assert_eq!(RenderAcceleration::from_setting("GPU"), RenderAcceleration::Gpu);
        assert_eq!(RenderAcceleration::from_setting(" gpu "), RenderAcceleration::Gpu);
        assert_eq!(RenderAcceleration::from_setting("cpu"), RenderAcceleration::Cpu);
        assert_eq!(RenderAcceleration::from_setting("vulkan"), RenderAcceleration::Cpu);
        assert_eq!(RenderAcceleration::Gpu.as_setting(), "gpu");
    }

    #[test]
    fn radius_is_clamped_and_stroke_inset_shrinks_the_corner() {
        let rect = RoundedRect { x: 2, y: 3, width: 10, height: 6, radius: 9 };
        assert_eq!(rect.normalized().radius, 3);
        let inner = rect.normalized().inset(1);
        assert_eq!(inner, RoundedRect { x: 3, y: 4, width: 8, height: 4, radius: 2 });
    }
}
