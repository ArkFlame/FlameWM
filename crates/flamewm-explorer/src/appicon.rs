//! Trusted application icon for a `.desktop` row (A06).
//!
//! A `.desktop` file is data, never code: this module reads only the `[Desktop Entry]` group and
//! picks out `Name`/`Icon`/`Hidden`/`Type`, then resolves `Icon` to one icon file. Nothing is
//! executed, `Exec` is ignored, and every path candidate is a bounded, non-recursive probe.
//!
//! Resolution runs on the thumbnail worker (see `thumbs::render_thumbnail`); the UI thread only
//! reads the resulting raster from the cache, and falls back to the embedded file icon.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Larger files are not read as desktop entries.
pub const MAX_DESKTOP_BYTES: u64 = 256 * 1024;
/// Longest `Icon=` value accepted.
const MAX_ICON_CHARS: usize = 200;
/// Icon themes probed under every icon root, in preference order.
pub const ICON_THEMES: [&str; 4] = ["hicolor", "breeze", "breeze-dark", "Adwaita"];
/// Size buckets probed inside a theme.
pub const ICON_SIZES: [&str; 5] = ["48x48", "64x64", "128x128", "256x256", "scalable"];
/// Extensions probed for a named icon; `xpm` is resolved but the decoder list decides readability.
pub const ICON_EXTENSIONS: [&str; 4] = ["png", "svg", "ico", "xpm"];
/// Upper bound on candidate paths per lookup, so a hostile `Icon=` cannot turn into a file scan.
pub const MAX_ICON_CANDIDATES: usize = 512;

/// The metadata an icon needs from a desktop entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launcher {
    pub name: Option<String>,
    pub icon: Option<String>,
}

fn clean(value: &str) -> Option<String> {
    let value = value.trim();
    let usable = !value.is_empty() && !value.chars().any(char::is_control);
    usable.then(|| value.chars().take(MAX_ICON_CHARS).collect())
}

/// Parses the `[Desktop Entry]` group. Returns `None` for a hidden entry, a non-application entry,
/// or one that declares neither a name nor an icon: such a file has no icon to show.
#[must_use]
pub fn parse_desktop_entry(text: &str) -> Option<Launcher> {
    let mut launcher = Launcher::default();
    let mut in_entry = false;
    let mut hidden = false;
    let mut entry_type: Option<String> = None;
    for line in text.lines().take(2048) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_entry = line.trim_end() == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" if launcher.name.is_none() => launcher.name = clean(value),
            "Icon" if launcher.icon.is_none() => launcher.icon = clean(value),
            "Hidden" => hidden = value.trim().eq_ignore_ascii_case("true"),
            "Type" => entry_type = clean(value),
            _ => {}
        }
    }
    if hidden || entry_type.is_some_and(|kind| !kind.eq_ignore_ascii_case("Application")) {
        return None;
    }
    (launcher.name.is_some() || launcher.icon.is_some()).then_some(launcher)
}

/// Icon roots, in preference order: user data, legacy per-user icons, then system roots.
pub fn icon_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut dirs = Vec::with_capacity(6);
    match env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(data) => dirs.push(data.join("icons")),
        None => {
            if let Some(home) = &home {
                dirs.push(home.join(".local/share/icons"));
            }
        }
    }
    if let Some(home) = home {
        dirs.push(home.join(".icons"));
    }
    for root in [
        "/usr/share/icons",
        "/usr/local/share/icons",
        "/usr/share/pixmaps",
    ] {
        dirs.push(PathBuf::from(root));
    }
    dirs
}

/// Paths to probe for an `Icon=` value. An absolute path is the only form accepted as a path:
/// relative or `..`-bearing names are refused so a desktop file cannot point outside an icon root.
#[must_use]
pub fn icon_candidates(icon: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    let icon = icon.trim();
    if icon.is_empty() || icon.chars().any(char::is_control) {
        return Vec::new();
    }
    if Path::new(icon).is_absolute() {
        return vec![PathBuf::from(icon)];
    }
    if icon.contains('/') || icon.contains("..") {
        return Vec::new();
    }
    let mut candidates = Vec::with_capacity(48);
    for dir in dirs {
        // Flat roots (pixmaps) hold `<name>.<ext>` directly.
        for extension in ICON_EXTENSIONS {
            candidates.push(dir.join(format!("{icon}.{extension}")));
        }
        for theme in ICON_THEMES {
            for size in ICON_SIZES {
                for extension in ICON_EXTENSIONS {
                    candidates.push(
                        dir.join(theme)
                            .join(size)
                            .join("apps")
                            .join(format!("{icon}.{extension}")),
                    );
                }
            }
        }
        if candidates.len() >= MAX_ICON_CANDIDATES {
            break;
        }
    }
    candidates.truncate(MAX_ICON_CANDIDATES);
    candidates
}

/// First candidate that exists as a regular file.
#[must_use]
pub fn resolve_icon(icon: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    icon_candidates(icon, dirs)
        .into_iter()
        .find(|path| path.is_file())
}

/// Icon file named by a `.desktop` entry, read without executing it. `dirs` is injected so the
/// lookup is testable without touching the real XDG roots.
#[must_use]
pub fn icon_file_in(desktop: &Path, dirs: &[PathBuf]) -> Option<PathBuf> {
    let meta = fs::metadata(desktop).ok()?;
    if !meta.is_file() || meta.len() > MAX_DESKTOP_BYTES {
        return None;
    }
    let bytes = fs::read(desktop).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let launcher = parse_desktop_entry(&text)?;
    resolve_icon(launcher.icon.as_deref()?, dirs)
}

/// Icon file named by a `.desktop` entry, using the real icon roots.
#[must_use]
pub fn icon_file(desktop: &Path) -> Option<PathBuf> {
    icon_file_in(desktop, &icon_dirs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let root = env::temp_dir().join(format!(
            "flamewm-desktop-icon-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn desktop_entry_parse_reads_only_the_application_group() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Text Editor
Icon=org.example.Editor
Exec=/usr/bin/editor %F
[Desktop Action new]
Name=New Document
Icon=should-not-be-used
";
        let launcher = parse_desktop_entry(text).expect("parses");
        assert_eq!(launcher.name.as_deref(), Some("Text Editor"));
        assert_eq!(launcher.icon.as_deref(), Some("org.example.Editor"));
    }

    #[test]
    fn hidden_and_non_application_entries_claim_no_icon() {
        assert!(
            parse_desktop_entry("[Desktop Entry]\nType=Application\nHidden=true\nName=X\nIcon=x\n")
                .is_none()
        );
        assert!(
            parse_desktop_entry("[Desktop Entry]\nType=Link\nURL=https://example.com\nIcon=x\n")
                .is_none()
        );
        assert!(
            parse_desktop_entry("[Desktop Entry]\nName=X\n").is_some(),
            "a name is enough to be an app row"
        );
        assert!(parse_desktop_entry("not a desktop entry\nIcon=x\n").is_none());
        // Keys before the group header are not part of the entry.
        assert_eq!(
            parse_desktop_entry("Icon=outside\n[Desktop Entry]\nName=Later\n")
                .unwrap()
                .icon,
            None
        );
    }

    #[test]
    fn icon_paths_are_resolved_inside_the_hinted_roots_only() {
        let dirs = vec![
            PathBuf::from("/usr/share/icons"),
            PathBuf::from("/usr/share/pixmaps"),
        ];
        let candidates = icon_candidates("myapp", &dirs);
        assert!(candidates.contains(&PathBuf::from("/usr/share/pixmaps/myapp.png")));
        assert!(candidates.contains(&PathBuf::from(
            "/usr/share/icons/hicolor/48x48/apps/myapp.svg"
        )));
        assert!(candidates.len() <= MAX_ICON_CANDIDATES);
        // Traversal and relative names resolve to nothing at all.
        assert!(icon_candidates("../secret", &dirs).is_empty());
        assert!(icon_candidates("sub/dir/icon", &dirs).is_empty());
        assert!(icon_candidates("", &dirs).is_empty());
        assert_eq!(
            icon_candidates("/opt/app/icon.png", &dirs),
            vec![PathBuf::from("/opt/app/icon.png")]
        );
    }

    #[test]
    fn desktop_file_resolves_to_an_installed_icon() {
        let root = temp_root("resolve");
        let icons = root.join("icons");
        let apps = icons.join("hicolor/48x48/apps");
        fs::create_dir_all(&apps).unwrap();
        fs::write(apps.join("myapp.png"), b"png").unwrap();
        let desktop = root.join("myapp.desktop");
        fs::write(
            &desktop,
            "[Desktop Entry]\nType=Application\nName=My App\nIcon=myapp\n",
        )
        .unwrap();
        let dirs = vec![icons.clone()];
        assert_eq!(icon_file_in(&desktop, &dirs), Some(apps.join("myapp.png")));
        // A missing icon is a clean miss, not a panic or a partial path.
        fs::remove_file(apps.join("myapp.png")).unwrap();
        assert_eq!(icon_file_in(&desktop, &dirs), None);
        // A hidden entry resolves to nothing even when its icon exists.
        fs::write(apps.join("myapp.png"), b"png").unwrap();
        fs::write(
            &desktop,
            "[Desktop Entry]\nType=Application\nHidden=true\nIcon=myapp\n",
        )
        .unwrap();
        assert_eq!(icon_file_in(&desktop, &dirs), None);
        // An absolute Icon= points at the artwork directly.
        fs::write(
            &desktop,
            format!(
                "[Desktop Entry]\nType=Application\nIcon={}\n",
                apps.join("myapp.png").display()
            ),
        )
        .unwrap();
        assert_eq!(icon_file_in(&desktop, &dirs), Some(apps.join("myapp.png")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_or_missing_desktop_files_are_ignored() {
        let root = temp_root("bounds");
        let desktop = root.join("big.desktop");
        fs::write(&desktop, vec![b'x'; MAX_DESKTOP_BYTES as usize + 1]).unwrap();
        assert_eq!(icon_file_in(&desktop, &[root.clone()]), None);
        assert_eq!(
            icon_file_in(&root.join("missing.desktop"), &[root.clone()]),
            None
        );
        let _ = fs::remove_dir_all(&root);
    }
}
