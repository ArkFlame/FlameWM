use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flamewm_api::panels::PanelEdge;
use flamewm_api::ports::ShortcutPort;
use flamewm_api::settings::{SettingValue, SettingsSnapshot, SettingsTransaction};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::services::shortcuts::effective_registry;
use crate::settings::{ConfigStore, ProductSettings, STABLE_SETTINGS, stable_setting};

#[derive(Debug, Clone)]
pub struct SettingsService {
    store: ConfigStore,
    path: PathBuf,
}

impl SettingsService {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            store: ConfigStore::default(),
            path,
        }
    }

    #[must_use]
    pub fn current(&self) -> &ProductSettings {
        self.store.current()
    }

    #[must_use]
    pub fn snapshot(&self) -> SettingsSnapshot {
        to_api_snapshot(self.store.current())
    }

    pub fn load_text(&mut self, text: &str) -> FlameResult<()> {
        let mut parsed = self.store.parse_snapshot(text)?;
        if parsed.revision <= self.store.current().revision {
            parsed.revision = self.store.current().revision.saturating_add(1);
        }
        self.store.apply_snapshot(parsed)
    }

    /// Apply one complete settings transaction with native shortcut state and persistence behaving
    /// as one transaction. Native grabs are rolled back if persistence fails; no `SettingsChanged`
    /// state is published until both native state and durable state succeed.
    pub fn apply<P: ShortcutPort>(
        &mut self,
        shortcut_port: &mut P,
        transaction: &SettingsTransaction,
    ) -> FlameResult<SettingsSnapshot> {
        if transaction.expected_revision != self.store.current().revision {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "settings revision is stale",
            ));
        }
        if transaction.is_empty() {
            return Ok(self.snapshot());
        }

        let mut candidate = self.store.current().clone();
        if let Some(section) = transaction.reset_section.as_deref() {
            reset_section(&mut candidate, section)?;
        }
        for change in &transaction.changes {
            apply_change(&mut candidate, &change.key, change.value.as_ref())?;
        }
        candidate.revision = self.store.current().revision.saturating_add(1);
        candidate.validate()?;

        let changes_shortcuts = transaction_changes_shortcuts(transaction);
        if changes_shortcuts {
            let desired_shortcuts = shortcut_bindings(&candidate)?;
            if let Err(error) = shortcut_port.prepare_shortcuts(&desired_shortcuts) {
                shortcut_port.rollback_shortcuts();
                return Err(error);
            }
            if let Err(error) = shortcut_port.commit_shortcuts() {
                shortcut_port.rollback_shortcuts();
                return Err(error);
            }
        }
        if let Err(error) = ConfigStore::persist(&self.path, &candidate) {
            if changes_shortcuts {
                shortcut_port.rollback_shortcuts();
            }
            return Err(error);
        }
        self.store.apply_snapshot(candidate)?;
        Ok(self.snapshot())
    }

    /// Persists one hotkey entry through the same candidate, persist, and commit steps as `apply`.
    /// `Some(text)` stores the binding text (empty means unassigned); `None` removes the override so the
    /// action falls back to its default. The caller owns the shortcut grabs. An unchanged entry writes nothing.
    pub fn persist_hotkey(&mut self, action: &str, binding: Option<&str>) -> FlameResult<()> {
        let value = binding.map(|text| SettingValue::Text(text.to_owned()));
        let mut candidate = self.store.current().clone();
        apply_change(&mut candidate, &format!("hotkey.{action}"), value.as_ref())?;
        if candidate.hotkeys == self.store.current().hotkeys {
            return Ok(());
        }
        candidate.revision = self.store.current().revision.saturating_add(1);
        ConfigStore::persist(&self.path, &candidate)?;
        self.store.apply_snapshot(candidate)
    }

    /// Persists one per-output shell scale through the same candidate, persist, and commit steps as
    /// `persist_hotkey`. An unchanged scale writes nothing.
    pub fn persist_scale(&mut self, output: &str, scale: u16) -> FlameResult<()> {
        let mut candidate = self.store.current().clone();
        apply_change(
            &mut candidate,
            &format!("scale.{output}"),
            Some(&SettingValue::Integer(i64::from(scale))),
        )?;
        if candidate.per_output_scale == self.store.current().per_output_scale {
            return Ok(());
        }
        // Scale is Displays-domain state: the settings revision stays put so
        // Settings clients holding it are not made stale by an unannounced bump.
        ConfigStore::persist(&self.path, &candidate)?;
        self.store.adopt_scales(&candidate);
        Ok(())
    }

    /// Persists one panel edge for an output. Like `persist_scale`, the settings revision stays put
    /// because panel edge is Panels-domain state. An unchanged edge writes nothing.
    pub fn persist_panel_edge(&mut self, output: &str, edge: PanelEdge) -> FlameResult<()> {
        if self.store.current().panel_edges.get(output) == Some(&edge) {
            return Ok(());
        }
        let mut candidate = self.store.current().clone();
        candidate.panel_edges.insert(output.to_owned(), edge);
        ConfigStore::persist(&self.path, &candidate)?;
        self.store.adopt_panel_edges(&candidate);
        Ok(())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[must_use]
pub fn to_api_snapshot(settings: &ProductSettings) -> SettingsSnapshot {
    let mut values: BTreeMap<String, SettingValue> = STABLE_SETTINGS
        .iter()
        .map(|row| (row.key.to_owned(), (row.get)(settings)))
        .collect();
    values.extend(settings.per_output_scale.iter().map(|(output, scale)| {
        (
            format!("scale.{output}"),
            SettingValue::Integer(i64::from(*scale)),
        )
    }));
    values.extend(settings.hotkeys.iter().map(|(action, binding)| {
        (
            format!("hotkey.{action}"),
            SettingValue::Text(binding.clone()),
        )
    }));
    SettingsSnapshot {
        revision: settings.revision,
        values,
    }
}

fn apply_change(
    settings: &mut ProductSettings,
    key: &str,
    value: Option<&SettingValue>,
) -> FlameResult<()> {
    if let Some(row) = stable_setting(key) {
        return (row.set)(settings, &ProductSettings::default(), value, key);
    }
    if key.starts_with("scale.") {
        let output = key.trim_start_matches("scale.");
        if output.is_empty() {
            return Err(FlameError::invalid("scale output id is empty"));
        }
        match value {
            Some(SettingValue::Integer(scale)) => {
                let scale: u16 = (*scale)
                    .try_into()
                    .map_err(|_| FlameError::invalid("scale is out of range"))?;
                settings.per_output_scale.insert(output.to_owned(), scale);
            }
            None => {
                settings.per_output_scale.remove(output);
            }
            Some(_) => return Err(FlameError::invalid("scale setting requires integer")),
        }
    } else if key.starts_with("hotkey.") {
        let action = key.trim_start_matches("hotkey.");
        match value {
            Some(SettingValue::Text(binding)) => {
                settings.hotkeys.insert(action.to_owned(), binding.clone());
            }
            None => {
                settings.hotkeys.remove(action);
            }
            Some(_) => return Err(FlameError::invalid("hotkey setting requires text")),
        }
    } else {
        return Err(FlameError::new(
            ErrorCode::NotFound,
            format!("unknown setting: {key}"),
        ));
    }
    Ok(())
}

fn reset_section(settings: &mut ProductSettings, section: &str) -> FlameResult<()> {
    let section = section.to_ascii_lowercase();
    let defaults = ProductSettings::default();
    match section.as_str() {
        "displays" => settings.per_output_scale.clear(),
        "hotkeys" => settings.hotkeys.clear(),
        _ if STABLE_SETTINGS.iter().any(|row| row.section == section) => {
            for row in STABLE_SETTINGS.iter().filter(|row| row.section == section) {
                (row.set)(settings, &defaults, None, row.key)?;
            }
        }
        _ => {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "unknown settings section",
            ));
        }
    }
    Ok(())
}

fn shortcut_bindings(settings: &ProductSettings) -> FlameResult<BTreeMap<String, KeyBinding>> {
    Ok(
        effective_registry(&settings.hotkeys, settings.assistant_enabled)?
            .bindings()
            .clone(),
    )
}

fn transaction_changes_shortcuts(transaction: &SettingsTransaction) -> bool {
    transaction.reset_section.as_deref().is_some_and(|section| {
        section.eq_ignore_ascii_case("hotkeys") || section.eq_ignore_ascii_case("assistant")
    }) || transaction
        .changes
        .iter()
        .any(|change| change.key.starts_with("hotkey.") || change.key == "assistant.enabled")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings_path(stem: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{stem}-{}.conf", std::process::id()))
    }

    struct UnsupportedShortcutPort;

    impl ShortcutPort for UnsupportedShortcutPort {
        fn prepare_shortcuts(
            &mut self,
            _desired: &BTreeMap<String, KeyBinding>,
        ) -> FlameResult<()> {
            Err(FlameError::new(
                ErrorCode::Unsupported,
                "test shortcut port does not support preparation",
            ))
        }

        fn commit_shortcuts(&mut self) -> FlameResult<()> {
            Err(FlameError::new(
                ErrorCode::Unsupported,
                "test shortcut port does not support commit",
            ))
        }

        fn rollback_shortcuts(&mut self) {}
    }

    #[test]
    fn api_snapshot_uses_stable_native_configuration_keys() {
        let snapshot = to_api_snapshot(&ProductSettings::default());
        assert!(snapshot.values.contains_key("wallpaper"));
    }

    #[test]
    fn unknown_setting_is_rejected_instead_of_becoming_hidden_state() {
        let mut settings = ProductSettings::default();
        let error = apply_change(
            &mut settings,
            "invented",
            Some(&SettingValue::Boolean(true)),
        )
        .expect_err("unknown key must fail");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn wallpaper_only_apply_does_not_require_shortcut_port_support() {
        for (case, wallpaper) in [("empty", ""), ("nonempty", "/tmp/flamewm-wallpaper.png")] {
            let path = temp_settings_path(&format!("flamewm-settings-wallpaper-only-{case}"));
            let _ = std::fs::remove_file(&path);
            let mut service = SettingsService::new(path.clone());
            let mut shortcut_port = UnsupportedShortcutPort;
            let transaction = SettingsTransaction {
                expected_revision: service.snapshot().revision,
                changes: vec![flamewm_api::settings::SettingsChange {
                    key: "wallpaper".to_owned(),
                    value: Some(SettingValue::Text(wallpaper.to_owned())),
                }],
                reset_section: None,
            };

            let snapshot = service
                .apply(&mut shortcut_port, &transaction)
                .expect("wallpaper-only apply must not require shortcut support");

            assert_eq!(snapshot.revision, 2);
            assert_eq!(service.current().revision, 2);
            assert_eq!(service.current().wallpaper, wallpaper);
            assert_eq!(
                snapshot.values.get("wallpaper"),
                Some(&SettingValue::Text(wallpaper.to_owned()))
            );
            let persisted = std::fs::read_to_string(&path).expect("wallpaper settings persisted");
            assert!(persisted.contains("revision=2\n"));
            assert!(persisted.contains(&format!("wallpaper={wallpaper}\n")));
            std::fs::remove_file(path).expect("remove test settings");
        }
    }

    #[test]
    fn invalid_new_key_apply_is_rejected_before_revision_or_disk_change() {
        let path = temp_settings_path("flamewm-settings-invalid-new-key");
        let _ = std::fs::remove_file(&path);
        let mut service = SettingsService::new(path.clone());
        let mut shortcut_port = UnsupportedShortcutPort;
        let transaction = SettingsTransaction {
            expected_revision: service.snapshot().revision,
            changes: vec![flamewm_api::settings::SettingsChange {
                key: "animationSpeed".to_owned(),
                value: Some(SettingValue::Integer(300)),
            }],
            reset_section: None,
        };

        let error = service
            .apply(&mut shortcut_port, &transaction)
            .expect_err("animationSpeed outside 25..=200 must be rejected");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(service.current().revision, 1);
        assert_eq!(service.current().animation_speed, 100);
        assert!(!path.exists());
    }

    #[test]
    fn new_percent_and_text_keys_persist_and_reset_by_section() {
        let path = temp_settings_path("flamewm-settings-new-keys");
        let _ = std::fs::remove_file(&path);
        let mut service = SettingsService::new(path.clone());
        let mut shortcut_port = UnsupportedShortcutPort;
        let transaction = SettingsTransaction {
            expected_revision: service.snapshot().revision,
            changes: vec![
                flamewm_api::settings::SettingsChange {
                    key: "accentOpacity".to_owned(),
                    value: Some(SettingValue::Percent(40)),
                },
                flamewm_api::settings::SettingsChange {
                    key: "titlebarCloseColor".to_owned(),
                    value: Some(SettingValue::Text("#FF0000".to_owned())),
                },
            ],
            reset_section: None,
        };
        let snapshot = service
            .apply(&mut shortcut_port, &transaction)
            .expect("valid new keys apply");
        assert_eq!(snapshot.revision, 2);
        assert_eq!(
            snapshot.values.get("accentOpacity"),
            Some(&SettingValue::Percent(40))
        );
        let persisted = std::fs::read_to_string(&path).expect("persisted");
        assert!(persisted.contains("accentOpacity=40\n"));
        assert!(persisted.contains("titlebarCloseColor=#FF0000\n"));

        let reset = SettingsTransaction {
            expected_revision: 2,
            changes: Vec::new(),
            reset_section: Some("windows".to_owned()),
        };
        service
            .apply(&mut shortcut_port, &reset)
            .expect("windows reset applies");
        assert!(service.current().titlebar_close_color.is_empty());
        assert_eq!(service.current().accent_opacity, 40);
        std::fs::remove_file(path).expect("remove test settings");
    }

    #[test]
    fn mixed_wallpaper_and_shortcut_apply_preserves_shortcut_transaction() {
        let path = temp_settings_path("flamewm-settings-mixed-wallpaper-shortcut");
        let _ = std::fs::remove_file(&path);
        let mut service = SettingsService::new(path.clone());
        let mut shortcut_port = UnsupportedShortcutPort;
        let transaction = SettingsTransaction {
            expected_revision: service.snapshot().revision,
            changes: vec![
                flamewm_api::settings::SettingsChange {
                    key: "wallpaper".to_owned(),
                    value: Some(SettingValue::Text("/tmp/mixed-wallpaper.png".to_owned())),
                },
                flamewm_api::settings::SettingsChange {
                    key: format!("hotkey.{}", flamewm_api::shortcuts::ACTION_WINDOW_CLOSE),
                    value: Some(SettingValue::Text("Ctrl+Alt+W".to_owned())),
                },
            ],
            reset_section: None,
        };

        let error = service
            .apply(&mut shortcut_port, &transaction)
            .expect_err("shortcut changes must still require shortcut support");

        assert_eq!(error.code, ErrorCode::Unsupported);
        assert_eq!(service.current().revision, 1);
        assert!(service.current().wallpaper.is_empty());
        assert!(!path.exists());
    }
}
