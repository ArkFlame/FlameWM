//! Unity `com.canonical.Unity.LauncherEntry` listener. KDE, Firefox, Chromium and Telegram emit
//! `Update(s app_uri, a{sv} properties)` on any object path. Progress is keyed by the emitting
//! unique bus owner and `app_uri`, and is removed when that owner leaves the bus. Count and urgent
//! properties are parsed and ignored: the taskbar renders determinate progress only.

use std::collections::BTreeMap;
use std::time::Duration;

use dbus::arg::messageitem::MessageItem;
use dbus::{Message, MessageType};
use flamewm_api::applications::DesktopApplication;
use flamewm_api::panels::{PanelsSnapshot, TaskEntryKind, TaskProgress, TaskProgressEntry};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

pub const LAUNCHER_ENTRY_INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
pub const LAUNCHER_ENTRY_UPDATE: &str = "Update";

const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const NAME_OWNER_CHANGED: &str = "NameOwnerChanged";
const APP_URI_PREFIX: &str = "application://";
const DESKTOP_SUFFIX: &str = ".desktop";
const MAX_ENTRIES: usize = 256;
const MAX_APP_URI_BYTES: usize = 255;

/// Properties from one `Update`. `None` means the sender did not include that key, so the previous
/// value for that entry is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LauncherUpdate {
    pub progress: Option<u8>,
    pub progress_visible: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LauncherState {
    desktop_id: String,
    progress: Option<u8>,
    visible: bool,
}

/// Unity launcher progress source. Shares the shell's session-bus reactor pump; it owns no thread.
pub struct LauncherEntryProvider {
    pump: BusPump,
    entries: BTreeMap<(String, String), LauncherState>,
    started: bool,
    changed: bool,
}

impl LauncherEntryProvider {
    pub fn connect() -> FlameResult<Self> {
        Ok(Self {
            pump: BusPump::connect(BusKind::Session)?,
            entries: BTreeMap::new(),
            started: false,
            changed: false,
        })
    }

    /// Adds the signal and owner-change matches. Nothing is claimed on the bus.
    pub fn start(&mut self) -> FlameResult<()> {
        if self.started {
            return Ok(());
        }
        for rule in [
            format!(
                "type='signal',interface='{LAUNCHER_ENTRY_INTERFACE}',member='{LAUNCHER_ENTRY_UPDATE}'"
            ),
            format!(
                "type='signal',sender='{DBUS_SERVICE}',path='{DBUS_PATH}',interface='{DBUS_INTERFACE}',member='{NAME_OWNER_CHANGED}'"
            ),
        ] {
            add_match(&self.pump, &rule)?;
        }
        self.started = true;
        Ok(())
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.pump.watch()
    }

    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let mut changed = false;
        let entries = &mut self.entries;
        let count = self
            .pump
            .on_ready(|message| handle_message(message, entries, &mut changed))?;
        self.changed |= changed;
        Ok(count)
    }

    /// Maps visible launcher progress onto running task entries. Each desktop id selects the first
    /// running task whose app id matches its desktop id or `StartupWMClass`; Unity carries no window
    /// identity, so multiple windows of one application share one bar on the first task.
    #[must_use]
    pub fn task_progress(
        &self,
        panels: &PanelsSnapshot,
        applications: &[DesktopApplication],
    ) -> Vec<TaskProgressEntry> {
        task_progress_for_entries(&self.entries, panels, applications)
    }

    /// Whether the most recent reactor turn changed visible launcher progress.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

/// Parses `(s app_uri, a{sv} props)` from an `Update` body.
#[allow(clippy::result_unit_err)]
pub fn parse_update(items: &[MessageItem]) -> Result<(String, LauncherUpdate), ()> {
    let [MessageItem::Str(app_uri), MessageItem::Dict(properties)] = items else {
        return Err(());
    };
    let mut update = LauncherUpdate::default();
    for (key, value) in properties.clone().into_vec() {
        let MessageItem::Str(key) = key else {
            return Err(());
        };
        match key.as_str() {
            "progress" => {
                let MessageItem::Double(value) = unwrap_variant(value) else {
                    return Err(());
                };
                if !value.is_finite() {
                    return Err(());
                }
                update.progress = Some((value.clamp(0.0, 1.0) * 100.0).round() as u8);
            }
            "progress-visible" => match unwrap_variant(value) {
                MessageItem::Bool(value) => update.progress_visible = Some(value),
                _ => return Err(()),
            },
            _ => {}
        }
    }
    Ok((app_uri.clone(), update))
}

/// Desktop id from `application://<id>.desktop` or `application:///path/<id>.desktop`.
#[must_use]
pub fn desktop_id_from_app_uri(app_uri: &str) -> Option<String> {
    if app_uri.len() > MAX_APP_URI_BYTES {
        return None;
    }
    let rest = app_uri.strip_prefix(APP_URI_PREFIX)?;
    let file = rest.rsplit('/').next()?;
    let id = file.strip_suffix(DESKTOP_SUFFIX)?;
    (!id.is_empty() && !id.starts_with('.') && id.chars().all(|c| !c.is_control() && c != '\\'))
        .then(|| id.to_owned())
}

/// Applies one update to the keyed state. Returns `true` when visible progress changed.
fn apply_update(
    entries: &mut BTreeMap<(String, String), LauncherState>,
    owner: &str,
    app_uri: &str,
    update: LauncherUpdate,
) -> bool {
    let Some(desktop_id) = desktop_id_from_app_uri(app_uri) else {
        return false;
    };
    let key = (owner.to_owned(), app_uri.to_owned());
    let before = entries.get(&key).map(visible_progress);
    if !entries.contains_key(&key) {
        if entries.len() >= MAX_ENTRIES {
            return false;
        }
        entries.insert(
            key.clone(),
            LauncherState {
                desktop_id,
                progress: None,
                visible: false,
            },
        );
    }
    let state = entries.get_mut(&key).expect("entry inserted above");
    if let Some(progress) = update.progress {
        state.progress = Some(progress);
    }
    if let Some(visible) = update.progress_visible {
        state.visible = visible;
    }
    let after = visible_progress(state);
    before != Some(after)
}

fn visible_progress(state: &LauncherState) -> Option<u8> {
    state.visible.then_some(state.progress).flatten()
}

/// Names a launcher desktop id may appear as in a task `app_id`: the id itself, and the desktop
/// file's `StartupWMClass` (for example `TelegramDesktop`). Unresolved windows carry the lowercased
/// `WM_CLASS` as their app id, so every name is compared case-insensitively in lowercase.
fn match_names(desktop_id: &str, applications: &[DesktopApplication]) -> Vec<String> {
    let mut names = vec![desktop_id.to_ascii_lowercase()];
    if let Some(app) = applications
        .iter()
        .find(|app| app.id.as_str() == desktop_id)
    {
        let class = app.startup_wm_class.trim();
        if !class.is_empty() {
            names.push(class.to_ascii_lowercase());
        }
    }
    names
}

/// A task is running when it is a window, or a pinned slot whose window is alive. A pinned slot
/// with no window never shows progress, so the fill clears once the owning window closes.
fn is_running(kind: &TaskEntryKind) -> bool {
    match kind {
        TaskEntryKind::Window { .. } => true,
        TaskEntryKind::PinnedSlot { window } => window.is_some(),
    }
}

fn task_progress_for_entries(
    entries: &BTreeMap<(String, String), LauncherState>,
    panels: &PanelsSnapshot,
    applications: &[DesktopApplication],
) -> Vec<TaskProgressEntry> {
    let mut by_app: BTreeMap<&str, u8> = BTreeMap::new();
    for state in entries.values() {
        if let Some(progress) = visible_progress(state) {
            let slot = by_app.entry(state.desktop_id.as_str()).or_insert(progress);
            *slot = (*slot).max(progress);
        }
    }
    by_app
        .into_iter()
        .filter_map(|(desktop_id, progress)| {
            let names = match_names(desktop_id, applications);
            let task = panels.tasks.iter().find(|task| {
                is_running(&task.kind)
                    && names
                        .iter()
                        .any(|name| task.app_id.as_str().eq_ignore_ascii_case(name))
            })?;
            Some(TaskProgressEntry {
                task_id: task.id.clone(),
                progress: TaskProgress::new(progress)?,
            })
        })
        .collect()
}

fn handle_message(
    message: &Message,
    entries: &mut BTreeMap<(String, String), LauncherState>,
    changed: &mut bool,
) -> MessageDisposition {
    if message.msg_type() == MessageType::Signal
        && message.interface().is_some_and(|v| v == DBUS_INTERFACE)
        && message.member().is_some_and(|v| v == NAME_OWNER_CHANGED)
    {
        if let [
            MessageItem::Str(_name),
            MessageItem::Str(old_owner),
            MessageItem::Str(new_owner),
        ] = message.get_items().as_slice()
        {
            if new_owner.is_empty() && !old_owner.is_empty() {
                let before = entries.len();
                entries.retain(|(owner, _), _| owner != old_owner);
                *changed |= entries.len() != before;
            }
        }
        return MessageDisposition::Handled;
    }
    if message.msg_type() == MessageType::Signal
        && message
            .interface()
            .is_some_and(|v| v == LAUNCHER_ENTRY_INTERFACE)
        && message.member().is_some_and(|v| v == LAUNCHER_ENTRY_UPDATE)
    {
        let Some(owner) = message.sender().map(|value| value.to_string()) else {
            return MessageDisposition::Handled;
        };
        if let Ok((app_uri, update)) = parse_update(&message.get_items()) {
            *changed |= apply_update(entries, &owner, &app_uri, update);
        }
        return MessageDisposition::Handled;
    }
    MessageDisposition::Unhandled
}

fn add_match(pump: &BusPump, rule: &str) -> FlameResult<()> {
    let call = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, "AddMatch")
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
        .append1(rule.to_owned());
    let reply = pump.send_with_reply(call, Duration::from_secs(2))?;
    if reply.msg_type() == MessageType::Error {
        return Err(FlameError::unavailable(
            "cannot watch launcher entry signals",
        ));
    }
    Ok(())
}

fn unwrap_variant(item: MessageItem) -> MessageItem {
    match item {
        MessageItem::Variant(value) => unwrap_variant(*value),
        item => item,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::panels::TaskEntry;
    use flamewm_api::{DesktopAppId, TaskEntryId, WindowRef};

    fn app(id: &str, startup_wm_class: &str) -> DesktopApplication {
        DesktopApplication {
            id: DesktopAppId::new(id),
            name: id.into(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: startup_wm_class.into(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: String::new(),
            categories: Vec::new(),
        }
    }

    /// One running window task per app id, window ids starting at 1.
    fn window_tasks(apps: &[&str]) -> PanelsSnapshot {
        PanelsSnapshot {
            revision: 1,
            panels: Vec::new(),
            tasks: apps
                .iter()
                .enumerate()
                .map(|(index, app)| {
                    let window = WindowRef::new(index as u64 + 1, 1);
                    TaskEntry {
                        id: TaskEntryId::window(window),
                        app_id: DesktopAppId::new(*app),
                        kind: TaskEntryKind::Window { window },
                        order_index: index,
                    }
                })
                .collect(),
            pinned_apps: Vec::new(),
        }
    }

    fn props(progress: Option<f64>, visible: Option<bool>) -> MessageItem {
        let mut entries = Vec::new();
        if let Some(progress) = progress {
            entries.push((
                MessageItem::Str("progress".into()),
                MessageItem::Variant(Box::new(MessageItem::Double(progress))),
            ));
        }
        if let Some(visible) = visible {
            entries.push((
                MessageItem::Str("progress-visible".into()),
                MessageItem::Variant(Box::new(MessageItem::Bool(visible))),
            ));
        }
        entries.push((
            MessageItem::Str("count".into()),
            MessageItem::Variant(Box::new(MessageItem::Int64(7))),
        ));
        MessageItem::new_dict(entries).expect("dict")
    }

    fn body(uri: &str, progress: Option<f64>, visible: Option<bool>) -> Vec<MessageItem> {
        vec![MessageItem::Str(uri.into()), props(progress, visible)]
    }

    #[test]
    fn parses_progress_and_visibility_and_ignores_count() {
        let (uri, update) = parse_update(&body(
            "application://org.kde.x.desktop",
            Some(0.425),
            Some(true),
        ))
        .expect("valid update");
        assert_eq!(uri, "application://org.kde.x.desktop");
        assert_eq!(update.progress, Some(43));
        assert_eq!(update.progress_visible, Some(true));
    }

    #[test]
    fn missing_keys_stay_none_and_bad_types_are_rejected() {
        let (_, update) = parse_update(&body("application://a.desktop", None, None)).unwrap();
        assert_eq!(update, LauncherUpdate::default());
        let bad = vec![
            MessageItem::Str("application://a.desktop".into()),
            MessageItem::new_dict(vec![(
                MessageItem::Str("progress".into()),
                MessageItem::Variant(Box::new(MessageItem::Str("x".into()))),
            )])
            .unwrap(),
        ];
        assert!(parse_update(&bad).is_err());
        assert!(parse_update(&[MessageItem::UInt32(1)]).is_err());
    }

    #[test]
    fn progress_is_clamped_and_nan_is_rejected() {
        let (_, high) = parse_update(&body("application://a.desktop", Some(3.0), None)).unwrap();
        assert_eq!(high.progress, Some(100));
        assert!(parse_update(&body("application://a.desktop", Some(f64::NAN), None)).is_err());
    }

    #[test]
    fn desktop_id_accepts_name_and_path_forms_only() {
        assert_eq!(
            desktop_id_from_app_uri("application://firefox.desktop").as_deref(),
            Some("firefox")
        );
        assert_eq!(
            desktop_id_from_app_uri("application:///usr/share/applications/telegram.desktop")
                .as_deref(),
            Some("telegram")
        );
        assert_eq!(desktop_id_from_app_uri("file:///x.desktop"), None);
        assert_eq!(desktop_id_from_app_uri("application://.desktop"), None);
        assert_eq!(desktop_id_from_app_uri("application://plain"), None);
    }

    #[test]
    fn update_maps_to_visible_progress_on_the_matching_task() {
        let mut entries = BTreeMap::new();
        assert!(apply_update(
            &mut entries,
            ":1.5",
            "application://org.kde.dolphin.desktop",
            parse_update(&body(
                "application://org.kde.dolphin.desktop",
                Some(0.5),
                Some(true)
            ))
            .unwrap()
            .1
        ));
        let mapped =
            task_progress_for_entries(&entries, &window_tasks(&["org.kde.dolphin", "other"]), &[]);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].task_id, TaskEntryId::window(WindowRef::new(1, 1)));
        assert_eq!(mapped[0].progress.percent(), 50);
    }

    #[test]
    fn hidden_or_unmatched_progress_is_not_mapped() {
        let mut entries = BTreeMap::new();
        apply_update(
            &mut entries,
            ":1.5",
            "application://a.desktop",
            LauncherUpdate {
                progress: Some(40),
                progress_visible: Some(false),
            },
        );
        assert!(task_progress_for_entries(&entries, &window_tasks(&["a"]), &[]).is_empty());
        apply_update(
            &mut entries,
            ":1.5",
            "application://a.desktop",
            LauncherUpdate {
                progress: None,
                progress_visible: Some(true),
            },
        );
        assert!(task_progress_for_entries(&entries, &window_tasks(&["zzz"]), &[]).is_empty());
        let mapped = task_progress_for_entries(&entries, &window_tasks(&["A"]), &[]);
        assert_eq!(mapped.len(), 1, "app ids compare case-insensitively");
    }

    #[test]
    fn pinned_slot_without_window_never_shows_progress() {
        let mut entries = BTreeMap::new();
        apply_update(
            &mut entries,
            ":1.5",
            "application://a.desktop",
            LauncherUpdate {
                progress: Some(40),
                progress_visible: Some(true),
            },
        );
        let pinned = PanelsSnapshot {
            revision: 1,
            panels: Vec::new(),
            tasks: vec![TaskEntry {
                id: TaskEntryId::pinned(&DesktopAppId::new("a")),
                app_id: DesktopAppId::new("a"),
                kind: TaskEntryKind::PinnedSlot { window: None },
                order_index: 0,
            }],
            pinned_apps: Vec::new(),
        };
        assert!(task_progress_for_entries(&entries, &pinned, &[]).is_empty());
    }

    #[test]
    fn startup_wm_class_maps_telegram_and_wm_class_fallback_maps_firefox() {
        let mut entries = BTreeMap::new();
        apply_update(
            &mut entries,
            ":1.5",
            "application://org.telegram.desktop.desktop",
            LauncherUpdate {
                progress: Some(30),
                progress_visible: Some(true),
            },
        );
        let apps = [app("org.telegram.desktop", "TelegramDesktop")];
        let mapped =
            task_progress_for_entries(&entries, &window_tasks(&["telegramdesktop"]), &apps);
        assert_eq!(
            mapped.len(),
            1,
            "StartupWMClass maps the unresolved WM_CLASS app id"
        );

        let mut firefox = BTreeMap::new();
        apply_update(
            &mut firefox,
            ":1.6",
            "application://firefox.desktop",
            LauncherUpdate {
                progress: Some(10),
                progress_visible: Some(true),
            },
        );
        let mapped = task_progress_for_entries(&firefox, &window_tasks(&["Firefox"]), &[]);
        assert_eq!(
            mapped.len(),
            1,
            "desktop id matches the WM_CLASS case-insensitively"
        );
    }

    #[test]
    fn repeated_identical_updates_report_no_change() {
        let mut entries = BTreeMap::new();
        let on = LauncherUpdate {
            progress: Some(10),
            progress_visible: Some(true),
        };
        assert!(apply_update(
            &mut entries,
            ":1.2",
            "application://a.desktop",
            on
        ));
        assert!(!apply_update(
            &mut entries,
            ":1.2",
            "application://a.desktop",
            on
        ));
    }

    #[test]
    fn owner_scoped_entries_are_removed_with_owner_and_entry_cap_holds() {
        let on = LauncherUpdate {
            progress: Some(1),
            progress_visible: Some(true),
        };
        let mut entries = BTreeMap::new();
        for index in 0..MAX_ENTRIES - 1 {
            apply_update(
                &mut entries,
                ":1.9",
                &format!("application://a{index}.desktop"),
                on,
            );
        }
        assert!(apply_update(
            &mut entries,
            ":1.10",
            "application://b.desktop",
            on
        ));
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert!(
            !apply_update(&mut entries, ":1.10", "application://c.desktop", on),
            "cap refuses new keys once full"
        );
        assert_eq!(entries.len(), MAX_ENTRIES);
        entries.retain(|(owner, _), _| owner != ":1.9");
        assert_eq!(
            entries.len(),
            1,
            "owner exit removes only that owner's entries"
        );
    }
}
