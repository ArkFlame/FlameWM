use std::cell::RefCell;
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use flamewm_api::settings::{AppearanceMode, SettingValue, SettingsSnapshot};
use flamewm_session_core::{
    build_launch_plan_with_appearance, default_paths_from_home, resolve_cursor_theme,
};
use flamewm_wm::{
    SettingsObserver, SettingsWake, WmConfig, apply_cursor_theme, persisted_cursor_theme, run,
    run_with_settings_observer,
};
use flamewm_xsettings::{XSettingsInput, X11XSettingsManager, claim_x11_manager, published_settings};

fn main() {
    apply_session_environment();
    // The manager is shared (Rc<RefCell>) by the settings observer and the
    // reactor wake; it lives for the process lifetime and dropping the last
    // handle releases _XSETTINGS_S0.
    let result = match claim_xsettings_manager() {
        Some(manager) => {
            let (observer, wake) = xsettings_owner(manager);
            run_with_settings_observer(WmConfig::from_env(), observer, wake)
        }
        None => run(WmConfig::from_env()),
    };
    if let Err(error) = result {
        eprintln!("flamewm: {error}");
        std::process::exit(1);
    }
}

/// Consume session-core environment so the WM process and every child it
/// spawns inherit the FlameWM identity (FlameWM, never KDE) plus cursor and
/// appearance state. Dark is the default; explicit user values win except
/// for FlameWM-owned identity keys.
fn apply_session_environment() {
    let existing: BTreeMap<String, String> = env::vars().collect();
    let appearance = existing
        .get("FLAMEWM_APPEARANCE")
        .and_then(|value| AppearanceMode::parse(value))
        .unwrap_or_default();
    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    let bindir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    let paths = default_paths_from_home(bindir, home);
    let mut pending: BTreeMap<String, String> = BTreeMap::new();
    // session-core contributes cursor, identity, and appearance-marker
    // state only. No generic toolkit default injection: explicit inherited
    // GTK_THEME/QT_STYLE_OVERRIDE pass through untouched, never synthesized.
    let search_path = existing.get("XCURSOR_PATH").cloned().unwrap_or_default();
    let persisted = persisted_cursor_theme().unwrap_or_default();
    let cursor_theme = resolve_cursor_theme(&persisted, &search_path);
    let plan = build_launch_plan_with_appearance(&paths, &[], &existing, None, appearance);
    if let Some(process) = plan.processes.first() {
        for (key, value) in &process.environment {
            if key == "XDG_CURRENT_DESKTOP" && is_kde(value) {
                continue;
            }
            if key.starts_with("FLAMEWM_") || env::var_os(key).is_none() {
                pending.insert(key.clone(), value.clone());
            }
        }
    }
    if let Some(current) = env::var_os("XDG_CURRENT_DESKTOP") {
        if current.to_string_lossy().eq_ignore_ascii_case("kde") {
            pending.insert("XDG_CURRENT_DESKTOP".to_owned(), "FlameWM".to_owned());
        }
    }
    pending.insert("XCURSOR_THEME".to_owned(), cursor_theme);
    flamewm_session_core::apply_current_process_env(&pending);
}

fn is_kde(value: &str) -> bool {
    value.eq_ignore_ascii_case("kde")
}

/// Start the FlameWM XSettings owner: take `_XSETTINGS_S0` (refusing any
/// foreign owner) and publish the default FlameWM settings. The WM keeps running
/// without XSettings ownership when the claim is refused or fails.
fn claim_xsettings_manager() -> Option<X11XSettingsManager> {
    let input = xsettings_input_from_snapshot(&startup_settings_snapshot());
    match claim_x11_manager(&published_settings(&input)) {
        Ok(manager) => Some(manager),
        Err(error) => {
            eprintln!("flamewm: XSettings manager not claimed: {error}");
            None
        }
    }
}

/// Startup settings: only the process-start appearance from
/// `FLAMEWM_APPEARANCE`; every other key is absent and maps to its default.
/// Routed through the same mapper as live updates so the paths cannot diverge.
fn startup_settings_snapshot() -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(0);
    if let Ok(appearance) = env::var("FLAMEWM_APPEARANCE") {
        snapshot
            .values
            .insert("appearance".to_owned(), SettingValue::Text(appearance));
    }
    snapshot
}

/// Map WM settings-snapshot values onto the XSettings input. Absent or
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
    xsettings_input_with_cursor_env(snapshot, &session_theme, &search_path)
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
        input.icon_theme = icon_theme.to_owned();
    }
    if let Some(font_family) = text_value(snapshot, "fontFamily") {
        input.font_family = font_family.to_owned();
    }
    if let Some(font_bold) = boolean_value(snapshot, "fontBold") {
        input.font_bold = font_bold;
    }
    if let Some(points) = font_size_points(snapshot) {
        input.font_size_points = points;
    }
    input
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

/// `fontSizeOffset` is relative; XSettings publishes an absolute point size, so
/// the offset is applied to the default base size and clamped to 1..=72.
fn font_size_points(snapshot: &SettingsSnapshot) -> Option<u32> {
    let base = i64::from(XSettingsInput::default().font_size_points);
    match snapshot.values.get("fontSizeOffset") {
        Some(SettingValue::Integer(offset)) => u32::try_from((base + offset).clamp(1, 72)).ok(),
        _ => None,
    }
}

/// Live XSettings path. Each settings sync point maps the snapshot and
/// republishes (a no-op when nothing changed). The first failure is logged;
/// later failures are suppressed so a persistent error cannot flood stderr.
/// The WM keeps running without XSettings updates.
/// Split the owned manager into the publish observer and the reactor wake. The
/// manager is shared through `Rc<RefCell<_>>`; the descriptor is read before the
/// move so the wake can be registered without borrowing the manager. Without a
/// descriptor the wake is `None` and only publish-time draining remains.
fn xsettings_owner(manager: X11XSettingsManager) -> (SettingsObserver, Option<SettingsWake>) {
    let wake_fd = manager.wake_fd();
    let shared = Rc::new(RefCell::new(manager));
    let observer_manager = Rc::clone(&shared);
    let mut reported = false;
    let observer: SettingsObserver = Box::new(move |snapshot: &SettingsSnapshot| {
        let input = xsettings_input_from_snapshot(snapshot);
        for error in apply_cursor_theme(&input.cursor_theme) {
            eprintln!("flamewm: cursor theme reload failed: {error}");
        }
        let outcome = {
            let mut manager = observer_manager.borrow_mut();
            manager
                .drain_events()
                .and_then(|_| manager.republish(&input))
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
        assert_eq!(xsettings_input_from_snapshot(&dark).appearance, AppearanceMode::Dark);
        assert_eq!(xsettings_input_from_snapshot(&light).appearance, AppearanceMode::Light);
        assert_eq!(xsettings_input_from_snapshot(&system).appearance, AppearanceMode::System);
    }

    #[test]
    fn unknown_or_non_text_appearance_falls_back_to_default() {
        let unknown = snapshot_with(&[("appearance", text("purple"))]);
        let wrong_type = snapshot_with(&[("appearance", SettingValue::Integer(1))]);
        assert_eq!(xsettings_input_from_snapshot(&unknown).appearance, AppearanceMode::Dark);
        assert_eq!(xsettings_input_from_snapshot(&wrong_type).appearance, AppearanceMode::Dark);
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
}
