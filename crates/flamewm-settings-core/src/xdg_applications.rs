//! Lossless targeted edits for XDG application configuration files.
//!
//! Filesystem operations use explicit XDG roots, so tests never modify the real home.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flamewm_applications::ApplicationCatalog;

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// XDG roots in descending precedence, including the writable user root first.
#[derive(Debug, Clone)]
pub struct ApplicationSettings {
    pub config_dirs: Vec<PathBuf>,
    pub data_dirs: Vec<PathBuf>,
    pub desktops: Vec<String>,
}

/// A visible desktop entry that declares a MIME type in `MimeType=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledHandler {
    pub id: String,
    pub name: String,
    pub mimes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupEntry {
    pub id: String,
    pub text: String,
    pub enabled: bool,
}

impl ApplicationSettings {
    /// Resolve standard XDG paths; relative or empty XDG values are ignored.
    pub fn from_environment() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "HOME must be absolute"))?;
        let user = |key: &str, fallback: &str| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        let dirs = |key: &str, fallback: &str| {
            let raw = std::env::var_os(key)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| fallback.into());
            std::env::split_paths(&raw)
                .filter(|p| p.is_absolute())
                .collect::<Vec<_>>()
        };
        let mut config_dirs = vec![user("XDG_CONFIG_HOME", ".config")];
        config_dirs.extend(dirs("XDG_CONFIG_DIRS", "/etc/xdg"));
        let mut data_dirs = vec![user("XDG_DATA_HOME", ".local/share")];
        data_dirs.extend(dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
        let desktops = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .filter(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            })
            .map(str::to_ascii_lowercase)
            .collect();
        Ok(Self {
            config_dirs,
            data_dirs,
            desktops,
        })
    }

    /// Discover installed handlers through the shared applications catalog.
    pub fn installed_applications(&self) -> flamewm_api::FlameResult<ApplicationCatalog> {
        let dirs: Vec<_> = self
            .data_dirs
            .iter()
            .map(|p| p.join("applications"))
            .collect();
        ApplicationCatalog::from_dirs(&dirs)
    }

    /// Higher-priority desktop files mask lower-priority files, including Hidden entries.
    pub fn startup_entries(&self) -> io::Result<Vec<StartupEntry>> {
        let mut entries = BTreeMap::new();
        for root in &self.config_dirs {
            let dir = root.join("autostart");
            let files = match fs::read_dir(dir) {
                Ok(files) => files,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            for file in files {
                let file = file?;
                let Some(id) = file.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !valid_desktop_id(&id) || entries.contains_key(&id) {
                    continue;
                }
                let text = fs::read_to_string(file.path())?;
                let enabled = autostart_enabled(&text);
                entries.insert(id.clone(), StartupEntry { id, text, enabled });
            }
        }
        Ok(entries.into_values().collect())
    }

    /// Override an existing autostart entry in the user config directory.
    pub fn set_startup_enabled(&self, id: &str, enabled: bool) -> io::Result<()> {
        let entry = self
            .startup_entries()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "autostart entry not found"))?;
        let text = set_value(
            &entry.text,
            "Desktop Entry",
            "Hidden",
            if enabled { "false" } else { "true" },
        )
        .map_err(invalid)?;
        let text = set_value(
            &text,
            "Desktop Entry",
            "X-GNOME-Autostart-enabled",
            if enabled { "true" } else { "false" },
        )
        .map_err(invalid)?;
        atomic_write(&self.config_home()?.join("autostart").join(id), &text)
    }

    /// Install a supplied desktop entry without replacing an existing entry.
    pub fn add_startup(&self, id: &str, text: &str) -> io::Result<()> {
        if !valid_desktop_id(id)
            || value(text, "Desktop Entry", "Type") != Some("Application")
            || value(text, "Desktop Entry", "Name").is_none_or(str::is_empty)
            || value(text, "Desktop Entry", "Exec").is_none_or(str::is_empty)
        {
            return Err(invalid("invalid startup desktop entry"));
        }
        if self.startup_entries()?.iter().any(|entry| entry.id == id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "autostart entry already exists",
            ));
        }
        atomic_write(&self.config_home()?.join("autostart").join(id), text)
    }

    /// Remove a user entry, masking any lower-priority entry rather than reviving it.
    pub fn remove_startup(&self, id: &str) -> io::Result<()> {
        if !valid_desktop_id(id) {
            return Err(invalid("invalid desktop id"));
        }
        let user_file = self.config_home()?.join("autostart").join(id);
        if self
            .config_dirs
            .iter()
            .skip(1)
            .any(|root| root.join("autostart").join(id).exists())
        {
            return self.set_startup_enabled(id, false);
        }
        fs::remove_file(user_file)
    }

    /// Visible desktop entries that handle `mime`, in ID order.
    ///
    /// The first root that defines an ID decides it, so a hidden or NoDisplay user entry masks
    /// a visible system entry with the same ID.
    pub fn installed_handlers(&self, mime: &str) -> io::Result<Vec<InstalledHandler>> {
        if !mime.contains('/') || mime.contains(['\n', '\r', ';']) {
            return Err(invalid("invalid MIME type"));
        }
        let mut found: BTreeMap<String, Option<InstalledHandler>> = BTreeMap::new();
        for root in &self.data_dirs {
            let files = match fs::read_dir(root.join("applications")) {
                Ok(files) => files,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            for file in files {
                let file = file?;
                let Some(id) = file.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !valid_desktop_id(&id) || found.contains_key(&id) {
                    continue;
                }
                let Ok(text) = fs::read_to_string(file.path()) else {
                    continue;
                };
                let hidden = ["Hidden", "NoDisplay"]
                    .iter()
                    .any(|key| value(&text, "Desktop Entry", key) == Some("true"));
                let handler = if hidden {
                    None
                } else {
                    let mimes: Vec<String> = value(&text, "Desktop Entry", "MimeType")
                        .unwrap_or("")
                        .split(';')
                        .filter(|m| !m.is_empty())
                        .map(str::to_owned)
                        .collect();
                    mimes.iter().any(|m| m == mime).then(|| InstalledHandler {
                        name: value(&text, "Desktop Entry", "Name")
                            .filter(|n| !n.is_empty())
                            .unwrap_or(&id)
                            .to_owned(),
                        id: id.clone(),
                        mimes,
                    })
                };
                found.insert(id, handler);
            }
        }
        Ok(found.into_values().flatten().collect())
    }

    /// Resolve the first configured default, respecting desktop-specific XDG files.
    pub fn default_application(&self, mime: &str) -> io::Result<Option<String>> {
        validate_key(mime)?;
        for root in self
            .config_dirs
            .iter()
            .cloned()
            .chain(self.data_dirs.iter().map(|p| p.join("applications")))
        {
            for name in self
                .desktops
                .iter()
                .filter(|d| valid_desktop_name(d))
                .map(|d| format!("{d}-mimeapps.list"))
                .chain(std::iter::once("mimeapps.list".to_owned()))
            {
                match fs::read_to_string(root.join(name)) {
                    Ok(text) => {
                        if let Some(id) = value(&text, "Default Applications", mime)
                            .and_then(|v| v.split(';').find(|s| !s.is_empty()))
                        {
                            return Ok(Some(id.to_owned()));
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(None)
    }

    /// Persist a user-level default without disturbing unrelated associations.
    pub fn set_default_application(&self, mime: &str, id: &str) -> io::Result<()> {
        validate_key(mime)?;
        if !valid_desktop_id(id) {
            return Err(invalid("invalid desktop id"));
        }
        // Desktop-specific user files outrank generic mimeapps.list.
        let name = self
            .desktops
            .iter()
            .find(|d| valid_desktop_name(d))
            .map_or_else(
                || "mimeapps.list".to_owned(),
                |d| format!("{d}-mimeapps.list"),
            );
        let path = self.config_home()?.join(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let text =
            set_value(&text, "Default Applications", mime, &format!("{id};")).map_err(invalid)?;
        atomic_write(&path, &text)
    }

    pub(crate) fn config_home(&self) -> io::Result<&Path> {
        self.config_dirs
            .first()
            .filter(|p| p.is_absolute())
            .map(PathBuf::as_path)
            .ok_or_else(|| invalid("missing absolute user config directory"))
    }
}

pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn valid_desktop_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn valid_desktop_id(id: &str) -> bool {
    id.ends_with(".desktop")
        && id.len() > ".desktop".len()
        && !id.contains(['/', '\\', '\n', '\r', ';', '\0'])
        && id != ".desktop"
}

fn validate_key(key: &str) -> io::Result<()> {
    if key.is_empty() || key.contains(['\n', '\r', '=', '\0']) {
        Err(invalid("invalid MIME type"))
    } else {
        Ok(())
    }
}

pub(crate) fn atomic_write(path: &Path, text: &str) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| invalid("missing parent"))?;
    fs::create_dir_all(parent)?;
    for _ in 0..16 {
        let temp = parent.join(format!(
            ".flamewm-{}-{}.tmp",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let result = (|| {
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot reserve temporary file",
    ))
}

/// Return the last value for a key in a named group.
#[must_use]
pub fn value<'a>(text: &'a str, group: &str, key: &str) -> Option<&'a str> {
    let mut current = false;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            current = &line[1..line.len() - 1] == group;
        } else if current && !line.starts_with('#') {
            if let Some((name, value)) = line.split_once('=') {
                if name.trim() == key {
                    found = Some(value.trim());
                }
            }
        }
    }
    found
}

/// Replace a key without discarding unrelated groups, comments, or associations.
/// Keys, groups and values containing line breaks are rejected.
pub fn set_value(
    text: &str,
    group: &str,
    key: &str,
    new_value: &str,
) -> Result<String, &'static str> {
    if group.is_empty()
        || key.is_empty()
        || group.contains(['\n', '\r', '[', ']'])
        || key.contains(['\n', '\r', '='])
        || new_value.contains(['\n', '\r'])
    {
        return Err("invalid XDG configuration field");
    }
    let mut result = String::new();
    let mut current = false;
    let mut found_group = false;
    let mut inserted = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if current && !inserted {
                result.push_str(&format!("{key}={new_value}\n"));
                inserted = true;
            }
            current = &trimmed[1..trimmed.len() - 1] == group;
            found_group |= current;
        } else if current
            && !trimmed.starts_with('#')
            && trimmed
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            if !inserted {
                result.push_str(&format!("{key}={new_value}\n"));
                inserted = true;
            }
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    if !found_group {
        result.push_str(&format!("[{group}]\n"));
    }
    if !inserted {
        result.push_str(&format!("{key}={new_value}\n"));
    }
    Ok(result)
}

/// XDG autostart entries are enabled unless either disabling flag is set.
#[must_use]
pub fn autostart_enabled(text: &str) -> bool {
    value(text, "Desktop Entry", "Hidden") != Some("true")
        && value(text, "Desktop Entry", "X-GNOME-Autostart-enabled") != Some("false")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_home_runtime_round_trip() {
        let Some(home) = std::env::var_os("FLAMEWM_XDG_TEST_HOME").map(PathBuf::from) else {
            return;
        };
        assert!(home.is_absolute());
        assert!(home.starts_with(std::env::temp_dir()));
        assert_eq!(
            std::env::var_os("HOME"),
            Some(home.clone().into_os_string())
        );
        let settings = ApplicationSettings::from_environment().unwrap();
        assert_eq!(settings.config_dirs[0], home.join(".config"));
        let apps = home.join(".local/share/applications");
        fs::create_dir_all(&apps).unwrap();
        let desktop = "[Desktop Entry]\nType=Application\nName=Example\nExec=/bin/true\nMimeType=text/plain;\n";
        fs::write(apps.join("example.desktop"), desktop).unwrap();
        assert!(
            settings
                .installed_applications()
                .unwrap()
                .entries()
                .iter()
                .any(|e| e.id().as_str() == "example.desktop")
        );
        settings.add_startup("example.desktop", desktop).unwrap();
        settings
            .set_startup_enabled("example.desktop", false)
            .unwrap();
        assert!(!settings.startup_entries().unwrap()[0].enabled);
        settings
            .set_default_application("text/plain", "example.desktop")
            .unwrap();
        let output = std::process::Command::new("xdg-mime")
            .args(["query", "default", "text/plain"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "example.desktop"
        );
    }

    #[test]
    fn isolated_roots_obey_precedence_and_persist_overrides() {
        let root = std::env::temp_dir().join(format!(
            "flamewm-xdg-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let user = root.join("home/.config");
        let system = root.join("etc/xdg");
        let data = root.join("share");
        fs::create_dir_all(system.join("autostart")).unwrap();
        fs::create_dir_all(data.join("applications")).unwrap();
        fs::write(
            system.join("autostart/demo.desktop"),
            "[Desktop Entry]\nType=Application\nName=Demo\nExec=/bin/true\n",
        )
        .unwrap();
        fs::write(
            data.join("applications/demo.desktop"),
            "[Desktop Entry]\nType=Application\nName=Demo\nExec=/bin/true\n",
        )
        .unwrap();
        fs::write(
            system.join("mimeapps.list"),
            "[Default Applications]\ntext/plain=system.desktop;\n",
        )
        .unwrap();
        let settings = ApplicationSettings {
            config_dirs: vec![user.clone(), system.clone()],
            data_dirs: vec![data],
            desktops: vec!["test".into()],
        };
        assert_eq!(
            settings.installed_applications().unwrap().entries().len(),
            1
        );
        assert_eq!(
            settings
                .default_application("text/plain")
                .unwrap()
                .as_deref(),
            Some("system.desktop")
        );
        settings
            .set_default_application("text/plain", "demo.desktop")
            .unwrap();
        assert_eq!(
            settings
                .default_application("text/plain")
                .unwrap()
                .as_deref(),
            Some("demo.desktop")
        );
        assert!(user.join("test-mimeapps.list").exists());
        assert!(settings.startup_entries().unwrap()[0].enabled);
        settings.set_startup_enabled("demo.desktop", false).unwrap();
        assert!(!settings.startup_entries().unwrap()[0].enabled);
        assert!(
            !fs::read_to_string(system.join("autostart/demo.desktop"))
                .unwrap()
                .contains("Hidden=")
        );
        settings.set_startup_enabled("demo.desktop", true).unwrap();
        assert!(settings.startup_entries().unwrap()[0].enabled);
        settings.remove_startup("demo.desktop").unwrap();
        assert!(!settings.startup_entries().unwrap()[0].enabled);
        let custom = "[Desktop Entry]\nType=Application\nName=Custom\nExec=/bin/true\n";
        settings.add_startup("custom.desktop", custom).unwrap();
        assert!(settings.add_startup("custom.desktop", custom).is_err());
        settings.remove_startup("custom.desktop").unwrap();
        assert!(!user.join("autostart/custom.desktop").exists());
        assert!(
            settings
                .set_default_application("text/plain", "../bad.desktop")
                .is_err()
        );
        assert!(
            settings
                .set_default_application("text/plain\nInjected", "demo.desktop")
                .is_err()
        );
        assert!(fs::read_dir(&user).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn desktop_entry_edit_preserves_other_groups_and_flags() {
        let original = "# fixture\n[Desktop Entry]\nName=Example\nExec=example --flag\nHidden=true\nX-GNOME-Autostart-enabled=false\n[Desktop Action New]\nExec=example --new\n";
        assert!(!autostart_enabled(original));
        let edited = set_value(original, "Desktop Entry", "Hidden", "false").unwrap();
        assert!(!autostart_enabled(&edited));
        let edited = set_value(
            &edited,
            "Desktop Entry",
            "X-GNOME-Autostart-enabled",
            "true",
        )
        .unwrap();
        assert!(autostart_enabled(&edited));
        assert_eq!(
            value(&edited, "Desktop Entry", "Exec"),
            Some("example --flag")
        );
        assert!(edited.contains("[Desktop Action New]\nExec=example --new\n"));
        assert_eq!(
            set_value(&edited, "Desktop Entry", "Hidden", "false").unwrap(),
            edited
        );
    }

    #[test]
    fn mimeapps_edit_preserves_associations_and_other_defaults() {
        let original = "[Default Applications]\ntext/plain=old.desktop;\nx-scheme-handler/http=browser.desktop;\n[Added Associations]\ntext/plain=old.desktop;other.desktop;\n";
        let edited = set_value(
            original,
            "Default Applications",
            "text/plain",
            "new.desktop;",
        )
        .unwrap();
        assert_eq!(
            value(&edited, "Default Applications", "text/plain"),
            Some("new.desktop;")
        );
        assert_eq!(
            value(&edited, "Added Associations", "text/plain"),
            Some("old.desktop;other.desktop;")
        );
        assert_eq!(
            value(&edited, "Default Applications", "x-scheme-handler/http"),
            Some("browser.desktop;")
        );
        assert_eq!(
            set_value(
                &edited,
                "Default Applications",
                "text/plain",
                "new.desktop;"
            )
            .unwrap(),
            edited
        );
    }

    #[test]
    fn rejects_line_injection_and_creates_missing_group() {
        assert!(set_value("", "Desktop Entry", "Name", "ok\nExec=bad").is_err());
        assert_eq!(
            set_value(
                "# comment\n",
                "Default Applications",
                "text/plain",
                "a.desktop;"
            )
            .unwrap(),
            "# comment\n[Default Applications]\ntext/plain=a.desktop;\n"
        );
    }
}
