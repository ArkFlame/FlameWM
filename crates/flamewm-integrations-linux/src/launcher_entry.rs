//! Unity `com.canonical.Unity.LauncherEntry` listener. KDE, Firefox, Chromium and Telegram emit
//! `Update(s app_uri, a{sv} properties)` on any object path. Progress is keyed by the emitting
//! unique bus owner and `app_uri`, and is removed when that owner leaves the bus. Count and urgent
//! properties are parsed and ignored: the taskbar renders determinate progress only.

use std::collections::BTreeMap;
use std::time::Duration;

use dbus::arg::messageitem::MessageItem;
use dbus::{Message, MessageType};
use flamewm_api::panels::{PanelsSnapshot, TaskProgress, TaskProgressEntry};
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
        let count = self.pump.on_ready(|message| {
            handle_message(message, entries, &mut changed)
        })?;
        self.changed |= changed;
        Ok(count)
    }

    /// Maps visible launcher progress onto task entries by desktop id. Each desktop id selects the
    /// first task with that app id; Unity carries no window identity, so multiple windows of one
    /// application share one bar on the first task.
    #[must_use]
    pub fn task_progress(&self, panels: &PanelsSnapshot) -> Vec<TaskProgressEntry> {
        task_progress_for_entries(&self.entries, panels)
    }

    /// Whether the most recent reactor turn changed visible launcher progress.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

}

/// Parses `(s app_uri, a{sv} props)` from an `Update` body.
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
    (!id.is_empty()
        && !id.starts_with('.')
        && id.chars().all(|c| !c.is_control() && c != '\\'))
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

fn task_progress_for_entries(
    entries: &BTreeMap<(String, String), LauncherState>,
    panels: &PanelsSnapshot,
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
            let task = panels.tasks.iter().find(|task| {
                let app = task.app_id.as_str();
                app == desktop_id || app.eq_ignore_ascii_case(desktop_id)
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
        return Err(FlameError::unavailable("cannot watch launcher entry signals"));
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
    use flamewm_api::panels::{TaskEntry, TaskEntryKind};
    use flamewm_api::{DesktopAppId, TaskEntryId};

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

    fn panels(apps: &[&str]) -> PanelsSnapshot {
        PanelsSnapshot {
            revision: 1,
            panels: Vec::new(),
            tasks: apps
                .iter()
                .enumerate()
                .map(|(index, app)| TaskEntry {
                    id: TaskEntryId::pinned(&DesktopAppId::new(*app)),
                    app_id: DesktopAppId::new(*app),
                    kind: TaskEntryKind::PinnedSlot { window: None },
                    order_index: index,
                })
                .collect(),
            pinned_apps: Vec::new(),
        }
    }

    #[test]
    fn parses_progress_and_visibility_and_ignores_count() {
        let (uri, update) = parse_update(&body("application://org.kde.x.desktop", Some(0.425), Some(true)))
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
        assert_eq!(desktop_id_from_app_uri("application://firefox.desktop").as_deref(), Some("firefox"));
        assert_eq!(
            desktop_id_from_app_uri("application:///usr/share/applications/telegram.desktop").as_deref(),
            Some("telegram")
        );
        assert_eq!(desktop_id_from_app_uri("file:///x.desktop"), None);
        assert_eq!(desktop_id_from_app_uri("application://.desktop"), None);
        assert_eq!(desktop_id_from_app_uri("application://plain"), None);
    }

    #[test]
    fn update_maps_to_visible_progress_on_the_matching_task() {
        let mut entries = BTreeMap::new();
        assert!(apply_update(&mut entries, ":1.5", "application://org.kde.dolphin.desktop", parse_update(&body("application://org.kde.dolphin.desktop", Some(0.5), Some(true))).unwrap().1));
        let mapped = task_progress_for_entries(&entries, &panels(&["org.kde.dolphin", "other"]));
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].task_id, TaskEntryId::pinned(&DesktopAppId::new("org.kde.dolphin")));
        assert_eq!(mapped[0].progress.percent(), 50);
    }

    #[test]
    fn hidden_or_unmatched_progress_is_not_mapped() {
        let mut entries = BTreeMap::new();
        apply_update(&mut entries, ":1.5", "application://a.desktop", LauncherUpdate { progress: Some(40), progress_visible: Some(false) });
        assert!(task_progress_for_entries(&entries, &panels(&["a"])).is_empty());
        apply_update(&mut entries, ":1.5", "application://a.desktop", LauncherUpdate { progress: None, progress_visible: Some(true) });
        assert!(task_progress_for_entries(&entries, &panels(&["zzz"])).is_empty());
        let mapped = task_progress_for_entries(&entries, &panels(&["A"]));
        assert_eq!(mapped.len(), 1, "app ids compare case-insensitively");
    }

    #[test]
    fn repeated_identical_updates_report_no_change() {
        let mut entries = BTreeMap::new();
        let on = LauncherUpdate { progress: Some(10), progress_visible: Some(true) };
        assert!(apply_update(&mut entries, ":1.2", "application://a.desktop", on));
        assert!(!apply_update(&mut entries, ":1.2", "application://a.desktop", on));
    }

    #[test]
    fn owner_scoped_entries_are_removed_with_owner_and_entry_cap_holds() {
        let on = LauncherUpdate { progress: Some(1), progress_visible: Some(true) };
        let mut entries = BTreeMap::new();
        for index in 0..MAX_ENTRIES - 1 {
            apply_update(&mut entries, ":1.9", &format!("application://a{index}.desktop"), on);
        }
        assert!(apply_update(&mut entries, ":1.10", "application://b.desktop", on));
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert!(!apply_update(&mut entries, ":1.10", "application://c.desktop", on), "cap refuses new keys once full");
        assert_eq!(entries.len(), MAX_ENTRIES);
        entries.retain(|(owner, _), _| owner != ":1.9");
        assert_eq!(entries.len(), 1, "owner exit removes only that owner's entries");
    }
}
