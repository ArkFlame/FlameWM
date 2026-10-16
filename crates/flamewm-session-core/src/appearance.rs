//! Session-scoped appearance preference plan.
//!
//! Every write is confined to a FlameWM-owned root (`<config_dir>/appearance`).
//! Host `~/.config/kdeglobals`, dconf/GSettings, and portal state are never
//! touched. Explicit inherited toolkit variables pass through untouched.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use flamewm_api::settings::AppearanceMode;

use crate::{appearance_environment, SessionPaths};

/// Breeze color scheme names shipped with Plasma (`/usr/share/color-schemes`).
pub const KDE_COLOR_SCHEME_DARK: &str = "BreezeDark";
pub const KDE_COLOR_SCHEME_LIGHT: &str = "BreezeLight";
/// `org.freedesktop.appearance color-scheme` values (portal/GSettings). Returned
/// by [`portal_color_scheme`] as data for a future portal backend; this module
/// never writes portal or GSettings state.
pub const PORTAL_COLOR_SCHEME_DARK: u32 = 1;
pub const PORTAL_COLOR_SCHEME_LIGHT: u32 = 2;
const DEFAULT_XDG_CONFIG_DIRS: &str = "/etc/xdg";
const DEFAULT_XDG_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// GTK3 theme candidates in preference order. A candidate is used only when
/// `<root>/<name>/gtk-3.0` is a directory under a themes root.
const GTK3_DARK_THEMES: &[&str] = &["Breeze-Dark", "Adwaita-dark"];
const GTK3_LIGHT_THEMES: &[&str] = &["Breeze", "Adwaita"];
/// Qt platform theme that applies kdeglobals outside Plasma (plasma-integration).
const KDE_QT_PLATFORM_THEME: &str = "kde";
const KDE_PLATFORM_THEME_PLUGINS: &[&str] =
    &["KDEPlasmaPlatformTheme6.so", "KDEPlasmaPlatformTheme.so"];
const QT_PLUGIN_ROOTS: &[&str] = &[
    "/usr/lib/x86_64-linux-gnu/qt6/plugins",
    "/usr/lib64/qt6/plugins",
    "/usr/lib/qt6/plugins",
    "/usr/lib/x86_64-linux-gnu/qt5/plugins",
    "/usr/lib64/qt5/plugins",
    "/usr/lib/qt5/plugins",
    "/usr/lib/qt/plugins",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Toolkit {
    Qt,
    Gtk3,
    Gtk4,
}

/// No variant reports delivery as observed: nothing in the session observes a
/// toolkit's rendered result, so no toolkit is ever `Supported` here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolkitStatus {
    /// Channel is written, but no executed probe observed the toolkit result.
    Unverified(&'static str),
    /// No session-scoped channel or prerequisite exists; FlameWM does not fake one.
    Unsupported(&'static str),
}

/// Facts the caller observed about the live session. Pure input for [`plan_appearance`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppearanceProbe {
    /// This session owns `_XSETTINGS_S0` (GTK3 receives the published settings).
    pub xsettings_owned: bool,
    /// An installed GTK3 theme matches the requested mode (see [`gtk3_theme_name`]).
    pub gtk3_theme_installed: bool,
    /// The KDE Qt platform theme plugin is installed (see [`kde_platform_theme_installed`]).
    pub kde_platform_theme: bool,
}

/// Whether a KDE platform theme plugin exists under `QT_PLUGIN_PATH` or a
/// standard Qt plugin root. Presence only; the caller decides whether to use it.
#[must_use]
pub fn kde_platform_theme_installed(existing_env: &BTreeMap<String, String>) -> bool {
    let from_env = existing_env
        .get("QT_PLUGIN_PATH")
        .into_iter()
        .flat_map(|value| value.split(':'))
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    from_env
        .chain(QT_PLUGIN_ROOTS.iter().map(PathBuf::from))
        .any(|root| {
            KDE_PLATFORM_THEME_PLUGINS
                .iter()
                .any(|plugin| root.join("platformthemes").join(plugin).is_file())
        })
}

/// Portal `color-scheme` value for a Dark/Light mode; `None` for System, which
/// follows the host preference and is never pinned by this session.
#[must_use]
pub const fn portal_color_scheme(mode: AppearanceMode) -> Option<u32> {
    match mode {
        AppearanceMode::Dark => Some(PORTAL_COLOR_SCHEME_DARK),
        AppearanceMode::Light => Some(PORTAL_COLOR_SCHEME_LIGHT),
        AppearanceMode::System => None,
    }
}

/// GTK3/GTK4 `settings.ini` body for a Dark/Light mode. Only the prefer-dark key
/// is written; no `gtk-theme-name` is set, so the installed theme stays with XSETTINGS.
fn gtk_settings_ini(mode: AppearanceMode) -> String {
    let prefer_dark = u8::from(mode.prefers_dark());
    format!("[Settings]\ngtk-application-prefer-dark-theme={prefer_dark}\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearancePlan {
    pub mode: AppearanceMode,
    /// Environment overlay: FLAMEWM_* markers plus owned XDG_CONFIG_DIRS prefix.
    pub environment: BTreeMap<String, String>,
    /// Files to materialize under `owned_root`, keyed by relative path.
    pub owned_files: BTreeMap<PathBuf, String>,
    pub owned_root: PathBuf,
    pub toolkits: BTreeMap<Toolkit, ToolkitStatus>,
}

#[must_use]
pub fn owned_appearance_root(paths: &SessionPaths) -> PathBuf {
    paths.config_dir().join("appearance")
}

/// Effective appearance, resolved once at startup: a parsable explicit
/// `FLAMEWM_APPEARANCE` wins, then the persisted settings appearance, then Dark.
#[must_use]
pub fn resolve_startup_appearance(
    explicit_env: Option<&str>,
    persisted: Option<AppearanceMode>,
) -> AppearanceMode {
    explicit_env
        .and_then(AppearanceMode::parse)
        .or(persisted)
        .unwrap_or_default()
}

/// Installed GTK3 theme for `mode`, searched in the standard theme roots
/// derived from `existing_env` (HOME, XDG_DATA_HOME, XDG_DATA_DIRS). Dark
/// prefers `Breeze-Dark` then `Adwaita-dark`; light and system prefer `Breeze`
/// then `Adwaita`. `None` when no candidate is installed.
#[must_use]
pub fn gtk3_theme_name(
    mode: AppearanceMode,
    existing_env: &BTreeMap<String, String>,
) -> Option<&'static str> {
    let candidates = if mode.prefers_dark() {
        GTK3_DARK_THEMES
    } else {
        GTK3_LIGHT_THEMES
    };
    let roots = gtk3_theme_roots(existing_env);
    candidates.iter().copied().find(|name| {
        roots
            .iter()
            .any(|root| root.join(name).join("gtk-3.0").is_dir())
    })
}

fn inherited_platform_theme(existing_env: &BTreeMap<String, String>) -> Option<&str> {
    existing_env
        .get("QT_QPA_PLATFORMTHEME")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

/// Full Breeze scheme file for a Dark/Light mode from `<data_root>/color-schemes`.
/// The named `ColorScheme=` key alone does not color Dolphin's folder view
/// (tools probe: view stayed white), so the owned file carries the scheme's role groups.
fn breeze_color_scheme_file(paths: &SessionPaths, mode: AppearanceMode) -> Option<String> {
    let name = if mode.prefers_dark() {
        KDE_COLOR_SCHEME_DARK
    } else {
        KDE_COLOR_SCHEME_LIGHT
    };
    std::fs::read_to_string(
        paths
            .data_root
            .join("color-schemes")
            .join(format!("{name}.colors")),
    )
    .ok()
}

fn gtk3_theme_roots(existing_env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let non_empty = |key: &str| existing_env.get(key).filter(|value| !value.is_empty());
    let mut roots = Vec::new();
    if let Some(home) = non_empty("HOME") {
        roots.push(Path::new(home).join(".themes"));
        roots.push(Path::new(home).join(".local/share/themes"));
    }
    let data_home = non_empty("XDG_DATA_HOME").map_or_else(
        || non_empty("HOME").map(|home| Path::new(home).join(".local/share")),
        |value| Some(PathBuf::from(value)),
    );
    if let Some(data_home) = data_home {
        roots.push(data_home.join("themes"));
    }
    let data_dirs = non_empty("XDG_DATA_DIRS").map_or(DEFAULT_XDG_DATA_DIRS, String::as_str);
    roots.extend(
        data_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| Path::new(dir).join("themes")),
    );
    roots
}

#[must_use]
pub fn plan_appearance(
    paths: &SessionPaths,
    existing_env: &BTreeMap<String, String>,
    mode: AppearanceMode,
    probe: AppearanceProbe,
) -> AppearancePlan {
    let owned_root = owned_appearance_root(paths);
    let mut environment = appearance_environment(mode);
    let mut owned_files = BTreeMap::new();
    let mut toolkits = BTreeMap::new();
    // libadwaita reads org.freedesktop.appearance color-scheme from the portal, which
    // has no session-owned backend; the owned settings.ini is not claimed as applied.
    let gtk4 = ToolkitStatus::Unsupported(
        "libadwaita follows portal color-scheme; no session portal backend exists",
    );
    let gtk3 = match (mode, probe.xsettings_owned, probe.gtk3_theme_installed) {
        (AppearanceMode::System, _, _) => {
            ToolkitStatus::Unsupported("system mode follows the host preference")
        }
        (_, false, _) => ToolkitStatus::Unsupported("XSettings manager not owned by this session"),
        (_, true, false) => {
            ToolkitStatus::Unsupported("no installed GTK3 theme for this appearance mode")
        }
        (_, true, true) => ToolkitStatus::Unverified(
            "XSettings published with installed GTK3 theme; GTK3 render not probed",
        ),
    };

    let color_scheme = matches!(mode, AppearanceMode::Dark | AppearanceMode::Light)
        .then(|| breeze_color_scheme_file(paths, mode))
        .flatten();
    let qt = match mode {
        AppearanceMode::System => {
            ToolkitStatus::Unsupported("system mode follows the host preference")
        }
        AppearanceMode::Dark | AppearanceMode::Light
            if owned_root.to_string_lossy().contains(':') =>
        {
            ToolkitStatus::Unsupported(
                "owned appearance root contains ':' and cannot be listed in XDG_CONFIG_DIRS",
            )
        }
        AppearanceMode::Dark | AppearanceMode::Light if !probe.kde_platform_theme => {
            ToolkitStatus::Unsupported("KDE Qt platform theme plugin not installed")
        }
        AppearanceMode::Dark | AppearanceMode::Light
            if inherited_platform_theme(existing_env)
                .is_some_and(|value| value != KDE_QT_PLATFORM_THEME) =>
        {
            ToolkitStatus::Unsupported(
                "inherited QT_QPA_PLATFORMTHEME wins; owned kdeglobals is not applied",
            )
        }
        AppearanceMode::Dark | AppearanceMode::Light if color_scheme.is_none() => {
            ToolkitStatus::Unsupported("Breeze color scheme file not installed")
        }
        AppearanceMode::Dark | AppearanceMode::Light => {
            // The guard above ensures the scheme file was read.
            let colors = color_scheme.unwrap_or_default();
            owned_files.insert(PathBuf::from("kdeglobals"), colors);
            // GTK reads these through the same owned XDG_CONFIG_DIRS prefix; the
            // user's own gtk-*/settings.ini still wins per key.
            owned_files.insert(
                PathBuf::from("gtk-3.0/settings.ini"),
                gtk_settings_ini(mode),
            );
            owned_files.insert(
                PathBuf::from("gtk-4.0/settings.ini"),
                gtk_settings_ini(mode),
            );
            // Owned root is listed first; the user's ~/.config/kdeglobals still
            // wins per key, and an inherited XDG_CONFIG_DIRS is kept after it.
            let inherited = existing_env
                .get("XDG_CONFIG_DIRS")
                .filter(|value| !value.is_empty())
                .map_or(DEFAULT_XDG_CONFIG_DIRS, String::as_str);
            environment.insert(
                "XDG_CONFIG_DIRS".to_owned(),
                format!("{}:{inherited}", owned_root.display()),
            );
            // Qt reads kdeglobals only through the KDE platform theme. Set it only
            // when no explicit value is inherited (checked above).
            if inherited_platform_theme(existing_env).is_none() {
                environment.insert(
                    "QT_QPA_PLATFORMTHEME".to_owned(),
                    KDE_QT_PLATFORM_THEME.to_owned(),
                );
            }
            ToolkitStatus::Unverified(
                "kdeglobals ColorScheme + KDE platform theme; KCalc dark/light observed by tools probe",
            )
        }
    };

    toolkits.insert(Toolkit::Qt, qt);
    toolkits.insert(Toolkit::Gtk3, gtk3);
    toolkits.insert(Toolkit::Gtk4, gtk4);
    AppearancePlan {
        mode,
        environment,
        owned_files,
        owned_root,
        toolkits,
    }
}

/// Write the plan's owned files under its owned root. Relative paths must be
/// plain components (no `..`, no absolute roots); anything else is rejected
/// before any write.
pub fn materialize_owned_files(plan: &AppearancePlan) -> io::Result<()> {
    for relative in plan.owned_files.keys() {
        let plain = relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        if !plain || relative.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "owned appearance path must be a plain relative path",
            ));
        }
    }
    std::fs::create_dir_all(&plan.owned_root)?;
    for (relative, content) in &plan.owned_files {
        // Write beside the target and rename, so readers see the old or the new file, never a partial one.
        let target = plan.owned_root.join(relative);
        let mut temp_name = relative.as_os_str().to_owned();
        temp_name.push(".tmp");
        let temp = plan.owned_root.join(temp_name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&temp, content)?;
        std::fs::rename(&temp, &target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_home(name: &str) -> PathBuf {
        let home =
            std::env::temp_dir().join(format!("flamewm-appearance-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        home
    }

    const DARK_SCHEME: &str =
        "[General]\nColorScheme=BreezeDark\n[Colors:View]\nBackgroundNormal=20,22,24\n";
    const LIGHT_SCHEME: &str =
        "[General]\nColorScheme=BreezeLight\n[Colors:View]\nBackgroundNormal=255,255,255\n";

    fn paths_for(home: &Path) -> SessionPaths {
        let data_root = home.join("share");
        let schemes = data_root.join("color-schemes");
        std::fs::create_dir_all(&schemes).expect("schemes");
        std::fs::write(schemes.join("BreezeDark.colors"), DARK_SCHEME).expect("dark");
        std::fs::write(schemes.join("BreezeLight.colors"), LIGHT_SCHEME).expect("light");
        SessionPaths {
            bindir: "/usr/bin".into(),
            config_home: home.join(".config"),
            data_root,
        }
    }

    const QT_PLUGIN_PRESENT: AppearanceProbe = AppearanceProbe {
        xsettings_owned: false,
        gtk3_theme_installed: false,
        kde_platform_theme: true,
    };

    fn env_with_home(home: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("HOME".to_owned(), home.display().to_string()),
            ("XDG_DATA_DIRS".to_owned(), "/nonexistent-flame".to_owned()),
        ])
    }

    #[test]
    fn dark_plan_owns_breeze_dark_and_prefixes_owned_config_dirs() {
        let home = isolated_home("dark");
        let paths = paths_for(&home);
        let plan = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            QT_PLUGIN_PRESENT,
        );
        assert_eq!(
            plan.owned_files
                .get(Path::new("kdeglobals"))
                .map(String::as_str),
            Some(DARK_SCHEME)
        );
        let xdg = plan.environment.get("XDG_CONFIG_DIRS").expect("xdg");
        assert_eq!(
            xdg,
            &format!("{}:/etc/xdg", owned_appearance_root(&paths).display())
        );
        for forbidden in ["GTK_THEME", "QT_STYLE_OVERRIDE"] {
            assert!(!plan.environment.contains_key(forbidden), "{forbidden}");
        }
        assert_eq!(
            plan.environment
                .get("QT_QPA_PLATFORMTHEME")
                .map(String::as_str),
            Some("kde")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn light_plan_and_inherited_config_dirs_are_preserved_after_owned_root() {
        let home = isolated_home("light");
        let paths = paths_for(&home);
        let existing = BTreeMap::from([("XDG_CONFIG_DIRS".to_owned(), "/opt/xdg".to_owned())]);
        let plan = plan_appearance(&paths, &existing, AppearanceMode::Light, QT_PLUGIN_PRESENT);
        assert_eq!(
            plan.owned_files
                .get(Path::new("kdeglobals"))
                .map(String::as_str),
            Some(LIGHT_SCHEME)
        );
        assert_eq!(
            plan.environment.get("XDG_CONFIG_DIRS").map(String::as_str),
            Some(format!("{}:/opt/xdg", owned_appearance_root(&paths).display()).as_str())
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn system_mode_writes_nothing_and_exports_no_qt_channel() {
        let home = isolated_home("system");
        let plan = plan_appearance(
            &paths_for(&home),
            &BTreeMap::new(),
            AppearanceMode::System,
            AppearanceProbe::default(),
        );
        assert!(plan.owned_files.is_empty());
        assert!(!plan.environment.contains_key("XDG_CONFIG_DIRS"));
        assert!(matches!(
            plan.toolkits[&Toolkit::Qt],
            ToolkitStatus::Unsupported(_)
        ));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn toolkit_matrix_tracks_probe_and_never_reports_supported() {
        let home = isolated_home("matrix");
        let paths = paths_for(&home);
        let ready = AppearanceProbe {
            xsettings_owned: true,
            gtk3_theme_installed: true,
            kde_platform_theme: true,
        };
        let owned = plan_appearance(&paths, &BTreeMap::new(), AppearanceMode::Dark, ready);
        assert!(matches!(
            owned.toolkits[&Toolkit::Gtk3],
            ToolkitStatus::Unverified(_)
        ));
        assert!(matches!(
            owned.toolkits[&Toolkit::Qt],
            ToolkitStatus::Unverified(_)
        ));
        assert_eq!(
            owned
                .environment
                .get("QT_QPA_PLATFORMTHEME")
                .map(String::as_str),
            Some("kde")
        );
        assert!(matches!(
            owned.toolkits[&Toolkit::Gtk4],
            ToolkitStatus::Unsupported(_)
        ));
        let foreign = AppearanceProbe {
            xsettings_owned: false,
            gtk3_theme_installed: true,
            kde_platform_theme: true,
        };
        let plan = plan_appearance(&paths, &BTreeMap::new(), AppearanceMode::Dark, foreign);
        assert!(matches!(
            plan.toolkits[&Toolkit::Gtk3],
            ToolkitStatus::Unsupported(_)
        ));
        let no_theme = AppearanceProbe {
            xsettings_owned: true,
            gtk3_theme_installed: false,
            kde_platform_theme: true,
        };
        let plan = plan_appearance(&paths, &BTreeMap::new(), AppearanceMode::Dark, no_theme);
        assert!(matches!(
            plan.toolkits[&Toolkit::Gtk3],
            ToolkitStatus::Unsupported(_)
        ));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn gtk3_theme_prefers_installed_dark_theme_and_never_invents_one() {
        let home = isolated_home("gtk3-theme");
        let env = env_with_home(&home);
        assert_eq!(gtk3_theme_name(AppearanceMode::Dark, &env), None);
        // Light-only install must not satisfy a dark request.
        std::fs::create_dir_all(home.join(".themes/Breeze/gtk-3.0")).expect("breeze");
        assert_eq!(gtk3_theme_name(AppearanceMode::Dark, &env), None);
        assert_eq!(gtk3_theme_name(AppearanceMode::Light, &env), Some("Breeze"));
        // A theme dir without gtk-3.0 is not a GTK3 theme.
        std::fs::create_dir_all(home.join(".themes/Breeze-Dark")).expect("bare dark");
        assert_eq!(gtk3_theme_name(AppearanceMode::Dark, &env), None);
        std::fs::create_dir_all(home.join(".themes/Breeze-Dark/gtk-3.0")).expect("dark");
        assert_eq!(
            gtk3_theme_name(AppearanceMode::Dark, &env),
            Some("Breeze-Dark")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn qt_channel_sets_kde_platform_theme_only_when_installed_and_unset() {
        let home = isolated_home("qt-theme");
        let paths = paths_for(&home);
        let probe = |kde_platform_theme| AppearanceProbe {
            kde_platform_theme,
            ..AppearanceProbe::default()
        };
        // Plugin absent: no platform theme, Qt reported unsupported.
        let absent = plan_appearance(&paths, &BTreeMap::new(), AppearanceMode::Dark, probe(false));
        assert!(!absent.environment.contains_key("QT_QPA_PLATFORMTHEME"));
        assert!(matches!(
            absent.toolkits[&Toolkit::Qt],
            ToolkitStatus::Unsupported(_)
        ));
        // Plugin present and unset: kde is exported.
        let unset = plan_appearance(&paths, &BTreeMap::new(), AppearanceMode::Light, probe(true));
        assert_eq!(
            unset
                .environment
                .get("QT_QPA_PLATFORMTHEME")
                .map(String::as_str),
            Some("kde")
        );
        // Explicit inherited values are never overridden; a non-kde one disables the channel.
        let fusion = BTreeMap::from([("QT_QPA_PLATFORMTHEME".to_owned(), "fusion".to_owned())]);
        let plan = plan_appearance(&paths, &fusion, AppearanceMode::Dark, probe(true));
        assert!(!plan.environment.contains_key("QT_QPA_PLATFORMTHEME"));
        assert!(matches!(
            plan.toolkits[&Toolkit::Qt],
            ToolkitStatus::Unsupported(_)
        ));
        let kde = BTreeMap::from([("QT_QPA_PLATFORMTHEME".to_owned(), "kde".to_owned())]);
        let plan = plan_appearance(&paths, &kde, AppearanceMode::Dark, probe(true));
        assert!(!plan.environment.contains_key("QT_QPA_PLATFORMTHEME"));
        assert!(matches!(
            plan.toolkits[&Toolkit::Qt],
            ToolkitStatus::Unverified(_)
        ));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn kde_platform_theme_detection_reads_qt_plugin_path() {
        let root = std::env::temp_dir().join(format!("flamewm-qt-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("platformthemes")).expect("dir");
        let env = BTreeMap::from([("QT_PLUGIN_PATH".to_owned(), root.display().to_string())]);
        std::fs::write(root.join("platformthemes/KDEPlasmaPlatformTheme6.so"), b"")
            .expect("plugin");
        assert!(kde_platform_theme_installed(&env));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_breeze_scheme_file_disables_qt_channel_instead_of_named_only_write() {
        let home = isolated_home("no-scheme");
        let paths = SessionPaths {
            bindir: "/usr/bin".into(),
            config_home: home.join(".config"),
            data_root: home.join("empty-share"),
        };
        let plan = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            QT_PLUGIN_PRESENT,
        );
        assert!(plan.owned_files.is_empty());
        assert!(!plan.environment.contains_key("XDG_CONFIG_DIRS"));
        assert!(matches!(
            plan.toolkits[&Toolkit::Qt],
            ToolkitStatus::Unsupported(_)
        ));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn colon_in_owned_root_disables_qt_channel() {
        let home = isolated_home("colon");
        let paths = SessionPaths {
            bindir: "/usr/bin".into(),
            config_home: home.join("a:b"),
            data_root: "/usr/share".into(),
        };
        let plan = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            AppearanceProbe::default(),
        );
        assert!(plan.owned_files.is_empty());
        assert!(!plan.environment.contains_key("XDG_CONFIG_DIRS"));
        assert!(matches!(
            plan.toolkits[&Toolkit::Qt],
            ToolkitStatus::Unsupported(_)
        ));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn materialize_writes_only_under_owned_root_and_never_host_kdeglobals() {
        let home = isolated_home("materialize");
        let paths = paths_for(&home);
        let plan = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            QT_PLUGIN_PRESENT,
        );
        materialize_owned_files(&plan).expect("materialize");
        let written = std::fs::read_to_string(owned_appearance_root(&paths).join("kdeglobals"))
            .expect("owned file");
        assert_eq!(written, DARK_SCHEME);
        assert!(
            !paths.config_home.join("kdeglobals").exists(),
            "host kdeglobals must not be written"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn startup_appearance_precedence_is_env_then_persisted_then_dark() {
        // Persisted light with the env unset must resolve to light, not the Dark default.
        assert_eq!(
            resolve_startup_appearance(None, Some(AppearanceMode::Light)),
            AppearanceMode::Light
        );
        // A parsable explicit env value wins over the persisted value, including system.
        assert_eq!(
            resolve_startup_appearance(Some("dark"), Some(AppearanceMode::Light)),
            AppearanceMode::Dark
        );
        assert_eq!(
            resolve_startup_appearance(Some(" System "), Some(AppearanceMode::Dark)),
            AppearanceMode::System
        );
        // Unparsable or empty env values are not overrides.
        assert_eq!(
            resolve_startup_appearance(Some("purple"), Some(AppearanceMode::Light)),
            AppearanceMode::Light
        );
        assert_eq!(
            resolve_startup_appearance(Some(""), Some(AppearanceMode::Light)),
            AppearanceMode::Light
        );
        // Neither source: default Dark.
        assert_eq!(resolve_startup_appearance(None, None), AppearanceMode::Dark);
        assert_eq!(
            resolve_startup_appearance(Some("purple"), None),
            AppearanceMode::Dark
        );
    }

    #[test]
    fn materialize_replaces_owned_file_atomically_without_temp_leftovers() {
        let home = isolated_home("atomic");
        let paths = paths_for(&home);
        let light = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Light,
            QT_PLUGIN_PRESENT,
        );
        materialize_owned_files(&light).expect("light");
        let dark = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            QT_PLUGIN_PRESENT,
        );
        materialize_owned_files(&dark).expect("dark");
        let root = owned_appearance_root(&paths);
        assert_eq!(
            std::fs::read_to_string(root.join("kdeglobals")).expect("owned"),
            DARK_SCHEME
        );
        assert!(
            !root.join("kdeglobals.tmp").exists(),
            "temp file must be renamed away"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn materialize_rejects_escaping_paths_before_any_write() {
        let home = isolated_home("escape");
        let paths = paths_for(&home);
        let mut plan = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            AppearanceProbe::default(),
        );
        plan.owned_files
            .insert(PathBuf::from("../../kdeglobals"), "x".to_owned());
        let error = materialize_owned_files(&plan).expect_err("escape must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!paths.config_home.join("kdeglobals").exists());
        assert!(!owned_appearance_root(&paths).exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn gtk_settings_ini_prefers_dark_only_for_dark_mode() {
        let home = isolated_home("gtk-ini");
        let paths = paths_for(&home);
        let dark = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            QT_PLUGIN_PRESENT,
        );
        let light = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Light,
            QT_PLUGIN_PRESENT,
        );
        for key in ["gtk-3.0/settings.ini", "gtk-4.0/settings.ini"] {
            assert_eq!(
                dark.owned_files.get(Path::new(key)).map(String::as_str),
                Some("[Settings]\ngtk-application-prefer-dark-theme=1\n"),
                "{key} dark"
            );
            assert_eq!(
                light.owned_files.get(Path::new(key)).map(String::as_str),
                Some("[Settings]\ngtk-application-prefer-dark-theme=0\n"),
                "{key} light"
            );
        }
        assert!(!dark.environment.contains_key("GTK_THEME"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn gtk_settings_are_written_under_owned_root_only() {
        let home = isolated_home("gtk-materialize");
        let paths = paths_for(&home);
        let plan = plan_appearance(
            &paths,
            &BTreeMap::new(),
            AppearanceMode::Dark,
            QT_PLUGIN_PRESENT,
        );
        materialize_owned_files(&plan).expect("materialize");
        let root = owned_appearance_root(&paths);
        assert_eq!(
            std::fs::read_to_string(root.join("gtk-3.0/settings.ini")).expect("gtk3"),
            "[Settings]\ngtk-application-prefer-dark-theme=1\n"
        );
        assert!(root.join("gtk-4.0/settings.ini").is_file());
        assert!(!root.join("gtk-3.0/settings.ini.tmp").exists());
        assert!(!home.join(".config/gtk-3.0").exists(), "host gtk config");
        assert!(!home.join(".config/gtk-4.0").exists(), "host gtk config");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn inherited_qt_override_suppresses_owned_gtk_settings_with_kdeglobals() {
        let home = isolated_home("gtk-override");
        let paths = paths_for(&home);
        let fusion = BTreeMap::from([("QT_QPA_PLATFORMTHEME".to_owned(), "fusion".to_owned())]);
        let plan = plan_appearance(&paths, &fusion, AppearanceMode::Dark, QT_PLUGIN_PRESENT);
        assert!(plan.owned_files.is_empty());
        assert!(!plan.environment.contains_key("XDG_CONFIG_DIRS"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn portal_color_scheme_maps_dark_light_and_leaves_system_unpinned() {
        assert_eq!(portal_color_scheme(AppearanceMode::Dark), Some(1));
        assert_eq!(portal_color_scheme(AppearanceMode::Light), Some(2));
        assert_eq!(portal_color_scheme(AppearanceMode::System), None);
    }
}
