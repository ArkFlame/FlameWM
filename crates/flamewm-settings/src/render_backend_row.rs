//! The Rendering row on the Appearance page: CPU, OpenGL and a disabled Vulkan segment.
//! It reads only the stored `renderBackend`. The Settings process never loads a GPU library.

use super::*;

/// Segment ids in `ui/settings.html`. Vulkan is shown disabled until it is built (phase 4).
pub(crate) const RENDER_BACKEND_OPTIONS: &[(&str, &str)] = &[
    ("cpu", "render-backend-cpu"),
    ("opengl", "render-backend-opengl"),
];

/// Status line for a stored `renderBackend`. Live device and fallback state comes from the
/// running session, which this page does not probe.
pub(crate) fn status_text(backend: &str) -> String {
    match backend {
        "opengl" => "OpenGL runs blur and backdrops on the GPU and switches live on X11. A failure falls back to CPU and logs one line. Wayland applies after restart."
            .to_owned(),
        _ => "CPU renders blur and backdrops. No GPU library is loaded.".to_owned(),
    }
}

impl<T: ControlTransport> SettingsApplication<T> {
    pub(crate) fn sync_render_backend(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let current = self
            .text_setting("renderBackend")
            .unwrap_or_else(|_| "cpu".to_owned());
        optional_text(document, "render-backend-note", status_text(&current));
        self.sync_option_setting(document, "renderBackend", RENDER_BACKEND_OPTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_names_the_backend_and_never_claims_vulkan_works() {
        assert!(status_text("opengl").starts_with("OpenGL"));
        assert!(status_text("cpu").contains("No GPU library"));
        assert!(!status_text("vulkan").contains("GPU runs"));
        assert!(
            RENDER_BACKEND_OPTIONS
                .iter()
                .all(|(value, _)| *value != "vulkan")
        );
    }
}
