//! Directory opening is a typed `FlameWM` decision. Folders never reach the KDE/XDG
//! opener: the effective `inode/directory` default (user `mimeapps.list` first, then
//! the XDG config/data search order) selects `FlameWM` Explorer or an installed,
//! non-FlameWM desktop entry; any uncertainty selects `FlameWM` Explorer.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::{DesktopAppId, FlameResult};

use super::{ApplicationCatalog, spawn_with_startup, xdg_application_dirs};

pub const FLAMEWM_EXPLORER_ID: &str = "flamewm-explorer.desktop";
const DIRECTORY_MIME: &str = "inode/directory";

/// Resolved owner for a folder open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryHandler {
    FlameExplorer,
    /// An installed desktop entry that the user explicitly chose (not `FlameWM` Explorer).
    Desktop(String),
}

/// Side effects of a folder open, injectable for tests.
pub trait DirectorySpawner {
    fn spawn_explorer(&mut self, binary: &Path, directory: &Path) -> FlameResult<()>;
    fn launch_desktop(&mut self, id: &str, directory: &Path) -> FlameResult<()>;
}

/// `mimeapps.list` files in XDG precedence order: user config, config dirs, then
/// the `applications/` subdirectory of the data home and data dirs.
#[must_use]
pub fn mimeapps_search_paths() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let config_dirs = split_env("XDG_CONFIG_DIRS", "/etc/xdg");
    let data_dirs = split_env("XDG_DATA_DIRS", "/usr/local/share:/usr/share");
    config_home
        .into_iter()
        .chain(config_dirs)
        .map(|dir| dir.join("mimeapps.list"))
        .chain(
            data_home
                .into_iter()
                .chain(data_dirs)
                .map(|dir| dir.join("applications/mimeapps.list")),
        )
        .collect()
}

fn split_env(name: &str, fallback: &str) -> Vec<PathBuf> {
    let value: OsString = std::env::var_os(name).unwrap_or_else(|| fallback.into());
    std::env::split_paths(&value).collect()
}

/// First `[Default Applications]` entry for `mime` in one `mimeapps.list` text.
#[must_use]
pub fn default_in_mimeapps(content: &str, mime: &str) -> Option<String> {
    let mut in_defaults = false;
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            in_defaults = line == "[Default Applications]";
            continue;
        }
        if !in_defaults {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == mime {
                return value
                    .split(';')
                    .map(str::trim)
                    .find(|id| !id.is_empty())
                    .map(str::to_owned);
            }
        }
    }
    None
}

/// Pure decision: the first mimeapps file that names a directory default wins.
/// Missing, unknown or uninstalled defaults fall back to `FlameWM` Explorer.
#[must_use]
pub fn decide_directory_handler(
    mimeapps_texts: &[Option<String>],
    installed: impl Fn(&str) -> bool,
) -> DirectoryHandler {
    let chosen = mimeapps_texts
        .iter()
        .flatten()
        .find_map(|text| default_in_mimeapps(text, DIRECTORY_MIME));
    match chosen {
        Some(id) if id != FLAMEWM_EXPLORER_ID && installed(&id) => DirectoryHandler::Desktop(id),
        _ => DirectoryHandler::FlameExplorer,
    }
}

fn installed_desktop_entry(id: &str) -> bool {
    xdg_application_dirs()
        .iter()
        .any(|dir| dir.join("applications").join(id).is_file())
}

fn read_mimeapps() -> Vec<Option<String>> {
    mimeapps_search_paths()
        .iter()
        .map(|path| std::fs::read_to_string(path).ok())
        .collect()
}

/// Locate the `flamewm-explorer` binary next to the running desktop, then on PATH.
#[must_use]
pub fn explorer_binary() -> PathBuf {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("flamewm-explorer")));
    if let Some(sibling) = sibling.filter(|candidate| candidate.is_file()) {
        return sibling;
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join("flamewm-explorer"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("flamewm-explorer"))
}

/// Production spawner: explorer by binary path, other handlers by their desktop entry.
struct SystemSpawner;

impl DirectorySpawner for SystemSpawner {
    fn spawn_explorer(&mut self, binary: &Path, directory: &Path) -> FlameResult<()> {
        let mut command = Command::new(binary);
        command.arg(directory);
        spawn_with_startup(
            &mut command,
            "FlameWM Explorer",
            "system-file-manager",
            "flamewm-explorer",
            true,
        )
    }

    fn launch_desktop(&mut self, id: &str, directory: &Path) -> FlameResult<()> {
        let catalog = ApplicationCatalog::discover_file_handlers()?;
        let mut options = ApplicationLaunchOptions::default();
        options
            .extra_args
            .push(directory.to_string_lossy().into_owned());
        catalog.launch(&DesktopAppId::new(id), &options)
    }
}

/// Open a folder through `FlameWM`'s own handler resolution. Never calls kde-open/xdg-open.
pub fn open_directory(directory: &Path) -> FlameResult<()> {
    let handler = decide_directory_handler(&read_mimeapps(), installed_desktop_entry);
    route_directory(&handler, directory, &explorer_binary(), &mut SystemSpawner)
}

/// Dispatch a resolved handler through a spawner; the testable core of `open_directory`.
pub fn route_directory(
    handler: &DirectoryHandler,
    directory: &Path,
    explorer: &Path,
    spawner: &mut dyn DirectorySpawner,
) -> FlameResult<()> {
    match handler {
        DirectoryHandler::FlameExplorer => spawner.spawn_explorer(explorer, directory),
        DirectoryHandler::Desktop(id) => spawner.launch_desktop(id, directory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
    }

    impl DirectorySpawner for Recorder {
        fn spawn_explorer(&mut self, binary: &Path, directory: &Path) -> FlameResult<()> {
            self.calls.push(format!(
                "explorer {} {}",
                binary.display(),
                directory.display()
            ));
            Ok(())
        }
        fn launch_desktop(&mut self, id: &str, directory: &Path) -> FlameResult<()> {
            self.calls
                .push(format!("desktop {id} {}", directory.display()));
            Ok(())
        }
    }

    const DOLPHIN_DEFAULT: &str =
        "[Default Applications]\ninode/directory=org.kde.dolphin.desktop;\n";
    const EXPLORER_DEFAULT: &str =
        "[Default Applications]\ninode/directory=flamewm-explorer.desktop\n";
    const DOLPHIN_INSTALLED: fn(&str) -> bool = |id| id == "org.kde.dolphin.desktop";

    #[test]
    fn directory_without_mimeapps_spawns_flamewm_explorer_with_path() {
        let mut recorder = Recorder::default();
        let handler = decide_directory_handler(&[None, None], DOLPHIN_INSTALLED);
        route_directory(
            &handler,
            Path::new("/home/u/New Folder"),
            Path::new("/bin/flamewm-explorer"),
            &mut recorder,
        )
        .unwrap();
        assert_eq!(
            recorder.calls,
            ["explorer /bin/flamewm-explorer /home/u/New Folder"]
        );
    }

    #[test]
    fn dolphin_system_default_does_not_win_over_user_flamewm_explorer_choice() {
        let mut recorder = Recorder::default();
        let texts = [
            Some(EXPLORER_DEFAULT.to_owned()),
            Some(DOLPHIN_DEFAULT.to_owned()),
        ];
        let handler = decide_directory_handler(&texts, DOLPHIN_INSTALLED);
        assert_eq!(handler, DirectoryHandler::FlameExplorer);
        route_directory(
            &handler,
            Path::new("/tmp/d"),
            Path::new("flamewm-explorer"),
            &mut recorder,
        )
        .unwrap();
        assert_eq!(recorder.calls, ["explorer flamewm-explorer /tmp/d"]);
    }

    #[test]
    fn explicit_installed_desktop_choice_is_honoured_without_kde_opener() {
        let handler =
            decide_directory_handler(&[Some(DOLPHIN_DEFAULT.to_owned())], DOLPHIN_INSTALLED);
        assert_eq!(
            handler,
            DirectoryHandler::Desktop("org.kde.dolphin.desktop".into())
        );
    }

    #[test]
    fn uninstalled_or_unknown_default_falls_back_to_flamewm_explorer() {
        let handler = decide_directory_handler(&[Some(DOLPHIN_DEFAULT.to_owned())], |_| false);
        assert_eq!(handler, DirectoryHandler::FlameExplorer);
    }

    #[test]
    fn default_parser_reads_only_default_applications_section() {
        let text = "[Added Associations]\ninode/directory=a.desktop;\n[Default Applications]\ninode/directory=b.desktop;c.desktop;\n";
        assert_eq!(
            default_in_mimeapps(text, "inode/directory").as_deref(),
            Some("b.desktop")
        );
        assert_eq!(
            default_in_mimeapps(
                "[Added Associations]\ninode/directory=a.desktop;\n",
                "inode/directory"
            ),
            None
        );
    }
}
