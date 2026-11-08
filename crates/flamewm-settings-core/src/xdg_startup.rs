//! Startup Applications additions on top of `xdg_applications`: a per-entry start delay, adding an
//! installed application, and adding a custom command. Every write goes to the user autostart
//! folder, so system entries are overridden rather than edited.

use std::fs;
use std::io;

use crate::xdg_applications::{ApplicationSettings, atomic_write, invalid, set_value, value};

/// Start delay in seconds, as GNOME-style autostart launchers read it.
pub const DELAY_KEY: &str = "X-GNOME-Autostart-Delay";
/// Read-only fallback for entries that declare the FlameWM spelling.
const FALLBACK_DELAY_KEY: &str = "X-FlameWM-Delay";

/// An installed application that has no autostart entry yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupCandidate {
    pub id: String,
    pub name: String,
}

/// Start delay in seconds from the entry text; `0` when absent or not a whole number.
#[must_use]
pub fn startup_delay(text: &str) -> u32 {
    [DELAY_KEY, FALLBACK_DELAY_KEY]
        .into_iter()
        .find_map(|key| value(text, "Desktop Entry", key))
        .and_then(|seconds| seconds.trim().parse().ok())
        .unwrap_or(0)
}

fn not_found(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, message)
}

fn catalog_error(error: flamewm_api::FlameError) -> io::Error {
    io::Error::other(error.to_string())
}

impl ApplicationSettings {
    /// Installed, visible applications with no autostart entry, sorted by name then ID.
    pub fn startup_candidates(&self) -> io::Result<Vec<StartupCandidate>> {
        let existing: Vec<String> = self.startup_entries()?.into_iter().map(|e| e.id).collect();
        let catalog = self.installed_applications().map_err(catalog_error)?;
        let mut candidates: Vec<StartupCandidate> = catalog
            .entries()
            .iter()
            .filter(|entry| {
                !entry.application.name.is_empty()
                    && !existing.iter().any(|id| id == entry.id.as_str())
            })
            .map(|entry| StartupCandidate {
                id: entry.id.as_str().to_owned(),
                name: entry.application.name.clone(),
            })
            .collect();
        candidates.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(candidates)
    }

    /// Copies an installed application's desktop entry into the user autostart folder.
    pub fn add_installed_startup(&self, id: &str) -> io::Result<()> {
        let catalog = self.installed_applications().map_err(catalog_error)?;
        let entry = catalog
            .entries()
            .iter()
            .find(|entry| entry.id.as_str() == id)
            .ok_or_else(|| not_found("application is not installed"))?;
        let text = fs::read_to_string(&entry.path)?;
        self.add_startup(id, &text)
    }

    /// Adds a one-line command as an autostart entry named after its program; returns the ID.
    pub fn add_command_startup(&self, command: &str) -> io::Result<String> {
        let command = command.trim();
        if command.chars().any(char::is_control) {
            return Err(invalid("command must be one line"));
        }
        let Some(program) = command.split_whitespace().next() else {
            return Err(invalid("command is empty"));
        };
        let name = program.rsplit('/').next().unwrap_or(program);
        let slug: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let slug = slug.trim_matches('-');
        if slug.is_empty() {
            return Err(invalid("command has no program name"));
        }
        let id = format!("{slug}.desktop");
        let text = format!("[Desktop Entry]\nType=Application\nName={name}\nExec={command}\n");
        self.add_startup(&id, &text)?;
        Ok(id)
    }

    /// Sets the start delay on the user's copy of an autostart entry; `0` removes the wait.
    pub fn set_startup_delay(&self, id: &str, seconds: u32) -> io::Result<()> {
        let entry = self
            .startup_entries()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| not_found("autostart entry not found"))?;
        let text = set_value(
            &entry.text,
            "Desktop Entry",
            DELAY_KEY,
            &seconds.to_string(),
        )
        .map_err(invalid)?;
        atomic_write(&self.config_home()?.join("autostart").join(id), &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Isolated XDG roots with a user config, a system autostart, and an installed-app folder.
    fn roots(tag: &str) -> (PathBuf, ApplicationSettings) {
        let root = std::env::temp_dir().join(format!(
            "flamewm-xdg-startup-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = root.join("config");
        let system = root.join("system");
        let data = root.join("data");
        for dir in [
            config.join("autostart"),
            system.join("autostart"),
            data.join("applications"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        let settings = ApplicationSettings {
            config_dirs: vec![config, system],
            data_dirs: vec![data],
            desktops: vec![],
        };
        (root, settings)
    }

    fn install(settings: &ApplicationSettings, file: &str, body: &str) {
        fs::write(settings.data_dirs[0].join("applications").join(file), body).unwrap();
    }

    #[test]
    fn delay_is_read_from_gnome_key_then_flamewm_fallback() {
        assert_eq!(startup_delay("[Desktop Entry]\nExec=a\n"), 0);
        assert_eq!(
            startup_delay("[Desktop Entry]\nX-GNOME-Autostart-Delay=15\n"),
            15
        );
        assert_eq!(startup_delay("[Desktop Entry]\nX-FlameWM-Delay=30\n"), 30);
        assert_eq!(
            startup_delay("[Desktop Entry]\nX-GNOME-Autostart-Delay=5\nX-FlameWM-Delay=30\n"),
            5
        );
        assert_eq!(
            startup_delay("[Desktop Entry]\nX-GNOME-Autostart-Delay=soon\n"),
            0
        );
    }

    #[test]
    fn set_delay_writes_a_user_override_and_leaves_the_system_entry() {
        let (root, settings) = roots("delay");
        let system = settings.config_dirs[1].join("autostart/alpha.desktop");
        let original = "[Desktop Entry]\nType=Application\nName=Alpha\nExec=alpha\n";
        fs::write(&system, original).unwrap();

        settings.set_startup_delay("alpha.desktop", 15).unwrap();
        let user =
            fs::read_to_string(settings.config_dirs[0].join("autostart/alpha.desktop")).unwrap();
        assert!(user.contains("X-GNOME-Autostart-Delay=15"), "{user}");
        assert!(user.contains("Exec=alpha"));
        assert_eq!(fs::read_to_string(&system).unwrap(), original);
        assert_eq!(startup_delay(&user), 15);

        settings.set_startup_delay("alpha.desktop", 0).unwrap();
        let reset =
            fs::read_to_string(settings.config_dirs[0].join("autostart/alpha.desktop")).unwrap();
        assert_eq!(startup_delay(&reset), 0);
        assert!(settings.set_startup_delay("missing.desktop", 5).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidates_skip_hidden_no_display_and_already_autostarted_apps() {
        let (root, settings) = roots("candidates");
        install(
            &settings,
            "firefox.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\n",
        );
        install(
            &settings,
            "terminal.desktop",
            "[Desktop Entry]\nType=Application\nName=Terminal\nExec=term\n",
        );
        install(
            &settings,
            "hidden.desktop",
            "[Desktop Entry]\nType=Application\nName=Hidden\nHidden=true\nExec=h\n",
        );
        install(
            &settings,
            "viewer.desktop",
            "[Desktop Entry]\nType=Application\nName=Viewer\nNoDisplay=true\nExec=v\n",
        );
        fs::write(
            settings.config_dirs[0].join("autostart/terminal.desktop"),
            "[Desktop Entry]\nType=Application\nName=Terminal\nExec=term\n",
        )
        .unwrap();

        let candidates = settings.startup_candidates().unwrap();
        assert_eq!(
            candidates,
            vec![StartupCandidate {
                id: "firefox.desktop".into(),
                name: "Firefox".into()
            }]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adding_an_installed_app_copies_its_entry_once() {
        let (root, settings) = roots("add-installed");
        install(
            &settings,
            "firefox.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\n",
        );
        install(
            &settings,
            "hidden.desktop",
            "[Desktop Entry]\nType=Application\nName=Hidden\nHidden=true\nExec=h\n",
        );

        settings.add_installed_startup("firefox.desktop").unwrap();
        let copied =
            fs::read_to_string(settings.config_dirs[0].join("autostart/firefox.desktop")).unwrap();
        assert!(copied.contains("Exec=firefox"));
        assert_eq!(
            settings
                .add_installed_startup("firefox.desktop")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            settings
                .add_installed_startup("hidden.desktop")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound,
            "hidden entries are not installable"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_command_becomes_a_named_entry_and_rejects_bad_input() {
        let (root, settings) = roots("command");
        let id = settings
            .add_command_startup("  /usr/bin/Notify-Daemon --quiet ")
            .unwrap();
        assert_eq!(id, "notify-daemon.desktop");
        let text = fs::read_to_string(settings.config_dirs[0].join("autostart").join(&id)).unwrap();
        assert!(text.contains("Name=Notify-Daemon"));
        assert!(text.contains("Exec=/usr/bin/Notify-Daemon --quiet"));
        assert_eq!(startup_delay(&text), 0);

        assert!(
            settings.add_command_startup("   ").is_err(),
            "empty command"
        );
        assert!(
            settings.add_command_startup("true\nrm -rf /").is_err(),
            "one line only"
        );
        assert!(
            settings.add_command_startup("!!!").is_err(),
            "no usable program name"
        );
        assert_eq!(
            settings
                .add_command_startup("notify-daemon")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::remove_dir_all(root).unwrap();
    }
}
