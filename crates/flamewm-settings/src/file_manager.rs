//! Freedesktop default file manager, backed by the shared application settings facade.
use flamewm_settings_core::xdg_applications::ApplicationSettings;

const MIME: &str = "inode/directory";
const EXPLORER: &str = "flamewm-explorer.desktop";
const DOLPHIN: [&str; 2] = ["org.kde.dolphin.desktop", "dolphin.desktop"];

fn label(id: Option<&str>) -> String {
    match id {
        Some(id) if DOLPHIN.contains(&id) => "Dolphin".into(),
        Some(EXPLORER) | None => "FlameWM Explorer".into(),
        Some(id) => id.into(),
    }
}

fn current_with(settings: &ApplicationSettings) -> Result<String, String> {
    settings
        .default_application(MIME)
        .map(|id| label(id.as_deref()))
        .map_err(|e| e.to_string())
}

pub fn current() -> String {
    if let Ok(output) = std::process::Command::new("xdg-mime")
        .args(["query", "default", MIME])
        .output()
    {
        let selected = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && !selected.trim().is_empty() {
            return label(Some(selected.trim()));
        }
    }
    ApplicationSettings::from_environment()
        .map_err(|e| e.to_string())
        .and_then(|settings| current_with(&settings))
        .unwrap_or_else(|_| "FlameWM Explorer".into())
}

fn toggle_with(settings: &ApplicationSettings, current: &str) -> Result<String, String> {
    let available = |id: &str| {
        settings
            .data_dirs
            .iter()
            .any(|root| root.join("applications").join(id).is_file())
    };
    let target = if current == "FlameWM Explorer" {
        DOLPHIN
            .into_iter()
            .find(|id| available(id))
            .ok_or("Dolphin is not installed")?
    } else {
        EXPLORER
    };
    if !available(target) {
        return Err(format!(
            "{target} is not installed. Build and install the full FlameWM package first."
        ));
    }
    settings
        .set_default_application(MIME, target)
        .map_err(|e| e.to_string())?;
    current_with(settings)
}

pub fn toggle() -> Result<String, String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    toggle_with(&settings, &current())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_uses_isolated_xdg_roots_and_preserves_other_defaults() {
        let root = std::env::temp_dir().join(format!(
            "flamewm-file-manager-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let config = root.join("config");
        let data = root.join("data");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(data.join("applications")).unwrap();
        std::fs::write(
            config.join("mimeapps.list"),
            "[Default Applications]\ntext/plain=editor.desktop;\n",
        )
        .unwrap();
        let settings = ApplicationSettings {
            config_dirs: vec![config.clone()],
            data_dirs: vec![data.clone()],
            desktops: vec![],
        };
        assert!(toggle_with(&settings, "FlameWM Explorer").is_err());
        for id in [EXPLORER, DOLPHIN[0]] {
            std::fs::write(data.join("applications").join(id), "").unwrap();
        }
        assert_eq!(
            toggle_with(&settings, "FlameWM Explorer").unwrap(),
            "Dolphin"
        );
        assert_eq!(
            toggle_with(&settings, "Dolphin").unwrap(),
            "FlameWM Explorer"
        );
        assert!(
            std::fs::read_to_string(config.join("mimeapps.list"))
                .unwrap()
                .contains("text/plain=editor.desktop;")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
