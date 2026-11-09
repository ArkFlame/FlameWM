//! Per-theme coverage of the 40 `freedesktop` names `FlameWM` asks for. Installed
//! themes only: an absent theme is skipped, never passed. Misses are reported,
//! not failed, except for the default theme, which must cover most names.

use super::*;

/// Names from `docs/ICON_CURSOR_THEMES_SPEC.md` section 6.1.
const MATRIX_NAMES: [&str; 40] = [
    "preferences-system",
    "folder",
    "user-trash",
    "utilities-terminal",
    "system-file-manager",
    "accessories-text-editor",
    "document-open",
    "edit-copy",
    "edit-paste",
    "document-save",
    "go-home",
    "view-refresh",
    "battery-full",
    "network-wireless",
    "audio-volume-high",
    "application-x-executable",
    "text-x-generic",
    "image-x-generic",
    "video-x-generic",
    "audio-x-generic",
    "application-pdf",
    "firefox",
    "dialog-error",
    "dialog-information",
    "dialog-warning",
    "emblem-default",
    "folder-open",
    "go-up",
    "list-add",
    "list-remove",
    "media-playback-start",
    "media-skip-forward",
    "preferences-desktop",
    "system-search",
    "system-shutdown",
    "user-home",
    "utilities-system-monitor",
    "web-browser",
    "window-close",
    "zoom-in",
];

const MATRIX_THEMES: [&str; 3] = ["breeze", "Adwaita", "MacTahoe"];

fn installed(theme: &str) -> bool {
    Path::new("/usr/share/icons")
        .join(theme)
        .join("index.theme")
        .is_file()
}

#[test]
fn installed_themes_resolve_the_matrix_names() {
    let mut checked = 0;
    for theme in MATRIX_THEMES {
        if !installed(theme) {
            eprintln!("SKIP theme={theme} (not installed under /usr/share/icons)");
            continue;
        }
        checked += 1;
        let mut resolver =
            IconResolver::from_environment_with_theme(std::env::temp_dir(), 0, Some(theme));
        let size = IconSize {
            logical: 24,
            physical: 24,
        };
        let mut misses = Vec::new();
        for name in MATRIX_NAMES {
            if resolver.prepare_name(name, size).is_err() {
                misses.push(name);
            }
        }
        eprintln!(
            "MATRIX theme={theme} hits={} misses={} {misses:?}",
            MATRIX_NAMES.len() - misses.len(),
            misses.len()
        );
        if theme == DEFAULT_ICON_THEME {
            assert!(
                misses.len() <= 10,
                "default theme {theme} misses too many names: {misses:?}"
            );
        }
    }
    if checked == 0 {
        eprintln!("SKIP all matrix themes absent");
    }
}
