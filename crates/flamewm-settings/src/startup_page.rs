//! Startup Applications page: XDG autostart rows through the shared application settings facade.
//!
//! The page has a fixed number of slots, so the markup stays static. Each slot reads the entry at
//! its index on every action, so the list is never cached across user changes.
use flamewm_settings_core::xdg_applications::{ApplicationSettings, StartupEntry, value};
use flamewm_settings_core::xdg_startup::{StartupCandidate, startup_delay};

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

/// Per-entry start delays in seconds, in the order the row button cycles through them.
pub const DELAYS: [u32; 6] = [0, 5, 10, 15, 30, 60];

/// The next delay after `current`; an off-list value moves to the next larger preset, and the
/// largest preset wraps to none.
pub fn next_delay(current: u32) -> u32 {
    DELAYS
        .iter()
        .copied()
        .find(|delay| *delay > current)
        .unwrap_or(0)
}

/// Row-button text for a delay in seconds.
pub fn delay_label(seconds: u32) -> String {
    if seconds == 0 {
        "No delay".to_owned()
    } else {
        format!("{seconds} s delay")
    }
}

/// Row-button text for an entry's current start delay.
pub fn delay_text(entry: &StartupEntry) -> String {
    delay_label(startup_delay(&entry.text))
}

/// Installed applications that can still be added, in display order.
pub fn candidates() -> Vec<StartupCandidate> {
    ApplicationSettings::from_environment()
        .and_then(|settings| settings.startup_candidates())
        .unwrap_or_default()
}

/// Advances the delay of the entry in `slot` to the next preset.
pub fn cycle_delay(slot: usize) -> Result<(), String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    cycle_delay_with(&settings, slot)
}

/// Adds the candidate at `index`, wrapping it into the current list.
pub fn add_candidate(index: usize) -> Result<(), String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    add_candidate_with(&settings, index)
}

/// Adds a one-line custom command as an autostart entry.
pub fn add_command(command: &str) -> Result<(), String> {
    let settings = ApplicationSettings::from_environment().map_err(|e| e.to_string())?;
    settings
        .add_command_startup(command)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn cycle_delay_with(settings: &ApplicationSettings, slot: usize) -> Result<(), String> {
    let entry = entry_at(settings, slot)?;
    let next = next_delay(startup_delay(&entry.text));
    settings
        .set_startup_delay(&entry.id, next)
        .map_err(|error| error.to_string())
}

fn add_candidate_with(settings: &ApplicationSettings, index: usize) -> Result<(), String> {
    let candidates = settings.startup_candidates().map_err(|e| e.to_string())?;
    if candidates.is_empty() {
        return Err("every installed application already starts at sign-in".to_owned());
    }
    let pick = &candidates[index % candidates.len()];
    settings
        .add_installed_startup(&pick.id)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn delay_presets_cycle_and_wrap() {
        assert_eq!(next_delay(0), 5);
        assert_eq!(next_delay(5), 10);
        assert_eq!(next_delay(7), 10, "off-list values move up");
        assert_eq!(next_delay(60), 0, "the largest preset wraps to none");
        assert_eq!(delay_label(0), "No delay");
        assert_eq!(delay_label(15), "15 s delay");
    }

    #[test]
    fn cycle_delay_writes_the_next_preset_to_the_user_copy() {
        let (root, settings) = roots("cycle");
        std::fs::write(
            settings.config_dirs[1].join("autostart/alpha.desktop"),
            "[Desktop Entry]\nType=Application\nName=Alpha\nExec=alpha\n",
        )
        .unwrap();

        cycle_delay_with(&settings, 0).unwrap();
        let user = std::fs::read_to_string(settings.config_dirs[0].join("autostart/alpha.desktop")).unwrap();
        assert_eq!(startup_delay(&user), 5);
        cycle_delay_with(&settings, 0).unwrap();
        let again = settings.startup_entries().unwrap();
        assert_eq!(startup_delay(&again[0].text), 10);
        assert!(cycle_delay_with(&settings, 4).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn add_candidate_wraps_the_index_and_reports_when_nothing_is_left() {
        let (root, settings) = roots("pick");
        std::fs::create_dir_all(settings.data_dirs[0].join("applications")).unwrap();
        std::fs::write(
            settings.data_dirs[0].join("applications/firefox.desktop"),
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\n",
        )
        .unwrap();

        add_candidate_with(&settings, 7).unwrap();
        assert!(settings.config_dirs[0].join("autostart/firefox.desktop").exists());
        assert!(add_candidate_with(&settings, 0).is_err(), "nothing left to add");
        std::fs::remove_dir_all(root).unwrap();
    }

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
