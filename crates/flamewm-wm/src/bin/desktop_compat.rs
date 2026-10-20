//! Session-scoped KDE application compatibility without impersonating a Plasma session.
//!
//! KDE as a global XDG desktop identity allows KDE-only display managers to
//! activate and race FlameWM's own XRandR authority. Keep the session FlameWM,
//! borrow only MIME defaults and Brave's wallet backend on application launch.
use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

fn installed(executable: &Path) -> bool {
    executable.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn atomic_owned_file(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid compatibility destination")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = fs::OpenOptions::new().create_new(true).write(true).mode(0o600)
        .open(&temp).map_err(|e| e.to_string())?;
    let outcome = file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temp, path));
    if outcome.is_err() { let _ = fs::remove_file(&temp); }
    outcome.map_err(|e| e.to_string())
}

fn defaults_from_kde(text: &str) -> BTreeMap<String, String> {
    let mut in_defaults = false;
    let mut defaults = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            in_defaults = line == "[Default Applications]";
        } else if in_defaults && !line.starts_with('#') && !line.starts_with(';') {
            if let Some((mime, apps)) = line.split_once('=') {
                let (mime, apps) = (mime.trim(), apps.trim());
                if mime.contains('/') && !mime.contains(char::is_whitespace) && !apps.is_empty() {
                    defaults.entry(mime.to_owned()).or_insert_with(|| apps.to_owned());
                }
            }
        }
    }
    defaults
}

/// KDE desktop-specific defaults can live in user config, site config or
/// distribution application data. Higher-priority entries win, exactly as in
/// the MIME Apps lookup order; never copy host KDE settings into the user file.
fn kde_mime_defaults(config_home: &Path, data_home: &Path,
                     config_dirs: &[PathBuf], data_dirs: &[PathBuf]) -> BTreeMap<String, String> {
    let paths = std::iter::once(config_home.join("kde-mimeapps.list"))
        .chain(config_dirs.iter().map(|dir| dir.join("kde-mimeapps.list")))
        .chain(std::iter::once(data_home.join("applications/kde-mimeapps.list")))
        .chain(data_dirs.iter().map(|dir| dir.join("applications/kde-mimeapps.list")));
    let mut merged = BTreeMap::new();
    for path in paths {
        if let Ok(contents) = fs::read_to_string(path) {
            if contents.len() <= 1_048_576 {
                for (mime, apps) in defaults_from_kde(&contents) {
                    merged.entry(mime).or_insert(apps);
                }
            }
        }
    }
    merged
}

fn desktop_id_installed(data_dirs: &[PathBuf], desktop_id: &str) -> bool {
    data_dirs.iter().any(|dir| dir.join("applications").join(desktop_id).is_file())
}

fn mime_defaults(mut defaults: BTreeMap<String, String>, data_dirs: &[PathBuf]) -> String {
    for (mime, candidates) in [
        ("text/plain", &["org.kde.kate.desktop", "kate.desktop"][..]),
        ("video/mp4", &["org.kde.haruna.desktop", "org.kde.dragonplayer.desktop", "vlc.desktop", "mpv.desktop"][..]),
    ] {
        // KDE can retain defaults for applications that were uninstalled.
        // Keep its preference order, but never shadow an installed fallback
        // with a stale entry that sends Dolphin to Open With.
        let installed = defaults.get(mime).map(|apps| {
            apps.split(';').filter(|id| !id.is_empty() && desktop_id_installed(data_dirs, id))
                .collect::<Vec<_>>()
        }).unwrap_or_default();
        if !installed.is_empty() {
            defaults.insert(mime.to_owned(), format!("{};", installed.join(";")));
        } else if let Some(id) = candidates.iter().find(|id| desktop_id_installed(data_dirs, id)) {
            defaults.insert(mime.to_owned(), format!("{id};"));
        } else {
            defaults.remove(mime);
        }
    }
    let mut text = String::from("# Generated fallback. User ~/.config/mimeapps.list always has precedence.\n[Default Applications]\n");
    for (mime, apps) in defaults { text.push_str(&format!("{mime}={apps}\n")); }
    text
}

/// Only application desktop entry Exec= lines get a KDE environment. Never
/// export KDE globally, and never modify Brave's own installed desktop file.
fn brave_desktop_entry(original: &str) -> String {
    let mut result = String::new();
    for line in original.lines() {
        if let Some(exec) = line.strip_prefix("Exec=") {
            if !exec.contains("XDG_CURRENT_DESKTOP=") && !exec.contains("--password-store=") {
                result.push_str("Exec=env XDG_CURRENT_DESKTOP=KDE ");
                result.push_str(exec);
            } else {
                result.push_str(line);
            }
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    result
}

/// Configure the session's XDG_DATA_DIRS / XDG_CONFIG_DIRS overlays. The
/// overlay is below the user's XDG_CONFIG_HOME/XDG_DATA_HOME but above system
/// defaults: user-chosen associations and desktop overrides always win.
/// Degraded compatibility is nonfatal; Plasma files and wallets are untouched.
pub fn configure(
    home: &Path,
    processes: &mut [flamewm_session_core::ProcessPlan],
) -> Result<(), String> {
    let first = processes.first().ok_or("missing FlameWM process plan")?;
    let config_home = first.environment.get("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| home.join(".config"));
    let data_home = first.environment.get("XDG_DATA_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| home.join(".local/share"));
    let existing_data_dirs = first.environment.get("XDG_DATA_DIRS").cloned()
        .or_else(|| std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let data_dirs = std::iter::once(data_home.clone())
        .chain(std::env::split_paths(&existing_data_dirs)).collect::<Vec<_>>();
    let existing_config_dirs = first.environment.get("XDG_CONFIG_DIRS").cloned()
        .or_else(|| std::env::var("XDG_CONFIG_DIRS").ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "/etc/xdg".into());
    let config_dirs = std::env::split_paths(&existing_config_dirs).collect::<Vec<_>>();
    let owned = config_home.join("flamewm/session-compat");
    let config_dir = owned.join("config");
    let data_dir = owned.join("data");

    let kde = kde_mime_defaults(&config_home, &data_home, &config_dirs, &data_dirs);
    atomic_owned_file(&config_dir.join("flamewm-mimeapps.list"),
                      &mime_defaults(kde, &data_dirs))?;

    // Only Brave gets a KDE identity for its encrypted-profile key backend.
    // All other applications remain FlameWM to avoid activating KScreen.
    let wallet = first.environment.get("KDE_SESSION_VERSION").map(String::as_str);
    if matches!(wallet, Some("5" | "6")) {
        for id in ["brave-browser.desktop", "brave-browser-stable.desktop", "com.brave.Browser.desktop"] {
            if let Some(contents) = data_dirs.iter()
                .map(|dir| dir.join("applications").join(id))
                .find_map(|path| fs::read_to_string(path).ok())
            {
                if contents.len() <= 1_048_576 && contents.contains("[Desktop Entry]") {
                    if let Err(error) = atomic_owned_file(&data_dir.join("applications").join(id),
                                                           &brave_desktop_entry(&contents)) {
                        eprintln!("flamewm-session: Brave desktop override {id} unavailable: {error}");
                    }
                }
            }
        }
    }
    for plan in processes {
        let config = plan.environment.get("XDG_CONFIG_DIRS").cloned()
            .unwrap_or_else(|| existing_config_dirs.clone());
        plan.environment.insert("XDG_CONFIG_DIRS".into(), format!("{}:{config}", config_dir.display()));
        let data = plan.environment.get("XDG_DATA_DIRS").cloned()
            .unwrap_or_else(|| existing_data_dirs.clone());
        plan.environment.insert("XDG_DATA_DIRS".into(), format!("{}:{data}", data_dir.display()));
    }
    Ok(())
}

/// KDE's PAM module already owns the login credential. pam_kwallet_init only
/// completes the existing handshake; it must never prompt for or store secrets.
/// When SDDM has no PAM KWallet support this cannot unlock the wallet.
pub fn finalize_pam_kwallet_login(environment: &BTreeMap<String, String>) {
    if !matches!(environment.get("KDE_SESSION_VERSION").map(String::as_str), Some("5" | "6")) {
        return;
    }
    for path in [
        "/usr/lib/x86_64-linux-gnu/libexec/pam_kwallet_init",
        "/usr/lib/pam_kwallet_init", "/usr/libexec/pam_kwallet_init",
        "/usr/lib/x86_64-linux-gnu/pam_kwallet_init",
    ] {
        if installed(Path::new(path)) {
            match Command::new(path).envs(environment).spawn() {
                Ok(mut child) => { std::thread::spawn(move || { let _ = child.wait(); }); }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => eprintln!("flamewm-session: KWallet PAM initialization unavailable: {error}"),
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kde_associations_preserved_and_common_defaults_only_fill_missing_types() {
        let kde = "[Added Associations]\ntext/plain=bad.desktop;\n[Default Applications]\ntext/plain=org.kde.kate.desktop;\nvideo/mp4=org.kde.haruna.desktop;\n";
        assert_eq!(defaults_from_kde(kde).get("text/plain").map(String::as_str), Some("org.kde.kate.desktop;"));
        let text = mime_defaults(defaults_from_kde(kde), &[]);
        assert!(text.contains("text/plain=org.kde.kate.desktop;"));
        assert!(text.contains("video/mp4=org.kde.haruna.desktop;"));
    }
    #[test]
    fn missing_kde_video_player_falls_back_to_an_installed_handler() {
        let root = std::env::temp_dir().join(format!("flamewm-mime-{}", std::process::id()));
        let apps = root.join("applications");
        std::fs::create_dir_all(&apps).expect("create fixture application directory");
        std::fs::write(apps.join("vlc.desktop"), "[Desktop Entry]\nType=Application\nName=VLC\nExec=vlc %U\nMimeType=video/mp4;\n")
            .expect("create fixture desktop file");
        let dirs = [root.clone()];
        let preferred = BTreeMap::from([
            ("video/mp4".to_owned(), "org.kde.haruna.desktop;vlc.desktop;".to_owned()),
        ]);
        let generated = mime_defaults(preferred, &dirs);
        assert!(generated.contains("video/mp4=vlc.desktop;"));
        assert!(!generated.contains("video/mp4=org.kde.haruna.desktop;"));
        std::fs::remove_dir_all(&root).expect("remove fixture");
    }

    #[test]
    fn brave_desktop_override_keeps_actions_and_existing_flags() {
        let desktop = "[Desktop Entry]\nExec=/usr/bin/brave-browser %U\n[Desktop Action new-private-window]\nExec=/usr/bin/brave-browser --incognito\n";
        let changed = brave_desktop_entry(desktop);
        assert!(changed.contains("Exec=env XDG_CURRENT_DESKTOP=KDE /usr/bin/brave-browser %U"));
        assert!(changed.contains("Exec=env XDG_CURRENT_DESKTOP=KDE /usr/bin/brave-browser --incognito"));
        assert_eq!(brave_desktop_entry("Exec=brave --password-store=kwallet6 %U\n"), "Exec=brave --password-store=kwallet6 %U\n");
    }
}
