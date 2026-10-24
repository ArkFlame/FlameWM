//! Session-scoped KDE application compatibility without impersonating a Plasma session.
//!
//! KDE as a global XDG desktop identity allows KDE-only display managers to
//! activate and race FlameWM's own XRandR authority. Keep the session FlameWM,
//! borrow only MIME defaults and Brave's wallet backend on application launch.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn installed(executable: &Path) -> bool {
    executable
        .metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn atomic_owned_file(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid compatibility destination")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let outcome = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temp, path));
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
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
                    defaults
                        .entry(mime.to_owned())
                        .or_insert_with(|| apps.to_owned());
                }
            }
        }
    }
    defaults
}

fn associations_from_kde(text: &str) -> BTreeMap<(String, String), Vec<String>> {
    let mut section = "";
    let mut associations = BTreeMap::<(String, String), Vec<String>>::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = match line {
                "[Added Associations]" | "[Removed Associations]" => line,
                _ => "",
            };
            continue;
        }
        if section.is_empty() || line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((mime, ids)) = line.split_once('=') else {
            continue;
        };
        let mime = mime.trim();
        if !mime.contains('/') || mime.contains(char::is_whitespace) {
            continue;
        }
        let apps = associations
            .entry((section.to_owned(), mime.to_owned()))
            .or_default();
        for id in ids
            .split(';')
            .map(str::trim)
            .filter(|id| id.ends_with(".desktop") && !id.contains(char::is_whitespace))
        {
            if !apps.iter().any(|old| old == id) {
                apps.push(id.to_owned());
            }
        }
    }
    associations
}

/// KDE desktop-specific defaults can live in user config, site config or
/// distribution application data. Higher-priority entries win, exactly as in
/// the MIME Apps lookup order; never copy host KDE settings into the user file.
fn kde_mime_defaults(
    config_home: &Path,
    data_home: &Path,
    config_dirs: &[PathBuf],
    data_dirs: &[PathBuf],
) -> BTreeMap<String, String> {
    let paths = std::iter::once(config_home.join("kde-mimeapps.list"))
        .chain(config_dirs.iter().map(|dir| dir.join("kde-mimeapps.list")))
        .chain(std::iter::once(
            data_home.join("applications/kde-mimeapps.list"),
        ))
        .chain(
            data_dirs
                .iter()
                .map(|dir| dir.join("applications/kde-mimeapps.list")),
        );
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

fn kde_mime_associations(
    config_home: &Path,
    data_home: &Path,
    config_dirs: &[PathBuf],
    data_dirs: &[PathBuf],
) -> BTreeMap<(String, String), Vec<String>> {
    let paths = std::iter::once(config_home.join("kde-mimeapps.list"))
        .chain(config_dirs.iter().map(|dir| dir.join("kde-mimeapps.list")))
        .chain(std::iter::once(
            data_home.join("applications/kde-mimeapps.list"),
        ))
        .chain(
            data_dirs
                .iter()
                .map(|dir| dir.join("applications/kde-mimeapps.list")),
        );
    let mut merged = BTreeMap::<(String, String), Vec<String>>::new();
    for path in paths {
        if let Ok(contents) = fs::read_to_string(path) {
            if contents.len() <= 1_048_576 {
                for (key, ids) in associations_from_kde(&contents) {
                    let existing = merged.entry(key).or_default();
                    for id in ids {
                        if existing.contains(&id) {
                            continue;
                        }
                        // KDE's first file wins ordering; lower-priority IDs remain
                        // available in Open With without changing the default.
                        existing.push(id);
                    }
                }
            }
        }
    }
    merged
}

fn user_removed_associations(
    config_home: &Path,
    config_dirs: &[PathBuf],
    data_home: &Path,
    data_dirs: &[PathBuf],
) -> BTreeMap<String, Vec<String>> {
    let paths = std::iter::once(config_home.join("mimeapps.list"))
        .chain(config_dirs.iter().map(|dir| dir.join("mimeapps.list")))
        .chain(std::iter::once(
            data_home.join("applications/mimeapps.list"),
        ))
        .chain(
            data_dirs
                .iter()
                .map(|dir| dir.join("applications/mimeapps.list")),
        );
    let mut removed = BTreeMap::<String, Vec<String>>::new();
    for path in paths {
        if let Ok(contents) = fs::read_to_string(path) {
            if contents.len() <= 1_048_576 {
                for ((section, mime), ids) in associations_from_kde(&contents) {
                    if section != "[Removed Associations]" {
                        continue;
                    }
                    let current = removed.entry(mime).or_default();
                    for id in ids {
                        if !current.contains(&id) {
                            current.push(id);
                        }
                    }
                }
            }
        }
    }
    removed
}

fn desktop_id_installed(data_dirs: &[PathBuf], desktop_id: &str) -> bool {
    data_dirs
        .iter()
        .any(|dir| dir.join("applications").join(desktop_id).is_file())
}

fn mime_defaults(mut defaults: BTreeMap<String, String>, data_dirs: &[PathBuf]) -> String {
    defaults.retain(|_, apps| {
        let installed = apps
            .split(';')
            .filter(|id| !id.is_empty() && desktop_id_installed(data_dirs, id))
            .collect::<Vec<_>>();
        if installed.is_empty() {
            return false;
        }
        *apps = format!("{};", installed.join(";"));
        true
    });
    for (mime, candidates) in [
        ("text/plain", &["org.kde.kate.desktop", "kate.desktop"][..]),
        (
            "video/mp4",
            &[
                "org.kde.haruna.desktop",
                "org.kde.dragonplayer.desktop",
                "vlc.desktop",
                "mpv.desktop",
            ][..],
        ),
    ] {
        // KDE can retain defaults for applications that were uninstalled.
        // Keep its preference order, but never shadow an installed fallback
        // with a stale entry that sends Dolphin to Open With.
        let installed = defaults
            .get(mime)
            .map(|apps| {
                apps.split(';')
                    .filter(|id| !id.is_empty() && desktop_id_installed(data_dirs, id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !installed.is_empty() {
            defaults.insert(mime.to_owned(), format!("{};", installed.join(";")));
        } else if let Some(id) = candidates
            .iter()
            .find(|id| desktop_id_installed(data_dirs, id))
        {
            defaults.insert(mime.to_owned(), format!("{id};"));
        } else {
            defaults.remove(mime);
        }
    }
    // Explorer is the session default; an explicit user mimeapps.list choice still wins.
    if desktop_id_installed(data_dirs, "flamewm-explorer.desktop") {
        defaults.insert("inode/directory".to_owned(), "flamewm-explorer.desktop;".to_owned());
    }
    let mut text = String::from(
        "# Generated fallback. User ~/.config/mimeapps.list always has precedence.\n[Default Applications]\n",
    );
    for (mime, apps) in defaults {
        text.push_str(&format!("{mime}={apps}\n"));
    }
    text
}

fn mime_associations(
    associations: BTreeMap<(String, String), Vec<String>>,
    data_dirs: &[PathBuf],
    protected_removals: &BTreeMap<String, Vec<String>>,
) -> String {
    let mut removed = protected_removals.clone();
    for ((section, mime), ids) in &associations {
        if section != "[Removed Associations]" {
            continue;
        }
        let current = removed.entry(mime.clone()).or_default();
        for id in ids {
            if !current.contains(id) {
                current.push(id.clone());
            }
        }
    }
    let mut text = String::new();
    for section in ["[Added Associations]", "[Removed Associations]"] {
        let entries = associations
            .iter()
            .filter(|((kind, _), _)| kind == section)
            .collect::<Vec<_>>();
        if entries.is_empty() {
            continue;
        }
        text.push_str(section);
        text.push('\n');
        for ((_, mime), ids) in entries {
            let ids = ids
                .iter()
                .filter(|id| {
                    section == "[Removed Associations]"
                        || (!removed.get(mime).is_some_and(|list| list.contains(id))
                            && desktop_id_installed(data_dirs, id))
                })
                .cloned()
                .collect::<Vec<_>>();
            if !ids.is_empty() {
                text.push_str(mime);
                text.push('=');
                text.push_str(&ids.join(";"));
                text.push_str(";\n");
            }
        }
    }
    text
}

const BRAVE_DESKTOP_IDS: [&str; 3] = [
    "brave-browser.desktop",
    "brave-browser-stable.desktop",
    "com.brave.Browser.desktop",
];
const BRAVE_OVERLAY_MARKER: &str =
    "# Generated by FlameWM session compatibility; original Brave entries remain untouched.";

/// Apply the SAME unambiguous KDE generation to every unqualified Brave
/// desktop action. Field codes, profile selection, and installed Exec flags
/// remain byte-for-byte intact. Chromium's own explicit backend wins.
fn brave_desktop_entry(original: &str, version: &str) -> String {
    let mut result = format!("{BRAVE_OVERLAY_MARKER}\n");
    for line in original.lines() {
        if let Some(exec) = line.strip_prefix("Exec=") {
            if !exec.contains("XDG_CURRENT_DESKTOP=") && !exec.contains("--password-store") {
                result.push_str("Exec=env XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=");
                result.push_str(version);
                result.push(' ');
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

/// Remove stale Pass 28-30 entries only within our dedicated XDG overlay.
/// Existing user/system Brave launchers and unrelated overlay files are never touched.
fn remove_owned_brave_override(path: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            return Err("Brave overlay is not a regular file; preserving it".to_owned());
        }
    }
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let legacy = content
        .lines()
        .any(|line| line.starts_with("Exec=env XDG_CURRENT_DESKTOP=KDE "));
    if content.starts_with(BRAVE_OVERLAY_MARKER) || legacy {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn install_applications_menu(config_dir: &Path) -> Result<(), String> {
    atomic_owned_file(
        &config_dir.join("menus/flamewm-applications.menu"),
        include_str!("../../../packaging/menus/flamewm-applications.menu"),
    )
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
    let config_home = first
        .environment
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| home.join(".config"));
    let data_home = first
        .environment
        .get("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| home.join(".local/share"));
    let existing_data_dirs = first
        .environment
        .get("XDG_DATA_DIRS")
        .cloned()
        .or_else(|| {
            std::env::var("XDG_DATA_DIRS")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let data_dirs = std::iter::once(data_home.clone())
        .chain(std::env::split_paths(&existing_data_dirs))
        .collect::<Vec<_>>();
    let existing_config_dirs = first
        .environment
        .get("XDG_CONFIG_DIRS")
        .cloned()
        .or_else(|| {
            std::env::var("XDG_CONFIG_DIRS")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "/etc/xdg".into());
    let config_dirs = std::env::split_paths(&existing_config_dirs).collect::<Vec<_>>();
    let owned = config_home.join("flamewm/session-compat");
    let config_dir = owned.join("config");
    let data_dir = owned.join("data");
    // A source-checkout FlameWM has no /etc/xdg installation. Publish the
    // same standards-compliant menu into the session-owned config search root
    // before any WM or KDE application starts. Never modify user menus.
    install_applications_menu(&config_dir)?;
    // Autostart and activated applications need both overlay directories even
    // when no optional Brave desktop override is generated.
    fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;

    let kde = kde_mime_defaults(&config_home, &data_home, &config_dirs, &data_dirs);
    atomic_owned_file(
        &config_dir.join("flamewm-mimeapps.list"),
        &mime_defaults(kde, &data_dirs),
    )?;
    let associations = kde_mime_associations(&config_home, &data_home, &config_dirs, &data_dirs);
    let removed = user_removed_associations(&config_home, &config_dirs, &data_home, &data_dirs);
    atomic_owned_file(
        &config_dir.join("mimeapps.list"),
        &mime_associations(associations, &data_dirs, &removed),
    )?;

    // Regenerate only from original XDG sources, never our own previous copy.
    // A wallet-generation hint is not proof of the user's original encryption
    // backend. Existing browser flags remain authoritative over auto-detection.
    let wallet = first
        .environment
        .get("KDE_SESSION_VERSION")
        .map(String::as_str);
    for id in BRAVE_DESKTOP_IDS {
        let destination = data_dir.join("applications").join(id);
        if let Err(error) = remove_owned_brave_override(&destination) {
            eprintln!("flamewm-session: old Brave overlay {id} cleanup unavailable: {error}");
            continue;
        }
        if let Some(version @ ("5" | "6")) = wallet {
            if let Some(contents) = data_dirs
                .iter()
                .filter(|dir| *dir != &data_dir)
                .map(|dir| dir.join("applications").join(id))
                .find_map(|path| fs::read_to_string(path).ok())
            {
                if contents.len() <= 1_048_576 && contents.contains("[Desktop Entry]") {
                    if let Err(error) =
                        atomic_owned_file(&destination, &brave_desktop_entry(&contents, version))
                    {
                        eprintln!("flamewm-session: Brave overlay {id} unavailable: {error}");
                    }
                }
            }
        }
    }
    for plan in processes {
        let config = plan
            .environment
            .get("XDG_CONFIG_DIRS")
            .cloned()
            .unwrap_or_else(|| existing_config_dirs.clone());
        plan.environment.insert(
            "XDG_CONFIG_DIRS".into(),
            format!("{}:{config}", config_dir.display()),
        );
        let data = plan
            .environment
            .get("XDG_DATA_DIRS")
            .cloned()
            .unwrap_or_else(|| existing_data_dirs.clone());
        plan.environment.insert(
            "XDG_DATA_DIRS".into(),
            format!("{}:{data}", data_dir.display()),
        );
    }
    Ok(())
}

/// KDE's PAM authentication module holds the login credential; this helper
/// only completes an already-pending, PAM-owned Unix-socket handshake.
/// Never start a new wallet daemon or attempt a password prompt.
fn pam_socket_ready(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
}

pub fn finalize_pam_kwallet_login(environment: &BTreeMap<String, String>) {
    static PAM_INIT: std::sync::Once = std::sync::Once::new();
    let Some(socket) = std::env::var_os("PAM_KWALLET5_LOGIN")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("PAM_KWALLET_LOGIN").filter(|value| !value.is_empty()))
    else {
        // Passwordless login, missing PAM hook, or a session where PAM already
        // completed its handshake: no credential is available to synthesize.
        if environment.contains_key("KDE_SESSION_VERSION") {
            eprintln!(
                "flamewm-session: no PAM KWallet handoff supplied; wallet unlock status unverified"
            );
        }
        return;
    };
    if !pam_socket_ready(Path::new(&socket)) {
        eprintln!(
            "flamewm-session: PAM KWallet handoff socket unavailable; automatic unlock unverified"
        );
        return;
    }
    PAM_INIT.call_once(|| {
        let helper = [
            "/usr/lib/x86_64-linux-gnu/libexec/pam_kwallet_init",
            "/usr/lib/pam_kwallet_init", "/usr/libexec/pam_kwallet_init",
            "/usr/lib/x86_64-linux-gnu/pam_kwallet_init",
        ]
        .into_iter()
        .find(|path| installed(Path::new(path)));
        let Some(path) = helper else {
            eprintln!("flamewm-session: pam_kwallet_init missing; automatic unlock unavailable");
            return;
        };
        let Ok(mut child) = Command::new(path).envs(environment).spawn() else {
            eprintln!("flamewm-session: unable to run PAM KWallet handoff helper");
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return,
                Ok(Some(_)) => {
                    eprintln!("flamewm-session: PAM KWallet handoff failed; wallet may require manual unlock");
                    return;
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    eprintln!("flamewm-session: PAM KWallet handoff failed; wallet may require manual unlock");
                    return;
                }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    eprintln!("flamewm-session: PAM KWallet handoff timed out; wallet may require manual unlock");
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kde_associations_preserved_and_common_defaults_only_fill_missing_types() {
        let kde = "[Added Associations]\ntext/plain=bad.desktop;org.kde.kate.desktop;\n[Removed Associations]\ntext/plain=user-hidden.desktop;\n[Default Applications]\ntext/plain=org.kde.kate.desktop;\nvideo/mp4=org.kde.haruna.desktop;\n";
        assert_eq!(
            defaults_from_kde(kde).get("text/plain").map(String::as_str),
            Some("org.kde.kate.desktop;")
        );
        assert_eq!(
            associations_from_kde(kde)
                .get(&("[Removed Associations]".to_owned(), "text/plain".to_owned()))
                .unwrap(),
            &["user-hidden.desktop"]
        );
        let text = mime_defaults(defaults_from_kde(kde), &[]);
        assert!(!text.contains("text/plain="));
        assert!(!text.contains("video/mp4="));
    }
    #[test]
    fn generated_generic_associations_add_installed_handlers_and_keep_removals() {
        let root =
            std::env::temp_dir().join(format!("flamewm-associations-{}", std::process::id()));
        let apps = root.join("applications");
        std::fs::create_dir_all(&apps).expect("create fixture application directory");
        std::fs::write(
            apps.join("kate.desktop"),
            "[Desktop Entry]\nType=Application\nName=Kate\nExec=kate %U\n",
        )
        .unwrap();
        let associations = associations_from_kde(
            "[Added Associations]\ntext/plain=stale.desktop;kate.desktop;\n[Removed Associations]\ntext/plain=user-hidden.desktop;\n",
        );
        let generated = mime_associations(associations.clone(), &[root.clone()], &BTreeMap::new());
        assert!(generated.contains("[Added Associations]\ntext/plain=kate.desktop;\n"));
        assert!(generated.contains("[Removed Associations]\ntext/plain=user-hidden.desktop;\n"));
        assert!(!generated.contains("stale.desktop"));
        let protected =
            BTreeMap::from([("text/plain".to_owned(), vec!["kate.desktop".to_owned()])]);
        let generated = mime_associations(associations, &[root.clone()], &protected);
        assert!(!generated.contains("text/plain=kate.desktop;"));
        assert!(generated.contains("[Removed Associations]\ntext/plain=user-hidden.desktop;\n"));
        assert!(!generated.contains("stale.desktop"));
        std::fs::remove_dir_all(&root).expect("remove fixture");
    }
    #[test]
    fn missing_kde_video_player_falls_back_to_an_installed_handler() {
        let root = std::env::temp_dir().join(format!("flamewm-mime-{}", std::process::id()));
        let apps = root.join("applications");
        std::fs::create_dir_all(&apps).expect("create fixture application directory");
        std::fs::write(
            apps.join("vlc.desktop"),
            "[Desktop Entry]\nType=Application\nName=VLC\nExec=vlc %U\nMimeType=video/mp4;\n",
        )
        .expect("create fixture desktop file");
        let dirs = [root.clone()];
        let preferred = BTreeMap::from([(
            "video/mp4".to_owned(),
            "org.kde.haruna.desktop;vlc.desktop;".to_owned(),
        )]);
        let generated = mime_defaults(preferred, &dirs);
        assert!(generated.contains("video/mp4=vlc.desktop;"));
        assert!(!generated.contains("video/mp4=org.kde.haruna.desktop;"));
        std::fs::remove_dir_all(&root).expect("remove fixture");
    }

    #[test]
    fn brave_desktop_override_keeps_actions_and_existing_flags() {
        let desktop = "[Desktop Entry]\nExec=/usr/bin/brave-browser %U\n[Desktop Action new-private-window]\nExec=/usr/bin/brave-browser --incognito\n";
        let changed = brave_desktop_entry(desktop, "6");
        assert!(changed.contains(
            "Exec=env XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=6 /usr/bin/brave-browser %U"
        ));
        assert!(changed.contains("Exec=env XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=6 /usr/bin/brave-browser --incognito"));
        let explicit = brave_desktop_entry(
            "Exec=brave --password-store=kwallet5 --user-data-dir=/profile %U\n",
            "6",
        );
        assert!(
            explicit.contains("Exec=brave --password-store=kwallet5 --user-data-dir=/profile %U")
        );
        assert!(!explicit.contains("Exec=env"));
        let separate = brave_desktop_entry("Exec=brave --password-store kwallet5 %U\n", "6");
        assert!(separate.contains("Exec=brave --password-store kwallet5 %U"));
        let user_desktop =
            brave_desktop_entry("Exec=env XDG_CURRENT_DESKTOP=GNOME brave %U\n", "6");
        assert!(user_desktop.contains("Exec=env XDG_CURRENT_DESKTOP=GNOME brave %U"));
    }

    #[test]
    fn missing_pam_socket_does_not_initiate_wallet_handshake() {
        let path = std::env::temp_dir().join(format!("flamewm-missing-pam-{}", std::process::id()));
        assert!(!pam_socket_ready(&path));
        let root = std::env::temp_dir().join(format!("flamewm-pam-socket-{}", std::process::id()));
        let _ = fs::remove_file(&root);
        let listener = std::os::unix::net::UnixListener::bind(&root).expect("disposable socket");
        assert!(pam_socket_ready(&root));
        drop(listener);
        fs::remove_file(&root).expect("remove disposable socket");
    }

    #[test]
    fn only_generated_brave_entries_are_removed() {
        let root = std::env::temp_dir().join(format!("flamewm-owned-brave-{}", std::process::id()));
        let _ = fs::remove_file(&root);
        fs::write(
            &root,
            "[Desktop Entry]\nExec=brave --profile-directory=Default\n",
        )
        .expect("write fixture");
        remove_owned_brave_override(&root).expect("preserve non-generated file");
        assert!(root.exists());
        fs::write(
            &root,
            brave_desktop_entry("[Desktop Entry]\nExec=brave %U\n", "5"),
        )
        .expect("write generated fixture");
        remove_owned_brave_override(&root).expect("remove generated file");
        assert!(!root.exists());
        fs::write(
            &root,
            "[Desktop Entry]\nExec=env XDG_CURRENT_DESKTOP=KDE brave %U\n",
        )
        .expect("legacy fixture");
        remove_owned_brave_override(&root).expect("remove legacy file");
        assert!(!root.exists());
    }
}
