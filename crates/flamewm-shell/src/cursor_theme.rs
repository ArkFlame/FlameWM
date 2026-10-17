//! Live `cursorTheme` gate shared by the shell and the quick-control helper. The cursor owners
//! live in the thread-local registry of the thread that created the windows, so `sync_snapshot`
//! must run on that thread.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// The resolved cursor theme to load, or `None` when `applied` already shows it.
#[must_use]
pub fn cursor_theme_reload(applied: Option<&str>, requested: &str, search_path: &str) -> Option<String> {
    let resolved = flamewm_session_core::resolve_cursor_theme(requested, search_path);
    (applied != Some(resolved.as_str())).then_some(resolved)
}

/// Applied-theme memory for one thread's cursor owners.
#[derive(Debug, Default)]
pub struct CursorThemeSync {
    applied: Option<String>,
}

impl CursorThemeSync {
    /// Reloads the live cursor owners when the resolved `cursorTheme` changed. A failed reload
    /// keeps the previous theme (rollback) and is retried at the next settings snapshot.
    pub fn sync_snapshot(&mut self, snapshot: &SettingsSnapshot) {
        let Some(SettingValue::Text(requested)) = snapshot.values.get("cursorTheme") else {
            return;
        };
        let search_path = std::env::var("XCURSOR_PATH").unwrap_or_default();
        let Some(theme) = cursor_theme_reload(self.applied.as_deref(), requested, &search_path)
        else {
            return;
        };
        let errors = flamewm_ui_x11::apply_cursor_theme(&theme);
        if errors.is_empty() {
            self.applied = Some(theme);
        }
        for error in errors {
            eprintln!("flamewm-shell: cursor theme reload failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::cursor_theme_reload;

    #[test]
    fn cursor_reload_only_when_resolved_theme_changes() {
        let root = std::env::temp_dir().join(format!("flamewm-shell-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Custom").join("cursors")).expect("theme");
        let search = root.display().to_string();
        let first = cursor_theme_reload(None, "Custom", &search);
        assert_eq!(first.as_deref(), Some("Custom"));
        assert_eq!(cursor_theme_reload(first.as_deref(), "Custom", &search), None);
        // An uninstalled request resolves to the same fallback as the applied theme: no reload.
        let fallback = flamewm_session_core::resolve_cursor_theme("Missing", &search);
        assert_eq!(cursor_theme_reload(Some(&fallback), "Missing", &search), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
