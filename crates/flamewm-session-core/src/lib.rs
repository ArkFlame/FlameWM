//! FlameWM session launch and supervision policy.
//!
//! This crate is intentionally independent. It plans FlameWM-owned processes only; process
//! spawning, signals, and parent-death handling remain adapter responsibilities.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flamewm_api::settings::AppearanceMode;

pub mod appearance;
pub mod display_order;
pub use appearance::{
    AppearancePlan, AppearanceProbe, Toolkit, ToolkitStatus, gtk3_theme_name,
    kde_platform_theme_installed, materialize_owned_files, owned_appearance_root, plan_appearance,
    resolve_startup_appearance,
};

/// Emergency bundled fallback. Retained for machines without either curated
/// cursor family, and for explicit saved selections from earlier FlameWM builds.
pub const CURSOR_THEME_ID: &str = "FlameWM-Breeze-Dark";
/// Preferred theme on a fresh profile, when present under XCURSOR_PATH.
pub const MCMOJAVE_CURSOR_THEME_ID: &str = "McMojave-cursors";
pub const CURSOR_SIZE: &str = "24";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorThemeFamily {
    McMojave,
    Oxygen,
}

/// A catalog and fallback policy based on stable directory ids, never a theme
/// display name. Dashes/underscores delimit real upstream cursor variants.
#[must_use]
pub fn cursor_theme_family(id: &str) -> Option<CursorThemeFamily> {
    let id = id.to_ascii_lowercase();
    let variant = |prefix: &str| {
        id == prefix
            || id
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('_'))
    };
    if variant("mcmojave") {
        Some(CursorThemeFamily::McMojave)
    } else if variant("oxygen") || variant("oxy") {
        Some(CursorThemeFamily::Oxygen)
    } else {
        None
    }
}

/// Resolve the cursor used by the WM and XSETTINGS. Explicit installed ids
/// (including older third-party choices) win. Otherwise choose McMojave,
/// then a verified installed Oxygen variant, then bundled Breeze as a last
/// resort. Never creates, deletes, or mutates host cursor directories.
#[must_use]
pub fn resolve_cursor_theme(requested: &str, search_path: &str) -> String {
    let roots: Vec<&str> = search_path
        .split(':')
        .filter(|root| !root.is_empty())
        .collect();
    let installed = |name: &str| {
        !name.is_empty()
            && !name.contains(['/', '\\'])
            && !matches!(name, "." | "..")
            && roots
                .iter()
                .any(|root| Path::new(root).join(name).join("cursors").is_dir())
    };
    let requested = requested.trim();
    if installed(requested) {
        return requested.to_owned();
    }
    let has_pointer = |name: &str| {
        roots.iter().any(|root| {
            Path::new(root)
                .join(name)
                .join("cursors/left_ptr")
                .is_file()
        })
    };
    if installed(MCMOJAVE_CURSOR_THEME_ID) && has_pointer(MCMOJAVE_CURSOR_THEME_ID) {
        return MCMOJAVE_CURSOR_THEME_ID.to_owned();
    }
    // Other McMojave variants are valid independent theme directory ids.
    // Sort for deterministic selection when multiple system roots exist.
    let mut candidates = Vec::new();
    for root in &roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if cursor_theme_family(&id).is_some() && installed(&id) && has_pointer(&id) {
                candidates.push(id);
            }
        }
    }
    candidates.sort_by_key(|id| id.to_ascii_lowercase());
    candidates.dedup();
    if let Some(id) = candidates
        .iter()
        .find(|id| cursor_theme_family(id) == Some(CursorThemeFamily::McMojave))
    {
        return id.clone();
    }
    if let Some(id) = candidates
        .iter()
        .find(|id| cursor_theme_family(id) == Some(CursorThemeFamily::Oxygen))
    {
        return id.clone();
    }
    CURSOR_THEME_ID.to_owned()
}

/// Resolves a request and returns it only when the applied theme would change.
#[must_use]
pub fn cursor_theme_reload(
    applied: Option<&str>,
    requested: &str,
    search_path: &str,
) -> Option<String> {
    let resolved = resolve_cursor_theme(requested, search_path);
    (applied != Some(resolved.as_str())).then_some(resolved)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPaths {
    pub bindir: PathBuf,
    pub config_home: PathBuf,
    pub data_root: PathBuf,
}

impl SessionPaths {
    #[must_use]
    pub fn flamewm_binary(&self) -> PathBuf {
        self.bindir.join("flamewm")
    }

    #[must_use]
    pub fn shell_binary(&self) -> PathBuf {
        self.bindir.join("flamewm-shell")
    }

    #[must_use]
    pub fn desktop_binary(&self) -> PathBuf {
        self.bindir.join("flamewm-desktop")
    }

    #[must_use]
    pub fn config_dir(&self) -> PathBuf {
        self.config_home.join("flamewm")
    }

    #[must_use]
    pub fn cursor_search_root(&self) -> PathBuf {
        self.data_root.join("icons")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    WindowManager,
    Desktop,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPlan {
    pub role: SessionRole,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLaunchPlan {
    pub processes: Vec<ProcessPlan>,
}

#[must_use]
pub fn build_launch_plan(
    paths: &SessionPaths,
    wm_passthrough: &[String],
    existing_env: &BTreeMap<String, String>,
) -> SessionLaunchPlan {
    build_launch_plan_with_cursor_library_path(paths, wm_passthrough, existing_env, None)
}

#[must_use]
pub fn build_launch_plan_with_appearance(
    paths: &SessionPaths,
    wm_passthrough: &[String],
    existing_env: &BTreeMap<String, String>,
    cursor_library_path: Option<&str>,
    appearance: AppearanceMode,
) -> SessionLaunchPlan {
    let mut plan = build_launch_plan_with_cursor_library_path(
        paths,
        wm_passthrough,
        existing_env,
        cursor_library_path,
    );
    let probe = AppearanceProbe {
        kde_platform_theme: kde_platform_theme_installed(existing_env),
        ..AppearanceProbe::default()
    };
    let overlay = plan_appearance(paths, existing_env, appearance, probe).environment;
    for process in &mut plan.processes {
        for (key, value) in &overlay {
            process.environment.insert(key.clone(), value.clone());
        }
    }
    plan
}

#[must_use]
pub fn build_launch_plan_with_cursor_library_path(
    paths: &SessionPaths,
    wm_passthrough: &[String],
    existing_env: &BTreeMap<String, String>,
    cursor_library_path: Option<&str>,
) -> SessionLaunchPlan {
    let environment = flame_environment(paths, existing_env, cursor_library_path);
    let wm_arguments = wm_passthrough
        .iter()
        .filter(|argument| !argument.starts_with("--config-dir="))
        .cloned()
        .collect::<Vec<_>>();

    SessionLaunchPlan {
        processes: vec![
            ProcessPlan {
                role: SessionRole::WindowManager,
                program: paths.flamewm_binary(),
                arguments: wm_arguments,
                environment: environment.clone(),
                required: true,
            },
            ProcessPlan {
                role: SessionRole::Desktop,
                program: paths.desktop_binary(),
                arguments: Vec::new(),
                environment: environment.clone(),
                required: true,
            },
            ProcessPlan {
                role: SessionRole::Shell,
                program: paths.shell_binary(),
                arguments: Vec::new(),
                environment,
                required: true,
            },
        ],
    }
}

/// Select a KDE-generation hint without inspecting browser profiles or wallet secrets.
/// Explicit provenance wins. If both daemon generations exist, availability
/// alone is ambiguous and cannot determine the browser's previous backend.
#[must_use]
pub fn resolve_kwallet_version(
    existing_env: &BTreeMap<String, String>,
    installed5: bool,
    installed6: bool,
) -> Option<&'static str> {
    match existing_env
        .get("FLAMEWM_BRAVE_PASSWORD_STORE")
        .map(String::as_str)
    {
        Some("kwallet5") => return Some("5"),
        Some("kwallet6") => return Some("6"),
        Some("gnome-libsecret" | "basic" | "none") => return None,
        _ => {}
    }
    // A leftover KDE_SESSION_VERSION from a different desktop is not
    // trustworthy. Treat it as provenance only when inherited with a real
    // KDE desktop identity (or via the explicit FlameWM override above).
    if existing_env
        .get("XDG_CURRENT_DESKTOP")
        .is_some_and(|desktop| desktop.split(':').any(|part| part == "KDE"))
    {
        match existing_env.get("KDE_SESSION_VERSION").map(String::as_str) {
            Some("5") => return Some("5"),
            Some("6") => return Some("6"),
            _ => {}
        }
    }
    match (installed5, installed6) {
        (true, false) => Some("5"),
        (false, true) => Some("6"),
        _ => None,
    }
}

fn flame_environment(
    paths: &SessionPaths,
    existing_env: &BTreeMap<String, String>,
    cursor_library_path: Option<&str>,
) -> BTreeMap<String, String> {
    // Empty values count as unset throughout, so `FOO=` never overrides a default.
    let non_empty = |key: &str| existing_env.get(key).filter(|value| !value.is_empty());
    let mut environment = BTreeMap::new();
    // Bundled authority first: explicit FLAMEWM_CURSOR_* wins, then existing
    // XCURSOR_*, then the bundled theme/size defaults.
    let requested_theme = non_empty("FLAMEWM_CURSOR_THEME")
        .or_else(|| non_empty("XCURSOR_THEME"))
        .map_or("", String::as_str);
    environment.insert(
        "XCURSOR_SIZE".to_owned(),
        non_empty("FLAMEWM_CURSOR_SIZE")
            .or_else(|| non_empty("XCURSOR_SIZE"))
            .cloned()
            .unwrap_or_else(|| CURSOR_SIZE.to_owned()),
    );
    environment.insert(
        "XCURSOR_THEME_CORE".to_owned(),
        existing_env
            .get("XCURSOR_THEME_CORE")
            .cloned()
            .unwrap_or_else(|| "1".to_owned()),
    );
    // An inherited FLAMEWM_CONFIG_DIR (isolated runs, tests) wins over the
    // session default; overwriting it sent the WM to ~/.config/flamewm.
    environment.insert(
        "FLAMEWM_CONFIG_DIR".to_owned(),
        non_empty("FLAMEWM_CONFIG_DIR")
            .cloned()
            .unwrap_or_else(|| paths.config_dir().display().to_string()),
    );
    environment.insert("FLAMEWM_DESKTOP".to_owned(), "FlameWM".to_owned());
    // FlameWM is its own desktop. Advertising KDE here causes KDE-only
    // autostarts (notably KScreen) to compete for RandR ownership when an
    // application activates KDE services such as Dolphin's Open With dialog.
    // Desktop-specific KDE MIME defaults and Brave's KWallet backend are
    // bridged individually by flamewm-session, not through global impersonation.
    environment.insert("XDG_CURRENT_DESKTOP".to_owned(), "FlameWM".to_owned());
    environment.insert("XDG_SESSION_DESKTOP".to_owned(), "flamewm".to_owned());
    // KService reads ${XDG_MENU_PREFIX}applications.menu when building KSycoca.
    environment.insert("XDG_MENU_PREFIX".to_owned(), "flamewm-".to_owned());
    // Keep KDE's version only as a browser compatibility hint; desktop identity
    // stays FlameWM. A daemon's existence never proves which wallet encrypted a
    // pre-existing browser profile. Ambiguous generations require provenance.
    let kde6 = [
        "/usr/bin/kwalletd6",
        "/usr/share/dbus-1/services/org.kde.kwalletd6.service",
    ];
    let kde5 = [
        "/usr/bin/kwalletd5",
        "/usr/share/dbus-1/services/org.kde.kwalletd5.service",
    ];
    let installed6 = kde6.iter().any(|path| Path::new(path).exists());
    let installed5 = kde5.iter().any(|path| Path::new(path).exists());
    if let Some(version) = resolve_kwallet_version(existing_env, installed5, installed6) {
        environment.insert("KDE_SESSION_VERSION".to_owned(), version.to_owned());
    }

    // Keep bundled themes first, then search standard XDG icon roots so the
    // themes shown by Settings remain usable when the login manager omits
    // XCURSOR_PATH. An explicit FlameWM cursor path stays isolated.
    let explicit_cursor_path = non_empty("FLAMEWM_CURSOR_PATH");
    let search_root = explicit_cursor_path
        .cloned()
        .unwrap_or_else(|| paths.cursor_search_root().display().to_string());
    let mut cursor_roots = vec![search_root];
    if explicit_cursor_path.is_none() {
        let home = non_empty("HOME")
            .map(PathBuf::from)
            .or_else(|| paths.config_home.parent().map(Path::to_path_buf));
        if let Some(data_home) = non_empty("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".local/share")))
        {
            cursor_roots.push(data_home.join("icons").display().to_string());
        }
        let data_dirs = non_empty("XDG_DATA_DIRS")
            .map(String::as_str)
            .unwrap_or("/usr/local/share:/usr/share");
        for dir in data_dirs
            .split(':')
            .filter(|dir| Path::new(dir).is_absolute())
        {
            let root = Path::new(dir).join("icons").display().to_string();
            if !cursor_roots.contains(&root) {
                cursor_roots.push(root);
            }
        }
    }
    if let Some(library_path) = cursor_library_path.filter(|value| !value.is_empty()) {
        for root in library_path.split(':').filter(|root| !root.is_empty()) {
            if !cursor_roots.iter().any(|existing| existing == root) {
                cursor_roots.push(root.to_owned());
            }
        }
    }
    let combined_cursor_path = cursor_roots.join(":");
    environment.insert(
        "XCURSOR_THEME".to_owned(),
        resolve_cursor_theme(requested_theme, &combined_cursor_path),
    );
    environment.insert("XCURSOR_PATH".to_owned(), combined_cursor_path);
    environment
}

/// Appearance markers for the env producer side: FLAMEWM_APPEARANCE and
/// FLAMEWM_DARK_MODE. Never synthesizes generic GTK_THEME/QT_STYLE_OVERRIDE
/// defaults; explicit inherited toolkit values pass through untouched.
/// The Qt channel is the owned XDG_CONFIG_DIRS prefix added by
/// [`appearance::plan_appearance`], not a style-override variable.
#[must_use]
pub fn appearance_environment(appearance: AppearanceMode) -> BTreeMap<String, String> {
    let dark = if appearance.prefers_dark() { "1" } else { "0" };
    BTreeMap::from([
        (
            "FLAMEWM_APPEARANCE".to_owned(),
            appearance.as_str().to_owned(),
        ),
        ("FLAMEWM_DARK_MODE".to_owned(), dark.to_owned()),
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorPolicy {
    pub maximum_restarts: u8,
    pub initial_backoff_ms: u64,
    pub maximum_backoff_ms: u64,
    pub graceful_stop_ms: u64,
}

impl Default for SupervisorPolicy {
    fn default() -> Self {
        Self {
            maximum_restarts: 5,
            initial_backoff_ms: 500,
            maximum_backoff_ms: 8_000,
            graceful_stop_ms: 1_500,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildExit {
    Success,
    ExitCode(i32),
    Signal(i32),
    SpawnFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorDecision {
    SpawnNow,
    RestartAfter(u64),
    StopCleanly,
    GiveUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSupervisorPolicy {
    policy: SupervisorPolicy,
    restarts: u8,
    stopping: bool,
    gave_up: bool,
}

impl Default for ProcessSupervisorPolicy {
    fn default() -> Self {
        Self::new(SupervisorPolicy::default())
    }
}

impl ProcessSupervisorPolicy {
    #[must_use]
    pub const fn new(policy: SupervisorPolicy) -> Self {
        Self {
            policy,
            restarts: 0,
            stopping: false,
            gave_up: false,
        }
    }

    #[must_use]
    pub const fn restarts(&self) -> u8 {
        self.restarts
    }

    #[must_use]
    pub const fn gave_up(&self) -> bool {
        self.gave_up
    }

    pub fn request_stop(&mut self) -> SupervisorDecision {
        self.stopping = true;
        SupervisorDecision::StopCleanly
    }

    pub fn parent_died(&mut self) -> SupervisorDecision {
        self.stopping = true;
        SupervisorDecision::StopCleanly
    }

    pub fn child_exited(&mut self, _exit: ChildExit) -> SupervisorDecision {
        if self.stopping {
            return SupervisorDecision::StopCleanly;
        }
        if self.restarts >= self.policy.maximum_restarts {
            self.gave_up = true;
            return SupervisorDecision::GiveUp;
        }
        let delay = self.backoff_for(self.restarts);
        self.restarts = self.restarts.saturating_add(1);
        SupervisorDecision::RestartAfter(delay)
    }

    #[must_use]
    pub const fn graceful_stop_ms(&self) -> u64 {
        self.policy.graceful_stop_ms
    }

    #[must_use]
    fn backoff_for(&self, attempt: u8) -> u64 {
        let shift = u32::from(attempt.min(7));
        self.policy
            .initial_backoff_ms
            .saturating_mul(1_u64 << shift)
            .min(self.policy.maximum_backoff_ms)
    }
}

#[must_use]
pub fn default_paths_from_home(bindir: impl AsRef<Path>, home: impl AsRef<Path>) -> SessionPaths {
    let bindir = bindir.as_ref().to_path_buf();
    SessionPaths {
        data_root: bindir
            .parent()
            .unwrap_or_else(|| Path::new("/usr"))
            .join("share"),
        bindir,
        config_home: home.as_ref().join(".config"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kwallet_version_requires_unambiguous_provenance() {
        let empty = BTreeMap::new();
        assert_eq!(resolve_kwallet_version(&empty, false, false), None);
        assert_eq!(resolve_kwallet_version(&empty, true, false), Some("5"));
        assert_eq!(resolve_kwallet_version(&empty, false, true), Some("6"));
        assert_eq!(resolve_kwallet_version(&empty, true, true), None);
        let explicit = BTreeMap::from([
            ("KDE_SESSION_VERSION".into(), "5".into()),
            ("XDG_CURRENT_DESKTOP".into(), "KDE".into()),
        ]);
        assert_eq!(resolve_kwallet_version(&explicit, true, true), Some("5"));
        let stale = BTreeMap::from([
            ("KDE_SESSION_VERSION".into(), "5".into()),
            ("XDG_CURRENT_DESKTOP".into(), "FlameWM".into()),
        ]);
        assert_eq!(resolve_kwallet_version(&stale, true, true), None);
        let selected = BTreeMap::from([
            ("KDE_SESSION_VERSION".into(), "5".into()),
            ("FLAMEWM_BRAVE_PASSWORD_STORE".into(), "kwallet6".into()),
        ]);
        assert_eq!(resolve_kwallet_version(&selected, true, true), Some("6"));
        let libsecret = BTreeMap::from([(
            "FLAMEWM_BRAVE_PASSWORD_STORE".into(),
            "gnome-libsecret".into(),
        )]);
        assert_eq!(resolve_kwallet_version(&libsecret, true, false), None);
    }

    #[test]
    fn session_keeps_flamewm_identity_without_inventing_kde_wallet_version() {
        let paths = default_paths_from_home("/usr/bin", "/home/test");
        let env = BTreeMap::from([
            ("KDE_SESSION_VERSION".to_owned(), "7".to_owned()),
            ("XDG_CURRENT_DESKTOP".to_owned(), "KDE".to_owned()),
        ]);
        let plan = build_launch_plan(&paths, &[], &env);
        let session = &plan.processes[0].environment;
        assert_eq!(
            session.get("XDG_CURRENT_DESKTOP").map(String::as_str),
            Some("FlameWM")
        );
        assert_ne!(
            session.get("KDE_SESSION_VERSION").map(String::as_str),
            Some("7")
        );
    }

    #[test]
    fn launch_plan_contains_only_flamewm_owned_processes() {
        let paths = default_paths_from_home("/usr/bin", "/home/test");
        let plan = build_launch_plan(&paths, &[], &BTreeMap::new());
        assert_eq!(plan.processes.len(), 3);
        assert_eq!(plan.processes[0].role, SessionRole::WindowManager);
        assert_eq!(plan.processes[0].program, paths.flamewm_binary());
        assert_eq!(plan.processes[1].role, SessionRole::Desktop);
        assert_eq!(plan.processes[1].program, paths.desktop_binary());
        assert_eq!(plan.processes[1].arguments, Vec::<String>::new());
        assert!(plan.processes[1].required);
        assert_eq!(plan.processes[2].role, SessionRole::Shell);
        assert_eq!(plan.processes[2].program, paths.shell_binary());
        assert_eq!(plan.processes[2].arguments, Vec::<String>::new());
        assert!(plan.processes[2].required);
    }

    #[test]
    fn config_directory_override_cannot_be_injected_through_passthrough() {
        let paths = default_paths_from_home("/usr/bin", "/home/test");
        let passthrough = vec!["--replace".to_owned(), "--config-dir=/tmp/evil".to_owned()];
        let plan = build_launch_plan(&paths, &passthrough, &BTreeMap::new());
        assert_eq!(plan.processes[0].arguments, vec!["--replace"]);
    }

    #[test]
    fn bundled_cursor_authority_defaults_and_overrides() {
        let root = cursor_test_root("launch-plan");
        let mut paths = default_paths_from_home("/usr/bin", "/home/test");
        paths.data_root = root.clone();
        let system_data = cursor_test_root("isolated-data");
        let cursor_path = format!(
            "{}:/home/test/.local/share/icons:{}/icons",
            root.join("icons").display(),
            system_data.display()
        );
        let existing = BTreeMap::from([(
            "XDG_DATA_DIRS".to_owned(),
            system_data.display().to_string(),
        )]);
        let plan = build_launch_plan(&paths, &[], &existing);
        let environment = &plan.processes[0].environment;
        assert_eq!(
            environment.get("XCURSOR_THEME").map(String::as_str),
            Some(CURSOR_THEME_ID)
        );
        assert_eq!(
            environment.get("XCURSOR_SIZE").map(String::as_str),
            Some(CURSOR_SIZE)
        );
        assert_eq!(
            environment.get("XCURSOR_PATH").map(String::as_str),
            Some(cursor_path.as_str())
        );
        // No host inherit: an existing XCURSOR_PATH is ignored.
        let mut existing = existing.clone();
        existing.insert("XCURSOR_PATH".to_owned(), "/tmp/host".to_owned());
        let plan = build_launch_plan(&paths, &[], &existing);
        assert_eq!(
            plan.processes[0]
                .environment
                .get("XCURSOR_PATH")
                .map(String::as_str),
            Some(cursor_path.as_str())
        );
        let system = cursor_test_root("system-icons");
        install_cursor_theme(&system.join("icons"), "AptCursor");
        let existing = BTreeMap::from([
            ("XDG_DATA_DIRS".to_owned(), system.display().to_string()),
            ("XCURSOR_THEME".to_owned(), "AptCursor".to_owned()),
        ]);
        let plan = build_launch_plan(&paths, &[], &existing);
        let environment = &plan.processes[0].environment;
        assert_eq!(
            environment.get("XCURSOR_THEME").map(String::as_str),
            Some("AptCursor")
        );
        assert!(environment["XCURSOR_PATH"].contains(&system.join("icons").display().to_string()));
        let _ = std::fs::remove_dir_all(&system);
        // FLAMEWM_CURSOR_* explicit overrides win.
        let mut existing = BTreeMap::new();
        existing.insert("FLAMEWM_CURSOR_THEME".to_owned(), "Custom".to_owned());
        existing.insert("FLAMEWM_CURSOR_SIZE".to_owned(), "32".to_owned());
        existing.insert(
            "FLAMEWM_CURSOR_PATH".to_owned(),
            "/tmp/stage/icons".to_owned(),
        );
        let plan = build_launch_plan(&paths, &[], &existing);
        let environment = &plan.processes[0].environment;
        // "Custom" is not installed under the stage root: falls back to the bundled theme.
        assert_eq!(
            environment.get("XCURSOR_THEME").map(String::as_str),
            Some(CURSOR_THEME_ID)
        );
        assert_eq!(
            environment.get("XCURSOR_SIZE").map(String::as_str),
            Some("32")
        );
        assert_eq!(
            environment.get("XCURSOR_PATH").map(String::as_str),
            Some("/tmp/stage/icons")
        );
        let _ = std::fs::remove_dir_all(&system_data);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_theme_core_and_library_fallback_are_explicit() {
        let paths = default_paths_from_home("/usr/bin", "/home/test");
        let plan = build_launch_plan_with_cursor_library_path(
            &paths,
            &[],
            &BTreeMap::new(),
            Some("/usr/share/icons:/usr/share/pixmaps"),
        );
        let environment = &plan.processes[0].environment;
        assert_eq!(
            environment.get("XCURSOR_THEME_CORE").map(String::as_str),
            Some("1")
        );
        let cursor_path = environment.get("XCURSOR_PATH").expect("cursor search path");
        assert!(cursor_path.contains("/usr/share/icons"));
        assert!(cursor_path.ends_with("/usr/share/pixmaps"));
    }

    #[test]
    fn appearance_overlay_marks_identity_without_toolkit_defaults() {
        let overlay = appearance_environment(AppearanceMode::Dark);
        assert_eq!(
            overlay.get("FLAMEWM_APPEARANCE").map(String::as_str),
            Some("dark")
        );
        assert_eq!(
            overlay.get("FLAMEWM_DARK_MODE").map(String::as_str),
            Some("1")
        );
        // No generic toolkit defaults even when themes are installed.
        assert!(!overlay.contains_key("GTK_THEME"));
        assert!(!overlay.contains_key("QT_STYLE_OVERRIDE"));
        // Negative control: Breeze plugin present + QML absent + no explicit
        // override => no QT_STYLE_OVERRIDE exported for ordinary plans.
        let existing = BTreeMap::new();
        let paths = default_paths_from_home("/usr/bin", "/home/test");
        let plan =
            build_launch_plan_with_appearance(&paths, &[], &existing, None, AppearanceMode::Dark);
        let environment = &plan.processes[0].environment;
        assert_eq!(
            environment.get("FLAMEWM_DESKTOP").map(String::as_str),
            Some("FlameWM")
        );
        assert_eq!(
            environment.get("FLAMEWM_APPEARANCE").map(String::as_str),
            Some("dark")
        );
        assert!(!environment.contains_key("GTK_THEME"));
        assert!(!environment.contains_key("QT_STYLE_OVERRIDE"));
        // Decision (B6): the Qt channel is an owned kdeglobals reached through
        // an XDG_CONFIG_DIRS prefix. QT_QPA_PLATFORMTHEME=kde is set only when
        // the KDE platform theme plugin is installed and no value is inherited.
        // GTK_THEME and QT_STYLE_OVERRIDE stay forbidden.
        let xdg = environment
            .get("XDG_CONFIG_DIRS")
            .expect("owned kdeglobals channel");
        assert!(xdg.starts_with(&format!("{}:", owned_appearance_root(&paths).display())));
        let expected_theme = kde_platform_theme_installed(&existing).then_some("kde");
        assert_eq!(
            environment.get("QT_QPA_PLATFORMTHEME").map(String::as_str),
            expected_theme
        );
        // An inherited explicit platform theme is never overlaid.
        let explicit = BTreeMap::from([("QT_QPA_PLATFORMTHEME".to_owned(), "fusion".to_owned())]);
        let plan =
            build_launch_plan_with_appearance(&paths, &[], &explicit, None, AppearanceMode::Dark);
        assert!(
            !plan.processes[0]
                .environment
                .contains_key("QT_QPA_PLATFORMTHEME")
        );
    }

    fn install_cursor_theme(root: &std::path::Path, name: &str) {
        let cursors = root.join(name).join("cursors");
        std::fs::create_dir_all(&cursors).expect("theme dir");
        std::fs::write(cursors.join("left_ptr"), b"Xcur").expect("pointer fixture");
    }

    fn cursor_test_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "flamewm-cursor-resolve-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("test root");
        root
    }

    #[test]
    fn cursor_theme_resolution_order_and_fallback_chain() {
        let root = cursor_test_root("chain");
        let search = root.display().to_string();
        // Nothing installed: bundled FlameWM theme.
        assert_eq!(resolve_cursor_theme("", &search), CURSOR_THEME_ID);
        assert_eq!(resolve_cursor_theme("Missing", &search), CURSOR_THEME_ID);
        // Bundled McMojave wins over the FlameWM default when present.
        install_cursor_theme(&root, MCMOJAVE_CURSOR_THEME_ID);
        assert_eq!(resolve_cursor_theme("", &search), MCMOJAVE_CURSOR_THEME_ID);
        assert_eq!(
            resolve_cursor_theme("Missing", &search),
            MCMOJAVE_CURSOR_THEME_ID
        );
        // An installed Oxygen theme wins when McMojave is missing.
        let oxygen_only = cursor_test_root("oxygen-only");
        install_cursor_theme(&oxygen_only, "Oxygen_Black");
        assert_eq!(
            resolve_cursor_theme("", &oxygen_only.display().to_string()),
            "Oxygen_Black"
        );
        assert_eq!(
            resolve_cursor_theme("Missing", &oxygen_only.display().to_string()),
            "Oxygen_Black"
        );
        let _ = std::fs::remove_dir_all(&oxygen_only);
        // Explicit installed theme wins over both.
        install_cursor_theme(&root, "Custom");
        assert_eq!(resolve_cursor_theme("Custom", &search), "Custom");
        // A directory without a cursors/ subdir is a broken theme and never selected.
        std::fs::create_dir_all(root.join("Broken")).expect("broken theme");
        assert_eq!(
            resolve_cursor_theme("Broken", &search),
            MCMOJAVE_CURSOR_THEME_ID
        );
        // Path-like names never resolve outside the search roots.
        assert_eq!(
            resolve_cursor_theme("../Custom", &search),
            MCMOJAVE_CURSOR_THEME_ID
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_reload_decision_compares_resolved_theme_to_applied_theme() {
        let root = cursor_test_root("reload-decision");
        let search = root.display().to_string();
        install_cursor_theme(&root, "Custom");
        assert_eq!(
            cursor_theme_reload(None, "Custom", &search).as_deref(),
            Some("Custom")
        );
        assert_eq!(cursor_theme_reload(Some("Custom"), "Custom", &search), None);

        let fallback = resolve_cursor_theme("Missing", &search);
        assert_eq!(
            cursor_theme_reload(Some(&fallback), "Missing", &search),
            None
        );
        assert_eq!(
            cursor_theme_reload(Some("Other"), "Missing", &search).as_deref(),
            Some(fallback.as_str())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_families_reject_lookalikes_and_variant_selection_is_stable() {
        assert_eq!(
            cursor_theme_family("McMojave-cursors"),
            Some(CursorThemeFamily::McMojave)
        );
        assert_eq!(
            cursor_theme_family("McMojave_Dark"),
            Some(CursorThemeFamily::McMojave)
        );
        assert_eq!(
            cursor_theme_family("Oxygen_Black"),
            Some(CursorThemeFamily::Oxygen)
        );
        assert_eq!(
            cursor_theme_family("oxy-white"),
            Some(CursorThemeFamily::Oxygen)
        );
        for unknown in [
            "McMojaveX",
            "Oxygenated",
            "oxywhatever",
            "Bibata",
            "../Oxygen",
        ] {
            assert_eq!(cursor_theme_family(unknown), None);
        }
        let root = cursor_test_root("variants");
        install_cursor_theme(&root, "Oxygen_White");
        install_cursor_theme(&root, "Oxygen_Black");
        let search = root.display().to_string();
        assert_eq!(resolve_cursor_theme("", &search), "Oxygen_Black");
        install_cursor_theme(&root, "McMojave_Dark");
        assert_eq!(resolve_cursor_theme("", &search), "McMojave_Dark");
        install_cursor_theme(&root, MCMOJAVE_CURSOR_THEME_ID);
        assert_eq!(resolve_cursor_theme("", &search), MCMOJAVE_CURSOR_THEME_ID);
        assert_eq!(
            resolve_cursor_theme("Oxygen_White", &search),
            "Oxygen_White"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_theme_search_path_accepts_multiple_roots() {
        let first = cursor_test_root("multi-a");
        let second = cursor_test_root("multi-b");
        install_cursor_theme(&second, "Custom");
        let search = format!("{}:{}", first.display(), second.display());
        assert_eq!(resolve_cursor_theme("Custom", &search), "Custom");
        assert_eq!(
            resolve_cursor_theme("Custom", &first.display().to_string()),
            CURSOR_THEME_ID
        );
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    fn session_env_and_resolver_agree_on_installed_bundled_theme() {
        let root = cursor_test_root("env");
        // Session search root is <data_root>/icons.
        install_cursor_theme(&root.join("icons"), MCMOJAVE_CURSOR_THEME_ID);
        let paths = SessionPaths {
            bindir: "/usr/bin".into(),
            config_home: "/home/test/.config".into(),
            data_root: root.clone(),
        };
        let plan = build_launch_plan(&paths, &[], &BTreeMap::new());
        let environment = &plan.processes[0].environment;
        assert_eq!(
            environment.get("XCURSOR_THEME").map(String::as_str),
            Some(MCMOJAVE_CURSOR_THEME_ID)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restart_policy_is_bounded() {
        let mut supervisor = ProcessSupervisorPolicy::default();
        let mut delays = Vec::new();
        for _ in 0..5 {
            if let SupervisorDecision::RestartAfter(delay) =
                supervisor.child_exited(ChildExit::ExitCode(1))
            {
                delays.push(delay);
            }
        }
        assert_eq!(delays, vec![500, 1_000, 2_000, 4_000, 8_000]);
        assert_eq!(
            supervisor.child_exited(ChildExit::ExitCode(1)),
            SupervisorDecision::GiveUp
        );
    }
}

/// Apply computed session environment to the current process. Centralizes
/// the single `std::env::set_var` owner (Rust 2024 marks it `unsafe`
/// because it races with `getenv` on other threads; call before spawning
/// threads/children, matching long-standing launcher practice).
#[allow(unsafe_code)]
pub fn apply_current_process_env(vars: &std::collections::BTreeMap<String, String>) {
    for (key, value) in vars {
        // SAFETY: launcher single-threaded setup before thread spawn.
        unsafe { std::env::set_var(key, value) };
    }
}
