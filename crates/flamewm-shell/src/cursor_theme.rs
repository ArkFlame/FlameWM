//! Live `cursorTheme` gate shared by the shell and the quick-control helper. The cursor owners
//! live in the thread-local registry of the thread that created the windows, so `sync_snapshot`
//! must run on that thread.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};

/// Applied-theme memory for one thread's cursor owners.
#[derive(Debug, Default)]
pub struct CursorThemeSync {
    applied: Option<String>,
}

impl CursorThemeSync {
    fn reload_theme(
        &mut self,
        requested: &str,
        search_path: &str,
        apply: impl FnOnce(&str) -> Vec<String>,
    ) -> Vec<String> {
        let Some(theme) = flamewm_session_core::cursor_theme_reload(
            self.applied.as_deref(),
            requested,
            search_path,
        ) else {
            return Vec::new();
        };
        let errors = apply(&theme);
        if errors.is_empty() {
            self.applied = Some(theme);
        }
        errors
    }

    /// Reloads the live cursor owners when the resolved `cursorTheme` changed. A failed reload
    /// keeps the previous theme (rollback) and is retried at the next settings snapshot.
    pub fn sync_snapshot(&mut self, snapshot: &SettingsSnapshot) {
        let shadow_enabled = !matches!(
            snapshot.values.get("cursorShadow"),
            Some(SettingValue::Boolean(false))
        );
        for error in flamewm_ui_x11::apply_cursor_shadow(shadow_enabled) {
            eprintln!("flamewm-shell: cursor shadow reload failed: {error}");
        }
        let Some(SettingValue::Text(requested)) = snapshot.values.get("cursorTheme") else {
            return;
        };
        let search_path = std::env::var("XCURSOR_PATH").unwrap_or_default();
        let errors = self.reload_theme(requested, &search_path, flamewm_ui_x11::apply_cursor_theme);
        for error in errors {
            eprintln!("flamewm-shell: cursor theme reload failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CursorThemeSync;

    #[test]
    fn cursor_reload_only_when_resolved_theme_changes() {
        let root =
            std::env::temp_dir().join(format!("flamewm-shell-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Custom").join("cursors")).expect("theme");
        let search = root.display().to_string();
        let first = flamewm_session_core::cursor_theme_reload(None, "Custom", &search);
        assert_eq!(first.as_deref(), Some("Custom"));
        assert_eq!(
            flamewm_session_core::cursor_theme_reload(first.as_deref(), "Custom", &search),
            None
        );
        // An uninstalled request resolves to the same fallback as the applied theme: no reload.
        let fallback = flamewm_session_core::resolve_cursor_theme("Missing", &search);
        assert_eq!(
            flamewm_session_core::cursor_theme_reload(Some(&fallback), "Missing", &search),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_cursor_apply_keeps_old_theme_and_retries() {
        let root =
            std::env::temp_dir().join(format!("flamewm-shell-cursor-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("New").join("cursors")).expect("theme");
        let search_path = root.display().to_string();
        let mut sync = CursorThemeSync {
            applied: Some("Old".to_owned()),
        };
        let mut attempts = 0;
        let errors = sync.reload_theme("New", &search_path, |_| {
            attempts += 1;
            vec!["rejected".to_owned()]
        });
        assert_eq!(errors, vec!["rejected".to_owned()]);
        assert_eq!(sync.applied.as_deref(), Some("Old"));

        let errors = sync.reload_theme("New", &search_path, |_| {
            attempts += 1;
            Vec::new()
        });
        assert!(errors.is_empty());
        assert_eq!(sync.applied.as_deref(), Some("New"));
        assert_eq!(attempts, 2);
        assert!(sync
            .reload_theme("New", &search_path, |_| panic!(
                "unchanged theme must not reload"
            ))
            .is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
