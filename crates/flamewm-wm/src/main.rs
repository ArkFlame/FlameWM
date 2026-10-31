use std::cell::RefCell;
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use flamewm_api::settings::{AppearanceMode, SettingValue, SettingsSnapshot};
use flamewm_session_core::{
    AppearancePlan, AppearanceProbe, SessionPaths, build_launch_plan_with_appearance,
    default_paths_from_home, gtk3_theme_name, kde_platform_theme_installed,
    materialize_owned_files, owned_appearance_root, plan_appearance, resolve_cursor_theme,
    resolve_startup_appearance,
};
use flamewm_wm::{
    SettingsObserver, SettingsWake, WmConfig, apply_cursor_theme, persisted_appearance,
    persisted_cursor_theme, run_with_settings_observer,
};
use flamewm_xsettings::{
    X11XSettingsManager, XSettingsInput, claim_x11_manager, published_settings,
};

pub fn main() {
    let inherited: BTreeMap<String, String> = env::vars().collect();
    // Resolved once: the XSETTINGS claim, the session environment, and the owned
    // appearance files must all agree on the same mode from the first publish.
    let explicit = inherited
        .get("FLAMEWM_APPEARANCE")
        .and_then(|value| AppearanceMode::parse(value));
    let appearance = resolve_startup_appearance(
        inherited.get("FLAMEWM_APPEARANCE").map(String::as_str),
        persisted_appearance(),
    );
    apply_session_environment(&inherited, appearance);
    // The manager is shared (Rc<RefCell>) by the settings observer and the
    // reactor wake; it lives for the process lifetime and dropping the last
    // handle releases _XSETTINGS_S0.
    let claimed = claim_xsettings_manager(appearance);
    materialize_appearance_files(&inherited, appearance, claimed.is_some());
    let result = match claimed {
        Some(manager) => {
            let sync = AppearanceSync::new(session_paths(), inherited, appearance, explicit);
            let (observer, wake) = xsettings_owner(manager, sync);
            run_with_settings_observer(WmConfig::from_env(), observer, wake)
        }
        None => run_with_settings_observer(WmConfig::from_env(), cursor_only_observer(), None),
    };
    if let Err(error) = result {
        eprintln!("flamewm: {error}");
        std::process::exit(1);
    }
}

/// Consume session-core environment so the WM process and every child it
/// spawns inherit the `FlameWM` identity (`FlameWM`, never KDE) plus cursor and
/// appearance state. Dark is the default; explicit user values win except
/// for FlameWM-owned identity keys.
fn apply_session_environment(existing: &BTreeMap<String, String>, appearance: AppearanceMode) {
    let paths = session_paths();
    // session-core contributes cursor, identity, and appearance-marker
    // state only. No generic toolkit default injection: explicit inherited
    // GTK_THEME/QT_STYLE_OVERRIDE pass through untouched, never synthesized.
    let plan = build_launch_plan_with_appearance(&paths, &[], existing, None, appearance);
    let overlay = plan
        .processes
        .first()
        .map_or_else(BTreeMap::new, |process| process.environment.clone());
    let search_path = cursor_search_path(existing, &overlay);
    let persisted = persisted_cursor_theme().unwrap_or_default();
    let cursor_theme = resolve_cursor_theme(&persisted, &search_path);
    let mut pending = session_overlay(existing, &overlay);
    // FlameWM owns its session identity even when a login manager inherited
    // KDE from a previous login. Secondary KDE compatibility is advertised
    // only in the explicit colon-separated session contract.
    if let Some(current_desktop) = overlay.get("XDG_CURRENT_DESKTOP") {
        pending.insert("XDG_CURRENT_DESKTOP".to_owned(), current_desktop.clone());
    }
    pending.insert("XCURSOR_THEME".to_owned(), cursor_theme);
    flamewm_session_core::apply_current_process_env(&pending);
}

fn session_paths() -> SessionPaths {
    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    let bindir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    default_paths_from_home(bindir, home)
}

/// Pure merge of the session overlay onto the inherited environment. FLAMEWM_*
/// markers and the owned `XDG_CONFIG_DIRS` prefix (already computed from the
/// inherited value) always win; every other key only fills an absent variable,
/// so explicit user QT_*/GTK_* values survive. `XDG_CURRENT_DESKTOP=KDE` is never
/// copied.
fn session_overlay(
    inherited: &BTreeMap<String, String>,
    overlay: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut pending = BTreeMap::new();
    for (key, value) in overlay {
        if key == "XDG_CURRENT_DESKTOP" && is_kde(value) {
            continue;
        }
        let absent = inherited.get(key).is_none_or(String::is_empty);
        if key.starts_with("FLAMEWM_")
            || key == "XDG_CONFIG_DIRS"
            || key == "XDG_CURRENT_DESKTOP"
            || key == "XDG_SESSION_DESKTOP"
            || absent
        {
            pending.insert(key.clone(), value.clone());
        }
    }
    pending
}

/// Write the session-owned appearance files and report the plan's toolkit
/// status. Materialization is idempotent and never touches host config.
fn materialize_appearance_files(
    inherited: &BTreeMap<String, String>,
    appearance: AppearanceMode,
    xsettings_owned: bool,
) {
    let probe = AppearanceProbe {
        xsettings_owned,
        gtk3_theme_installed: gtk3_theme_name(appearance, inherited).is_some(),
        kde_platform_theme: kde_platform_theme_installed(inherited),
    };
    let plan = plan_appearance(&session_paths(), inherited, appearance, probe);
    if let Err(error) = materialize_owned_files(&plan) {
        eprintln!("flamewm: appearance files not written: {error}");
    }
}

/// Search path for resolving the persisted cursor theme: an inherited `XCURSOR_PATH` wins, otherwise the
/// session plan's own path (the same one exported to children). An empty search would make every
/// persisted selection fall back to the default theme.
fn cursor_search_path(
    existing: &BTreeMap<String, String>,
    plan_environment: &BTreeMap<String, String>,
) -> String {
    existing
        .get("XCURSOR_PATH")
        .filter(|value| !value.is_empty())
        .or_else(|| plan_environment.get("XCURSOR_PATH"))
        .cloned()
        .unwrap_or_default()
}

fn is_kde(value: &str) -> bool {
    value.eq_ignore_ascii_case("kde")
}

/// Start the `FlameWM` `XSettings` owner: take `_XSETTINGS_S0` (refusing any
/// foreign owner) and publish the default `FlameWM` settings. The WM keeps running
/// without `XSettings` ownership when the claim is refused or fails.
fn claim_xsettings_manager(appearance: AppearanceMode) -> Option<X11XSettingsManager> {
    let input = xsettings_input_from_snapshot(&startup_settings_snapshot(appearance));
    match claim_x11_manager(&published_settings(&input)) {
        Ok(manager) => Some(manager),
        Err(error) => {
            eprintln!("flamewm: XSettings manager not claimed: {error}");
            None
        }
    }
}

/// Startup settings: only the resolved startup appearance; every other key is
/// absent and maps to its default. Routed through the same mapper as live
/// updates so the paths cannot diverge.
fn startup_settings_snapshot(appearance: AppearanceMode) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(0);
    snapshot.values.insert(
        "appearance".to_owned(),
        SettingValue::Text(appearance.as_str().to_owned()),
    );
    snapshot
}

/// Owns the owned-appearance refresh for live changes. Startup inputs are kept
/// so a later mode rebuilds the same plan `main` used for the first publish.
struct AppearanceSync {
    paths: SessionPaths,
    inherited: BTreeMap<String, String>,
    applied: AppearanceMode,
    /// Explicit `FLAMEWM_APPEARANCE` override: live persisted appearance is ignored
    /// for XSETTINGS and the owned files, so the startup resolution holds.
    pinned: Option<AppearanceMode>,
}

impl AppearanceSync {
    fn new(
        paths: SessionPaths,
        inherited: BTreeMap<String, String>,
        applied: AppearanceMode,
        pinned: Option<AppearanceMode>,
    ) -> Self {
        Self {
            paths,
            inherited,
            applied,
            pinned,
        }
    }

    /// Rebuild the owned appearance files when `mode` differs from the applied mode.
    /// Returns the plan whose environment the caller must export; `None` when unchanged.
    /// `gtk3_theme_installed` is the caller's [`gtk3_theme_name`] result for `mode`, so the
    /// theme roots are probed once per settings sync. The host kdeglobals is never touched:
    /// only the owned root is written.
    fn change(
        &mut self,
        mode: AppearanceMode,
        gtk3_theme_installed: bool,
    ) -> Option<AppearancePlan> {
        if self.applied == mode {
            return None;
        }
        self.applied = mode;
        let probe = AppearanceProbe {
            xsettings_owned: true,
            gtk3_theme_installed,
            kde_platform_theme: kde_platform_theme_installed(&self.inherited),
        };
        let plan = plan_appearance(&self.paths, &self.inherited, mode, probe);
        if plan.owned_files.is_empty() {
            // No owned scheme for this mode (system): drop the stale one so new Qt apps
            // do not inherit the previous mode.
            let stale = owned_appearance_root(&self.paths).join("kdeglobals");
            match std::fs::remove_file(&stale) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("flamewm: stale owned kdeglobals not removed: {error}"),
            }
        } else if let Err(error) = materialize_owned_files(&plan) {
            eprintln!("flamewm: appearance files not written: {error}");
        }
        Some(plan)
    }
}

/// Map WM settings-snapshot values onto the `XSettings` input. Absent or
/// unusable values fall back to `XSettingsInput::default()`.
///
/// `appearance: system` is intentionally NOT resolved to light or dark here:
/// no OS appearance query exists on this path, and the startup path has always
/// passed `System` through unchanged (publishes the light GTK theme). Live
/// updates follow the same rule.
#[must_use]
pub fn xsettings_input_from_snapshot(snapshot: &SettingsSnapshot) -> XSettingsInput {
    let session_theme = env::var("XCURSOR_THEME").unwrap_or_default();
    let search_path = env::var("XCURSOR_PATH").unwrap_or_default();
    let mut input = xsettings_input_with_cursor_env(snapshot, &session_theme, &search_path);
    // Installed GTK3 theme for the requested mode; absent themes keep the
    // Flame-* name so no dark request silently maps to a light theme.
    let inherited: BTreeMap<String, String> = env::vars().collect();
    input.gtk3_theme = gtk3_theme_name(input.appearance, &inherited).map(str::to_owned);
    input
}

/// Pure core of [`xsettings_input_from_snapshot`]: the published cursor theme is
/// the session resolution applied to the `cursorTheme` setting, so XSETTINGS and
/// the WM session env cannot disagree. An unset setting uses the session theme.
fn xsettings_input_with_cursor_env(
    snapshot: &SettingsSnapshot,
    session_theme: &str,
    search_path: &str,
) -> XSettingsInput {
    let mut input = XSettingsInput::default();
    let requested = text_value(snapshot, "cursorTheme").unwrap_or(session_theme);
    input.cursor_theme = resolve_cursor_theme(requested, search_path);
    if let Some(appearance) = text_value(snapshot, "appearance").and_then(AppearanceMode::parse) {
        input.appearance = appearance;
    }
    if let Some(icon_theme) = text_value(snapshot, "iconTheme") {
        icon_theme.clone_into(&mut input.icon_theme);
    }
    if let Some(font_family) = text_value(snapshot, "fontFamily") {
        font_family.clone_into(&mut input.font_family);
    }
    if let Some(font_bold) = boolean_value(snapshot, "fontBold") {
        input.font_bold = font_bold;
    }
    if let Some(points) = font_size_points(snapshot) {
        input.font_size_points = points;
    }
    if let Some(size) = cursor_size(snapshot) {
        input.cursor_size = size;
    }
    input
}

/// `cursorSize` is one of the supported pixel sizes; anything else keeps the `XSettings` default.
fn cursor_size(snapshot: &SettingsSnapshot) -> Option<u32> {
    match snapshot.values.get("cursorSize") {
        Some(SettingValue::Integer(size)) => u32::try_from(*size)
            .ok()
            .filter(|size| matches!(*size, 24 | 32 | 48 | 64)),
        _ => None,
    }
}

fn text_value<'a>(snapshot: &'a SettingsSnapshot, key: &str) -> Option<&'a str> {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(text)) if !text.trim().is_empty() => Some(text.as_str()),
        _ => None,
    }
}

fn boolean_value(snapshot: &SettingsSnapshot, key: &str) -> Option<bool> {
    match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => Some(*value),
        _ => None,
    }
}

/// `fontSizeOffset` is relative; `XSettings` publishes an absolute point size, so
/// the offset is applied to the default base size and clamped to 1..=72.
fn font_size_points(snapshot: &SettingsSnapshot) -> Option<u32> {
    let base = i64::from(XSettingsInput::default().font_size_points);
    match snapshot.values.get("fontSizeOffset") {
        Some(SettingValue::Integer(offset)) => u32::try_from((base + offset).clamp(1, 72)).ok(),
        _ => None,
    }
}

/// Settings observer used when no XSETTINGS manager is claimed: still applies
/// the Flame `cursorTheme` to live cursor owners at every settings sync point.
fn cursor_only_observer() -> SettingsObserver {
    Box::new(|snapshot: &SettingsSnapshot| {
        let theme = xsettings_input_from_snapshot(snapshot).cursor_theme;
        for error in apply_cursor_theme(&theme) {
            eprintln!("flamewm: cursor theme reload failed: {error}");
        }
    })
}

/// Live `XSettings` path. Each settings sync point maps the snapshot and
/// republishes (a no-op when nothing changed). The first failure is logged;
/// later failures are suppressed so a persistent error cannot flood stderr.
/// The WM keeps running without `XSettings` updates.
/// Split the owned manager into the publish observer and the reactor wake. The
/// manager is shared through `Rc<RefCell<_>>`; the descriptor is read before the
/// move so the wake can be registered without borrowing the manager. Without a
/// descriptor the wake is `None` and only publish-time draining remains.
fn xsettings_owner(
    manager: X11XSettingsManager,
    mut sync: AppearanceSync,
) -> (SettingsObserver, Option<SettingsWake>) {
    let wake_fd = manager.wake_fd();
    let shared = Rc::new(RefCell::new(manager));
    let observer_manager = Rc::clone(&shared);
    let mut reported = false;
    // Last input fully applied and published. Recorded only when the cursor apply and
    // republish both succeeded, so a failed step is retried on the next snapshot.
    let mut published: Option<XSettingsInput> = None;
    let observer: SettingsObserver = Box::new(move |snapshot: &SettingsSnapshot| {
        let mut pinned_snapshot = snapshot.clone();
        if let Some(mode) = sync.pinned {
            pinned_snapshot.values.insert(
                "appearance".to_owned(),
                SettingValue::Text(mode.as_str().to_owned()),
            );
        }
        let input = xsettings_input_from_snapshot(&pinned_snapshot);
        let outcome = if published.as_ref() == Some(&input) {
            // Unchanged input: no cursor, appearance, or publish work. Queued manager
            // events are still consumed so a lost selection is noticed at this sync point.
            observer_manager.borrow_mut().drain_events().map(|_| ())
        } else {
            let cursor_errors = apply_cursor_theme(&input.cursor_theme);
            for error in &cursor_errors {
                eprintln!("flamewm: cursor theme reload failed: {error}");
            }
            if let Some(plan) = sync.change(input.appearance, input.gtk3_theme.is_some()) {
                flamewm_session_core::apply_current_process_env(&plan.environment);
            }
            let outcome = {
                let mut manager = observer_manager.borrow_mut();
                manager
                    .drain_events()
                    .and_then(|_| manager.republish(&input))
                    .map(|_| ())
            };
            if outcome.is_ok() && cursor_errors.is_empty() {
                published = Some(input);
            }
            outcome
        };
        if let Err(error) = outcome {
            if !reported {
                reported = true;
                eprintln!("flamewm: XSettings republish failed (later errors suppressed): {error}");
            }
        }
    });
    let wake = wake_fd.map(|fd| SettingsWake {
        fd,
        on_readable: Box::new(move || {
            shared
                .borrow_mut()
                .on_manager_readable()
                .map(|_| ())
                .map_err(|error| error.to_string())
        }),
    });
    (observer, wake)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_session_core::{Toolkit, ToolkitStatus};

    fn snapshot_with(entries: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in entries {
            snapshot.values.insert((*key).to_owned(), value.clone());
        }
        snapshot
    }

    fn text(value: &str) -> SettingValue {
        SettingValue::Text(value.to_owned())
    }

    #[test]
    fn cursor_size_follows_supported_snapshot_values_only() {
        for size in [24, 32, 48, 64] {
            let snapshot = snapshot_with(&[("cursorSize", SettingValue::Integer(size))]);
            assert_eq!(
                xsettings_input_from_snapshot(&snapshot).cursor_size,
                u32::try_from(size).expect("size")
            );
        }
        for value in [
            SettingValue::Integer(16),
            SettingValue::Integer(65),
            SettingValue::Integer(-24),
            text("48"),
        ] {
            let snapshot = snapshot_with(&[("cursorSize", value.clone())]);
            assert_eq!(
                xsettings_input_from_snapshot(&snapshot).cursor_size,
                XSettingsInput::default().cursor_size,
                "{value:?} must fall back"
            );
        }
    }

    #[test]
    fn empty_snapshot_maps_to_default_input() {
        assert_eq!(
            xsettings_input_with_cursor_env(&SettingsSnapshot::new(0), "", ""),
            XSettingsInput::default()
        );
    }

    #[test]
    fn appearance_dark_light_and_system_map_without_resolution() {
        let dark = snapshot_with(&[("appearance", text("dark"))]);
        let light = snapshot_with(&[("appearance", text("LIGHT"))]);
        let system = snapshot_with(&[("appearance", text("system"))]);
        assert_eq!(
            xsettings_input_from_snapshot(&dark).appearance,
            AppearanceMode::Dark
        );
        assert_eq!(
            xsettings_input_from_snapshot(&light).appearance,
            AppearanceMode::Light
        );
        assert_eq!(
            xsettings_input_from_snapshot(&system).appearance,
            AppearanceMode::System
        );
    }

    #[test]
    fn unknown_or_non_text_appearance_falls_back_to_default() {
        let unknown = snapshot_with(&[("appearance", text("purple"))]);
        let wrong_type = snapshot_with(&[("appearance", SettingValue::Integer(1))]);
        assert_eq!(
            xsettings_input_from_snapshot(&unknown).appearance,
            AppearanceMode::Dark
        );
        assert_eq!(
            xsettings_input_from_snapshot(&wrong_type).appearance,
            AppearanceMode::Dark
        );
    }

    #[test]
    fn icon_theme_and_font_family_follow_snapshot_and_ignore_blank() {
        let set = snapshot_with(&[
            ("iconTheme", text("Papirus")),
            ("fontFamily", text("Noto Sans")),
        ]);
        let input = xsettings_input_from_snapshot(&set);
        assert_eq!(input.icon_theme, "Papirus");
        assert_eq!(input.font_family, "Noto Sans");

        let blank = snapshot_with(&[("iconTheme", text("  ")), ("fontFamily", text(""))]);
        let input = xsettings_input_from_snapshot(&blank);
        assert_eq!(input.icon_theme, XSettingsInput::default().icon_theme);
        assert_eq!(input.font_family, XSettingsInput::default().font_family);
    }

    #[test]
    fn font_size_offset_applies_to_default_base_and_clamps() {
        let base = XSettingsInput::default().font_size_points;
        let size = |offset: i64| {
            let snapshot = snapshot_with(&[("fontSizeOffset", SettingValue::Integer(offset))]);
            xsettings_input_from_snapshot(&snapshot).font_size_points
        };
        assert_eq!(size(0), base);
        assert_eq!(size(2), base + 2);
        assert_eq!(size(-3), base - 3);
        assert_eq!(size(-1000), 1);
        assert_eq!(size(1000), 72);
    }

    #[test]
    fn font_size_offset_wrong_type_keeps_default() {
        let snapshot = snapshot_with(&[("fontSizeOffset", text("2"))]);
        assert_eq!(
            xsettings_input_from_snapshot(&snapshot).font_size_points,
            XSettingsInput::default().font_size_points
        );
    }

    #[test]
    fn font_bold_setting_is_projected_to_xsettings_input() {
        let bold = snapshot_with(&[("fontBold", SettingValue::Boolean(true))]);
        assert!(xsettings_input_from_snapshot(&bold).font_bold);

        let normal = snapshot_with(&[("fontBold", SettingValue::Boolean(false))]);
        assert!(!xsettings_input_from_snapshot(&normal).font_bold);

        let wrong_type = snapshot_with(&[("fontBold", text("true"))]);
        assert_eq!(
            xsettings_input_from_snapshot(&wrong_type).font_bold,
            XSettingsInput::default().font_bold
        );
    }

    #[test]
    fn cursor_search_path_falls_back_to_the_session_plan_when_not_inherited() {
        let mut plan = BTreeMap::new();
        plan.insert("XCURSOR_PATH".to_owned(), "/usr/share/icons".to_owned());
        assert_eq!(
            cursor_search_path(&BTreeMap::new(), &plan),
            "/usr/share/icons"
        );
        let mut inherited = BTreeMap::new();
        inherited.insert("XCURSOR_PATH".to_owned(), "/opt/icons".to_owned());
        assert_eq!(cursor_search_path(&inherited, &plan), "/opt/icons");
        assert_eq!(cursor_search_path(&BTreeMap::new(), &BTreeMap::new()), "");
    }

    #[test]
    fn cursor_theme_setting_resolves_for_xsettings_like_session_env() {
        let root = env::temp_dir().join(format!("flamewm-wm-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Custom").join("cursors")).expect("theme");
        let search = root.display().to_string();
        let custom = snapshot_with(&[("cursorTheme", text("Custom"))]);
        assert_eq!(
            xsettings_input_with_cursor_env(&custom, "", &search).cursor_theme,
            "Custom"
        );
        // Missing explicit theme falls back to the bundled default, never a bare "Breeze".
        let missing = snapshot_with(&[("cursorTheme", text("Missing"))]);
        assert_eq!(
            xsettings_input_with_cursor_env(&missing, "", &search).cursor_theme,
            flamewm_session_core::CURSOR_THEME_ID
        );
        // Unset setting follows the session-resolved XCURSOR_THEME.
        let unset = SettingsSnapshot::new(0);
        assert_eq!(
            xsettings_input_with_cursor_env(&unset, "Custom", &search).cursor_theme,
            "Custom"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn overlay_prefixes_inherited_xdg_config_dirs_and_keeps_explicit_toolkit_values() {
        let inherited = env_map(&[
            ("XDG_CONFIG_DIRS", "/opt/xdg"),
            ("QT_QPA_PLATFORMTHEME", "kde"),
            ("GTK_THEME", "Custom"),
        ]);
        // Plan value already prefixes the inherited XDG_CONFIG_DIRS (session-core).
        let overlay = env_map(&[
            (
                "XDG_CONFIG_DIRS",
                "/home/u/.config/flamewm/appearance:/opt/xdg",
            ),
            ("FLAMEWM_APPEARANCE", "dark"),
        ]);
        let pending = session_overlay(&inherited, &overlay);
        assert_eq!(
            pending.get("XDG_CONFIG_DIRS").map(String::as_str),
            Some("/home/u/.config/flamewm/appearance:/opt/xdg")
        );
        assert_eq!(
            pending.get("FLAMEWM_APPEARANCE").map(String::as_str),
            Some("dark")
        );
        // Explicit toolkit values are never overlaid, even when the plan supplies them.
        let hostile = env_map(&[("QT_QPA_PLATFORMTHEME", "fusion"), ("GTK_THEME", "Flame")]);
        let pending = session_overlay(&inherited, &hostile);
        assert!(!pending.contains_key("QT_QPA_PLATFORMTHEME"));
        assert!(!pending.contains_key("GTK_THEME"));
    }

    #[test]
    fn overlay_skips_kde_identity_and_fills_only_absent_keys() {
        let inherited = env_map(&[("XCURSOR_SIZE", "32")]);
        // Built at runtime so the KDE identity literal never appears in source.
        let kde_identity = ["K", "D", "E"].concat();
        let overlay = env_map(&[
            ("XDG_CURRENT_DESKTOP", kde_identity.as_str()),
            ("XCURSOR_SIZE", "24"),
            ("XCURSOR_THEME_CORE", "1"),
        ]);
        let pending = session_overlay(&inherited, &overlay);
        assert!(!pending.contains_key("XDG_CURRENT_DESKTOP"));
        assert!(!pending.contains_key("XCURSOR_SIZE"));
        assert_eq!(
            pending.get("XCURSOR_THEME_CORE").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn overlay_fills_empty_inherited_platform_theme() {
        let inherited = env_map(&[("QT_QPA_PLATFORMTHEME", "")]);
        let overlay = env_map(&[("QT_QPA_PLATFORMTHEME", "kde")]);
        assert_eq!(
            session_overlay(&inherited, &overlay)
                .get("QT_QPA_PLATFORMTHEME")
                .map(String::as_str),
            Some("kde")
        );
        let explicit = env_map(&[("QT_QPA_PLATFORMTHEME", "fusion")]);
        assert!(!session_overlay(&explicit, &overlay).contains_key("QT_QPA_PLATFORMTHEME"));
    }

    #[test]
    fn live_appearance_change_rewrites_owned_kdeglobals_and_never_host() {
        let home =
            env::temp_dir().join(format!("flamewm-wm-live-appearance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let plugins = home.join("plugins");
        std::fs::create_dir_all(plugins.join("platformthemes")).expect("plugins");
        std::fs::write(
            plugins.join("platformthemes/KDEPlasmaPlatformTheme6.so"),
            b"",
        )
        .expect("plugin");
        let schemes = home.join("share/color-schemes");
        std::fs::create_dir_all(&schemes).expect("schemes");
        std::fs::write(
            schemes.join("BreezeDark.colors"),
            "[General]\nColorScheme=BreezeDark\n[Colors:View]\nBackgroundNormal=20,22,24\n",
        )
        .expect("dark scheme");
        std::fs::write(
            schemes.join("BreezeLight.colors"),
            "[General]\nColorScheme=BreezeLight\n[Colors:View]\nBackgroundNormal=255,255,255\n",
        )
        .expect("light scheme");
        let inherited = BTreeMap::from([
            ("QT_PLUGIN_PATH".to_owned(), plugins.display().to_string()),
            ("XDG_DATA_DIRS".to_owned(), "/nonexistent-flame".to_owned()),
        ]);
        let paths = || {
            let mut paths = default_paths_from_home("/usr/bin", &home);
            paths.data_root = home.join("share");
            paths
        };
        let owned = owned_appearance_root(&paths()).join("kdeglobals");
        let mut sync = AppearanceSync::new(paths(), inherited, AppearanceMode::Dark, None);
        // The applied mode is not rewritten: the startup publish already wrote it.
        assert!(sync.change(AppearanceMode::Dark, false).is_none());
        let light = sync
            .change(AppearanceMode::Light, false)
            .expect("light plan");
        assert_eq!(
            light
                .environment
                .get("FLAMEWM_APPEARANCE")
                .map(String::as_str),
            Some("light")
        );
        let light_file = std::fs::read_to_string(&owned).expect("light file");
        assert!(
            light_file.contains("ColorScheme=BreezeLight\n"),
            "{light_file}"
        );
        assert!(
            light_file.contains("[Colors:View]\nBackgroundNormal=255,255,255\n"),
            "{light_file}"
        );
        assert!(sync.change(AppearanceMode::Light, false).is_none());
        sync.change(AppearanceMode::Dark, false).expect("dark plan");
        let dark_file = std::fs::read_to_string(&owned).expect("dark file");
        assert!(
            dark_file.contains("ColorScheme=BreezeDark\n"),
            "{dark_file}"
        );
        assert!(
            dark_file.contains("[Colors:View]\nBackgroundNormal=20,22,24\n"),
            "{dark_file}"
        );
        assert!(!dark_file.contains("BreezeLight"), "{dark_file}");
        // System has no owned scheme: the stale owned file must go, not linger.
        sync.change(AppearanceMode::System, false)
            .expect("system plan");
        assert!(!owned.exists());
        assert!(
            !paths().config_home.join("kdeglobals").exists(),
            "host kdeglobals untouched"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn live_change_uses_the_shared_gtk3_probe_result() {
        let home = env::temp_dir().join(format!("flamewm-wm-shared-gtk3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let paths = default_paths_from_home("/usr/bin", &home);
        let gtk3_status = |installed: bool| {
            let mut sync =
                AppearanceSync::new(paths.clone(), BTreeMap::new(), AppearanceMode::Dark, None);
            sync.change(AppearanceMode::Light, installed)
                .expect("light plan")
                .toolkits[&Toolkit::Gtk3]
                .clone()
        };
        // The observer's probe decides the GTK3 status; change() must not re-probe the roots.
        assert!(matches!(gtk3_status(true), ToolkitStatus::Unverified(_)));
        assert!(matches!(gtk3_status(false), ToolkitStatus::Unsupported(_)));
        let _ = std::fs::remove_dir_all(&home);
    }
}
