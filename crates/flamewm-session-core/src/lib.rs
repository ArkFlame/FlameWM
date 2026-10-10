//! FlameWM session launch and supervision policy.
//!
//! This crate is intentionally independent. It plans FlameWM-owned processes only; process
//! spawning, signals, and parent-death handling remain adapter responsibilities.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flamewm_api::settings::AppearanceMode;

/// Bundled cursor authority: FlameWM-Breeze-Dark (Breeze Dark Xcursor files
/// under a FlameWM-owned theme id), size 24. Never claims KDE identity.
pub const CURSOR_THEME_ID: &str = "FlameWM-Breeze-Dark";
pub const CURSOR_SIZE: &str = "24";

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

    #[must_use]
    pub fn cursor_theme_dir(&self) -> PathBuf {
        self.cursor_search_root().join(CURSOR_THEME_ID)
    }

    #[must_use]
    pub fn cursor_path(&self) -> PathBuf {
        self.cursor_search_root()
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
    let overlay = appearance_environment(appearance);
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

fn flame_environment(
    paths: &SessionPaths,
    existing_env: &BTreeMap<String, String>,
    cursor_library_path: Option<&str>,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    // Bundled authority first: explicit FLAMEWM_CURSOR_* wins, then existing
    // XCURSOR_*, then the bundled theme/size defaults.
    environment.insert(
        "XCURSOR_THEME".to_owned(),
        existing_env
            .get("FLAMEWM_CURSOR_THEME")
            .filter(|value| !value.is_empty())
            .cloned()
            .or_else(|| {
                existing_env
                    .get("XCURSOR_THEME")
                    .filter(|value| !value.is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| CURSOR_THEME_ID.to_owned()),
    );
    environment.insert(
        "XCURSOR_SIZE".to_owned(),
        existing_env
            .get("FLAMEWM_CURSOR_SIZE")
            .filter(|value| !value.is_empty())
            .cloned()
            .or_else(|| {
                existing_env
                    .get("XCURSOR_SIZE")
                    .filter(|value| !value.is_empty())
                    .cloned()
            })
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
        existing_env
            .get("FLAMEWM_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| paths.config_dir().display().to_string()),
    );
    environment.insert("FLAMEWM_DESKTOP".to_owned(), "FlameWM".to_owned());
    environment.insert(
        "XDG_CURRENT_DESKTOP".to_owned(),
        existing_env
            .get("XDG_CURRENT_DESKTOP")
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| "FlameWM".to_owned()),
    );

    // Search root is the parent of the bundled theme dir (<root>/icons);
    // never inherit the host XCURSOR_PATH.
    let search_root = existing_env
        .get("FLAMEWM_CURSOR_PATH")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| paths.cursor_path().display().to_string());
    let combined_cursor_path = cursor_library_path
        .filter(|value| !value.is_empty())
        .map_or(search_root.clone(), |existing| {
            format!("{search_root}:{existing}")
        });
    environment.insert("XCURSOR_PATH".to_owned(), combined_cursor_path);
    environment
}

/// Appearance markers for the env producer side only: FLAMEWM_APPEARANCE and
/// FLAMEWM_DARK_MODE. Never synthesizes generic GTK_THEME/QT_STYLE_OVERRIDE
/// defaults; explicit inherited toolkit values pass through untouched.
#[must_use]
pub fn appearance_environment(appearance: AppearanceMode) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    let dark = appearance.prefers_dark();
    environment.insert(
        "FLAMEWM_APPEARANCE".to_owned(),
        appearance.as_str().to_owned(),
    );
    environment.insert(
        "FLAMEWM_DARK_MODE".to_owned(),
        if dark { "1".to_owned() } else { "0".to_owned() },
    );
    environment
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

pub type DesktopSupervisorPolicy = ProcessSupervisorPolicy;

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
    fn launch_plan_contains_only_flamewm_owned_processes() {
        let paths = default_paths_from_home("/usr/bin", "/home/test");
        let plan = build_launch_plan(&paths, &[], &BTreeMap::new());
        assert_eq!(plan.processes.len(), 3);
        assert_eq!(plan.processes[0].role, SessionRole::WindowManager);
        assert_eq!(plan.processes[0].program, paths.flamewm_binary());
        assert_eq!(
            plan.processes[1].role,
            SessionRole::Desktop
        );
        assert_eq!(
            plan.processes[1].program,
            paths.desktop_binary()
        );
        assert_eq!(plan.processes[1].arguments, Vec::<String>::new());
        assert!(plan.processes[1].required);
        assert_eq!(plan.processes[2].role, SessionRole::Shell);
        assert_eq!(
            plan.processes[2].program,
            paths.shell_binary()
        );
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
        let paths = default_paths_from_home("/usr/bin", "/home/test");
        let plan = build_launch_plan(&paths, &[], &BTreeMap::new());
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
            Some("/usr/share/icons")
        );
        // No host inherit: an existing XCURSOR_PATH is ignored.
        let mut existing = BTreeMap::new();
        existing.insert("XCURSOR_PATH".to_owned(), "/tmp/host".to_owned());
        let plan = build_launch_plan(&paths, &[], &existing);
        assert_eq!(
            plan.processes[0]
                .environment
                .get("XCURSOR_PATH")
                .map(String::as_str),
            Some("/usr/share/icons")
        );
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
        assert_eq!(
            environment.get("XCURSOR_THEME").map(String::as_str),
            Some("Custom")
        );
        assert_eq!(
            environment.get("XCURSOR_SIZE").map(String::as_str),
            Some("32")
        );
        assert_eq!(
            environment.get("XCURSOR_PATH").map(String::as_str),
            Some("/tmp/stage/icons")
        );
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
        assert!(
            environment
                .get("XCURSOR_PATH")
                .is_some_and(|value| value.ends_with("/usr/share/icons:/usr/share/pixmaps"))
        );
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
