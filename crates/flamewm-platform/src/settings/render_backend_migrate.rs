use super::ProductSettings;

/// One-way: a stored legacy `renderAcceleration=gpu` becomes `renderBackend=opengl`
/// and the legacy field is reset to its default so it is never written again.
pub(super) fn migrate_legacy_acceleration(s: &mut ProductSettings, backend_was_stored: bool) {
    if !backend_was_stored && s.render_acceleration == "gpu" {
        s.render_backend = "opengl".to_owned();
    }
    s.render_acceleration = "cpu".to_owned();
}
