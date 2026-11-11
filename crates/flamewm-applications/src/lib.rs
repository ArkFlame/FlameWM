mod browser_secrets;
mod directory_open;
pub mod launch_feedback;
mod opener;
pub mod terminal;
use browser_secrets::*;
pub use directory_open::{
    DirectoryHandler, DirectorySpawner, decide_directory_handler, open_directory, route_directory,
};
use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_api::{DesktopAppId, ErrorCode, FlameError, FlameResult};
use flamewm_session_core::{installed_kwallet_generations, resolve_kwallet_version};
#[cfg(test)]
use opener::*;
pub use opener::{open_target, preferred_file_opener};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod app_catalog;
mod desktop_entry;
mod exec_line;
mod launch;
pub use app_catalog::*;
pub use desktop_entry::*;
pub use exec_line::*;
pub use launch::*;

pub mod catalog {
    pub use super::{
        ApplicationCatalog, DesktopEntry, WindowApplicationIdentity, xdg_application_dirs,
    };
}

pub mod exec {
    pub use super::{expand_exec, tokenize_exec};
}

pub mod launcher {
    pub use super::launch_argv;
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod desktop_label_metadata_tests {
    use super::*;
    #[test]
    fn desktop_name_survives_an_unlaunchable_kde_only_shortcut() {
        let file = std::env::temp_dir().join(format!(
            "flamewm-name-{}-{}.desktop",
            std::process::id(),
            "kde"
        ));
        std::fs::write(&file, "[Desktop Entry]\nType=Application\nOnlyShowIn=KDE;\nName=Dolphin File Manager\nExec=does-not-exist\n").unwrap();
        assert_eq!(
            desktop_file_display_name(&file).as_deref(),
            Some("Dolphin File Manager")
        );
        std::fs::remove_file(file).unwrap();
    }
    #[test]
    fn ignores_desktop_actions_and_non_desktop_entry_names() {
        let file = std::env::temp_dir().join(format!(
            "flamewm-name-{}-{}.desktop",
            std::process::id(),
            "sections"
        ));
        std::fs::write(&file,"[Other]\nName=Wrong\n[Desktop Entry]\nName=Correct\n[Desktop Action More]\nName=Not the icon name\n").unwrap();
        assert_eq!(desktop_file_display_name(&file).as_deref(), Some("Correct"));
        std::fs::remove_file(file).unwrap();
    }
}

#[cfg(test)]
mod repeat_open_guard_tests {
    use super::*;
    use std::ffi::OsStr;
    use std::time::{Duration, Instant};

    #[test]
    fn identical_target_inside_window_is_a_repeat() {
        let start = Instant::now();
        let folder = OsStr::new("/home/u/Documents");
        let soon = start + Duration::from_millis(200);
        assert!(is_repeat_open(Some((folder, start)), folder, soon));
    }

    #[test]
    fn identical_target_after_window_or_different_target_is_allowed() {
        let start = Instant::now();
        let folder = OsStr::new("/home/u/Documents");
        let later = start + REPEAT_OPEN_WINDOW;
        assert!(!is_repeat_open(Some((folder, start)), folder, later));
        assert!(!is_repeat_open(
            Some((folder, start)),
            OsStr::new("/home/u/Music"),
            start
        ));
        assert!(!is_repeat_open(None, folder, start));
    }
}

#[cfg(test)]
mod native_mime_open_tests {
    use super::*;

    #[test]
    fn brave_launcher_detection_excludes_unrelated_browsers() {
        assert!(is_brave_program("/usr/bin/brave-browser-stable"));
        assert!(is_brave_program("brave-browser"));
        assert!(!is_brave_program("/usr/bin/chromium"));
        assert!(!is_brave_program("/opt/not-brave-browser"));
    }

    #[test]
    fn kde_default_provider_wins_without_overriding_mimeapps() {
        assert_eq!(
            preferred_file_opener(|name| name == "kioclient6").0,
            "kioclient6"
        );
        assert_eq!(
            preferred_file_opener(|name| name == "kioclient6").1,
            &["exec"]
        );
        assert_eq!(
            preferred_file_opener(|name| name == "kde-open6").0,
            "kde-open6"
        );
        assert_eq!(preferred_file_opener(|_| false).0, "xdg-open");
    }
}

#[cfg(test)]
mod browser_secret_tests {
    use super::{browser_secret_version, explicit_password_store};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn password_store_flag_is_parsed_in_both_forms() {
        assert_eq!(
            explicit_password_store(&args(&["brave", "--password-store=kwallet6"])),
            Some("kwallet6".to_owned())
        );
        assert_eq!(
            explicit_password_store(&args(&["brave", "--password-store", "kwallet5"])),
            Some("kwallet5".to_owned())
        );
        assert_eq!(explicit_password_store(&args(&["brave"])), None);
    }

    #[test]
    fn explicit_backend_is_preserved_and_never_forced() {
        assert_eq!(
            browser_secret_version(
                "brave-browser",
                &args(&["brave", "--password-store=kwallet6"])
            )
            .unwrap(),
            Some("6")
        );
        assert_eq!(
            browser_secret_version("brave-browser", &args(&["brave", "--password-store=basic"]))
                .unwrap(),
            None
        );
        assert_eq!(
            browser_secret_version(
                "brave-browser",
                &args(&["brave", "--password-store=gnome-libsecret"])
            )
            .unwrap(),
            None
        );
    }
}
