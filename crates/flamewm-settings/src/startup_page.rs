//! Startup Applications page: XDG autostart rows through the shared application settings facade.
//!
//! The page has a fixed number of slots, so the markup stays static. Each slot reads the entry at
//! its index on every action, so the list is never cached across user changes.
use flamewm_settings_core::xdg_applications::{ApplicationSettings, StartupEntry, value};

/// Visible rows on the Startup page; `ui/settings.html` declares this many slots.
pub const SLOTS: usize = 8;

/// Display name from `Name=`, falling back to the desktop ID without its suffix.
pub fn display_name(entry: &StartupEntry) -> String {
    value(&entry.text, "Desktop Entry", "Name")
        .filter(|name| !name.is_empty())
        .map_or_else(
            || entry.id.trim_end_matches(".desktop").to_owned(),
            str::to_owned,
        )
}

/// Startup entries in XDG precedence order, from the live user and system roots.
pub fn rows() -> Result<Vec<StartupEntry>, String> {
    ApplicationSettings::from_environment()
        .and_then(|settings| settings.startup_entries())
        .map_err(|error| error.to_string())
}

/// Flip the enabled state of the entry in `slot`.
pub fn toggle(slot: usize) -> Result<(), String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    toggle_with(&settings, slot)
}

/// Remove the entry in `slot`; system entries are masked, not deleted.
pub fn remove(slot: usize) -> Result<(), String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    remove_with(&settings, slot)
}

fn entry_at(settings: &ApplicationSettings, slot: usize) -> Result<StartupEntry, String> {
    settings
        .startup_entries()
        .map_err(|error| error.to_string())?
        .into_iter()
        .nth(slot)
        .ok_or_else(|| "that startup row no longer exists".to_owned())
}

fn toggle_with(settings: &ApplicationSettings, slot: usize) -> Result<(), String> {
    let entry = entry_at(settings, slot)?;
    settings
        .set_startup_enabled(&entry.id, !entry.enabled)
        .map_err(|error| error.to_string())
}

fn remove_with(settings: &ApplicationSettings, slot: usize) -> Result<(), String> {
    let entry = entry_at(settings, slot)?;
    settings
        .remove_startup(&entry.id)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Isolated XDG roots under a unique temp directory; the caller removes it.
    fn roots(tag: &str) -> (PathBuf, ApplicationSettings) {
        let root = std::env::temp_dir().join(format!(
            "flamewm-startup-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = root.join("config");
        let system = root.join("system");
        std::fs::create_dir_all(config.join("autostart")).unwrap();
        std::fs::create_dir_all(system.join("autostart")).unwrap();
        let settings = ApplicationSettings {
            config_dirs: vec![config, system],
            data_dirs: vec![root.join("data")],
            desktops: vec![],
        };
        (root, settings)
    }

    #[test]
    fn toggle_and_remove_act_on_the_listed_row_and_mask_system_entries() {
        let (root, settings) = roots("toggle");
        let system = settings.config_dirs[1].join("autostart");
        std::fs::write(
            system.join("alpha.desktop"),
            "[Desktop Entry]\nType=Application\nName=Alpha\nExec=alpha\n",
        )
        .unwrap();
        std::fs::write(
            settings.config_dirs[0].join("autostart/beta.desktop"),
            "[Desktop Entry]\nType=Application\nName=Beta\nExec=beta\nHidden=true\n",
        )
        .unwrap();

        let before = settings.startup_entries().unwrap();
        assert_eq!(before.len(), 2);
        assert_eq!(display_name(&before[0]), "Alpha");
        assert!(before[0].enabled);
        assert!(!before[1].enabled, "Hidden=true must read as disabled");

        // Disabling the system entry writes a user override and leaves the system file alone.
        toggle_with(&settings, 0).unwrap();
        let after = settings.startup_entries().unwrap();
        assert!(!after[0].enabled);
        assert!(
            std::fs::read_to_string(system.join("alpha.desktop"))
                .unwrap()
                .contains("Exec=alpha")
        );

        // Enabling the user entry clears Hidden and keeps the rest of the file.
        toggle_with(&settings, 1).unwrap();
        let beta = settings.startup_entries().unwrap()[1].clone();
        assert!(beta.enabled);
        assert!(beta.text.contains("Exec=beta"));

        // Removing the system entry masks it; removing the user entry deletes its file.
        remove_with(&settings, 0).unwrap();
        assert!(!settings.startup_entries().unwrap()[0].enabled);
        remove_with(&settings, 1).unwrap();
        assert!(!settings.config_dirs[0].join("autostart/beta.desktop").exists());

        assert!(toggle_with(&settings, 9).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn display_name_falls_back_to_the_desktop_id() {
        let entry = StartupEntry {
            id: "gamma.desktop".into(),
            text: "[Desktop Entry]\nType=Application\nName=\n".into(),
            enabled: true,
        };
        assert_eq!(display_name(&entry), "gamma");
    }
}
