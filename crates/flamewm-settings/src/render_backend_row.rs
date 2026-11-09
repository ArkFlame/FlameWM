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
        let note = match &self.render_probe {
            Some(probe) => probe.clone(),
            None => status_text(&current),
        };
        optional_text(document, "render-backend-note", note);
        self.sync_option_setting(document, "renderBackend", RENDER_BACKEND_OPTIONS)
    }
}

/// Runs `flamewm-render-x11 --gpu-probe` next to this binary (PATH as fallback) and returns
/// the note line. The child owns every GL load; a hung driver is killed after 10 seconds.
pub(crate) fn run_probe_child() -> String {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("flamewm-render-x11")))
        .filter(|path| path.exists());
    let program = sibling.unwrap_or_else(|| "flamewm-render-x11".into());
    let spawned = Command::new(program)
        .arg("--gpu-probe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return "GPU check: flamewm-render-x11 could not be started".to_owned();
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return "GPU check: timed out after 10 s".to_owned();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    format!("GPU check: {}", out.trim())
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
