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

/// Pixel operations on straight RGBA8 buffers. Implementations must leave `pixels` untouched when
/// they return `Err`, so a caller can run another backend on the same input.
pub trait PixelBackend {
    fn blur_rgba(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        radius: f32,
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
}
