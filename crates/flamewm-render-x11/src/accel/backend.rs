//! Applies the stored `renderBackend` request to the GPU/CPU switch. Only `OpenGl` reaches the
//! GPU path. Every other value, including the phase-1 fallbacks, runs the CPU path.

use flamewm_api::render_backend::{RenderBackend, choose_from_snapshot};
use flamewm_api::settings::SettingsSnapshot;

use super::{RenderAcceleration, set_render_acceleration};

#[must_use]
const fn acceleration_for_backend(backend: RenderBackend) -> RenderAcceleration {
    match backend {
        RenderBackend::OpenGl => RenderAcceleration::Gpu,
        RenderBackend::Auto | RenderBackend::Cpu | RenderBackend::Vulkan => RenderAcceleration::Cpu,
    }
}

/// Resolves the settings snapshot once and applies it. Logs the reason when a request was not
/// honoured as written.
pub fn apply_render_backend_setting(snapshot: &SettingsSnapshot) {
    let choice = choose_from_snapshot(snapshot);
    if let Some(reason) = &choice.reason {
        eprintln!("flamewm-render-x11: render backend: {reason}");
    }
    set_render_acceleration(acceleration_for_backend(choice.backend));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_opengl_selects_the_gpu_path() {
        assert_eq!(
            acceleration_for_backend(RenderBackend::OpenGl),
            RenderAcceleration::Gpu
        );
        assert_eq!(
            acceleration_for_backend(RenderBackend::Cpu),
            RenderAcceleration::Cpu
        );
        assert_eq!(
            acceleration_for_backend(RenderBackend::Auto),
            RenderAcceleration::Cpu
        );
        assert_eq!(
            acceleration_for_backend(RenderBackend::Vulkan),
            RenderAcceleration::Cpu
        );
    }
}
