//! Minimal freedesktop notification server backed by `FlameWM`'s session D-Bus reactor.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dbus::arg::messageitem::MessageItem;
use dbus::strings::ErrorName;
use dbus::{Message, MessageType};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

use crate::notification_history::{NotificationHistory, PopupDecision};

pub const NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
pub const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
pub const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";

const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const MAX_NOTIFICATIONS: usize = 64;
const MAX_UI_UPDATES: usize = 256;
const MAX_TEXT_BYTES: usize = 8192;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const HISTORY_CAPACITY: usize = 200;
const HISTORY_FILE: &str = "flamewm/notification-history.tsv";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationCloseReason {
    Expired,
    Dismissed,
    Closed,
}

impl NotificationCloseReason {
    const fn wire_value(self) -> u32 {
        match self {
            Self::Expired => 1,
            Self::Dismissed => 2,
            Self::Closed => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationView {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub urgency: NotificationUrgency,
    pub progress: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationUiUpdate {
    Upsert(NotificationView),
    Close {
        id: u32,
        reason: NotificationCloseReason,
    },
    Reset(Vec<NotificationView>),
}

#[derive(Debug, Clone)]
struct Notification {
    view: NotificationView,
    expires_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderState {
    Stopped,
    Unavailable,
    Owned,
}

/// A nonblocking freedesktop notification owner. It declines the well-known name when another
/// desktop owns it. UI consumers drain typed updates and drive `expire` from `next_deadline`.
pub struct NotificationProvider {
    pump: BusPump,
    state: ProviderState,
    notifications: BTreeMap<u32, Notification>,
    updates: VecDeque<NotificationUiUpdate>,
    next_id: u32,
    history: NotificationHistory,
    history_path: Option<PathBuf>,
    persisted_revision: u64,
}

/// `$XDG_STATE_HOME/flamewm/notification-history.tsv`, falling back to `~/.local/state`.
fn history_path() -> Option<PathBuf> {
    flamewm_api::paths::xdg_dir("XDG_STATE_HOME", ".local/state")
        .map(|base| base.join(HISTORY_FILE))
}

impl NotificationProvider {
    pub fn connect() -> FlameResult<Self> {
        let history_path = history_path();
        let history = history_path.as_deref().map_or_else(
            || NotificationHistory::new(HISTORY_CAPACITY),
            |path| NotificationHistory::load_file(path, HISTORY_CAPACITY),
        );
        let persisted_revision = history.revision();
        Ok(Self {
            pump: BusPump::connect(BusKind::Session)?,
            state: ProviderState::Stopped,
            notifications: BTreeMap::new(),
            updates: VecDeque::new(),
            next_id: 1,
            history,
            history_path,
            persisted_revision,
        })
    }

    /// Recorded notifications, grouped and bounded, for the notification center.
    #[must_use]
    pub const fn history(&self) -> &NotificationHistory {
        &self.history
    }

    pub fn set_dnd(&mut self, enabled: bool) {
        self.history.set_dnd(enabled);
        self.persist_history();
    }

    /// Returns false when the mute set is full and `app_name` cannot be added.
    pub fn set_app_muted(&mut self, app_name: &str, muted: bool) -> bool {
        let applied = self.history.set_app_muted(app_name, muted);
        self.persist_history();
        applied
    }

    pub fn remove_history_entry(&mut self, seq: u64) -> bool {
        let removed = self.history.remove(seq);
        self.persist_history();
        removed
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.persist_history();
    }

    /// Saves history when it changed since the last save. Failure is reported, never fatal.
    fn persist_history(&mut self) {
        let revision = self.history.revision();
        if revision == self.persisted_revision {
            return;
        }
        let Some(path) = self.history_path.as_deref() else {
            self.persisted_revision = revision;
            return;
        };
        match self.history.save_file(path) {
            Ok(()) => self.persisted_revision = revision,
            Err(error) => {
                eprintln!("flamewm-notifications: saving notification history failed: {error}")
            }
        }
    }

    /// Owns the name only if no existing notification server is present.
    pub fn start(&mut self) -> FlameResult<bool> {
        if self.state == ProviderState::Owned {
            return Ok(true);
        }
        let call = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, "RequestName")
            .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
            .append2(NOTIFICATIONS_SERVICE.to_owned(), 0x4_u32);
        let reply = self.pump.send_with_reply(call, Duration::from_secs(2))?;
        if reply.msg_type() == MessageType::Error {
            return Err(FlameError::unavailable(
                "notification D-Bus name request failed",
            ));
        }
        match reply.read1::<u32>() {
            Ok(1) => {
                self.state = ProviderState::Owned;
                Ok(true)
            }
            Ok(_) => {
                eprintln!(
                    "flamewm-notifications: org.freedesktop.Notifications is owned by another daemon; toasts are disabled"
                );
                self.state = ProviderState::Unavailable;
                Ok(false)
            }
            Err(error) => Err(FlameError::invalid(format!(
                "notification D-Bus name reply is invalid: {error}"
            ))),
        }
    }

    pub fn stop(&mut self) -> FlameResult<()> {
        let owned = self.state == ProviderState::Owned;
        self.state = ProviderState::Stopped;
        let ids: Vec<_> = self.notifications.keys().copied().collect();
        self.notifications.clear();
        for id in ids {
            self.push_update(NotificationUiUpdate::Close {
                id,
                reason: NotificationCloseReason::Closed,
            });
        }
        if !owned {
            return Ok(());
        }
        let call = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, "ReleaseName")
            .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
            .append1(NOTIFICATIONS_SERVICE.to_owned());
        let reply = self.pump.send_with_reply(call, Duration::from_secs(2))?;
        if reply.msg_type() == MessageType::Error {
            return Err(FlameError::unavailable(
                "notification D-Bus name release failed",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.pump.watch()
    }

    /// Handles one bounded reactor turn and sends generated closure signals after dispatch.
    pub fn on_ready(&mut self) -> FlameResult<usize> {
        self.expire(Instant::now())?;
        let mut signals = Vec::new();
        let state = &mut self.state;
        let notifications = &mut self.notifications;
        let updates = &mut self.updates;
        let next_id = &mut self.next_id;
        let history = &mut self.history;
        let count = self.pump.on_ready(|message| {
            handle_message(
                message,
                *state == ProviderState::Owned,
                notifications,
                updates,
                next_id,
                history,
                &mut signals,
                Instant::now(),
            )
        })?;
        for signal in signals {
            self.pump.send(signal)?;
        }
        self.persist_history();
        Ok(count)
    }

    /// Removes expired entries and emits the standard `NotificationClosed` signal.
    pub fn expire(&mut self, now: Instant) -> FlameResult<usize> {
        let expired: Vec<_> = self
            .notifications
            .iter()
            .filter_map(|(&id, notification)| {
                notification
                    .expires_at
                    .is_some_and(|deadline| deadline <= now)
                    .then_some(id)
            })
            .collect();
        for id in &expired {
            self.notifications.remove(id);
            self.push_update(NotificationUiUpdate::Close {
                id: *id,
                reason: NotificationCloseReason::Expired,
            });
            self.pump
                .send(closed_signal(*id, NotificationCloseReason::Expired))?;
        }
        Ok(expired.len())
    }

    /// Dismiss one active notification from the UI and report the standard close reason.
    pub fn dismiss(&mut self, id: u32) -> FlameResult<bool> {
        if self.notifications.remove(&id).is_none() {
            return Ok(false);
        }
        self.push_update(NotificationUiUpdate::Close {
            id,
            reason: NotificationCloseReason::Dismissed,
        });
        self.pump
            .send(closed_signal(id, NotificationCloseReason::Dismissed))?;
        Ok(true)
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.notifications
            .values()
            .filter_map(|notification| notification.expires_at)
            .min()
    }

    #[must_use]
    pub fn drain_ui_updates(&mut self) -> Vec<NotificationUiUpdate> {
        self.updates.drain(..).collect()
    }

    #[must_use]
    pub fn active(&self) -> Vec<NotificationView> {
        self.notifications
            .values()
            .map(|notification| notification.view.clone())
            .collect()
    }

    fn push_update(&mut self, update: NotificationUiUpdate) {
        if self.updates.len() >= MAX_UI_UPDATES {
            self.updates.clear();
            self.updates
                .push_back(NotificationUiUpdate::Reset(self.active()));
        } else {
            self.updates.push_back(update);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_message(
    message: &Message,
    owned: bool,
    notifications: &mut BTreeMap<u32, Notification>,
    updates: &mut VecDeque<NotificationUiUpdate>,
    next_id: &mut u32,
    history: &mut NotificationHistory,
    signals: &mut Vec<Message>,
    now: Instant,
) -> MessageDisposition {
    if message.msg_type() != MessageType::MethodCall
        || message.path().is_none_or(|path| path != NOTIFICATIONS_PATH)
        || message
            .interface()
            .is_none_or(|interface| interface != NOTIFICATIONS_INTERFACE)
    {
        return MessageDisposition::Unhandled;
    }
    if !owned {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "notification service is unavailable",
        );
    }
    match message.member().map(|member| member.to_string()).as_deref() {
        Some("GetCapabilities") if message.get_items().is_empty() => {
            MessageDisposition::Reply(message.method_return().append1(vec!["body".to_owned()]))
        }
        Some("GetServerInformation") if message.get_items().is_empty() => {
            MessageDisposition::Reply(
                message
                    .method_return()
                    .append1("FlameWM")
                    .append1("ArkFlame Studios")
                    .append1("FlameWM")
                    .append1("1.0"),
            )
        }
        Some("Notify") => notify(message, notifications, updates, next_id, history, now),
        Some("CloseNotification") => close_notification(message, notifications, updates, signals),
        Some(_) => error_reply(
            message,
            "org.freedesktop.DBus.Error.UnknownMethod",
            "unsupported notification method",
        ),
        None => error_reply(
            message,
            "org.freedesktop.DBus.Error.InvalidArgs",
            "notification method has no member name",
        ),
    }
}

fn notify(
    message: &Message,
    notifications: &mut BTreeMap<u32, Notification>,
    updates: &mut VecDeque<NotificationUiUpdate>,
    next_id: &mut u32,
    history: &mut NotificationHistory,
    now: Instant,
) -> MessageDisposition {
    let items = message.get_items();
    let [
        MessageItem::Str(app_name),
        MessageItem::UInt32(replaces_id),
        MessageItem::Str(_app_icon),
        MessageItem::Str(summary),
        MessageItem::Str(body),
        MessageItem::Array(_actions),
        MessageItem::Dict(hints),
        MessageItem::Int32(timeout),
    ] = items.as_slice()
    else {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.InvalidArgs",
            "invalid Notify arguments",
        );
    };
    if app_name.len() > 255 || summary.len() > MAX_TEXT_BYTES || body.len() > MAX_TEXT_BYTES {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.LimitsExceeded",
            "notification text exceeds the supported limit",
        );
    }
    let (urgency, progress) = parse_hints(hints);
    // Replacing an active toast keeps it on screen and is not a new history entry.
    let replacing = *replaces_id != 0 && notifications.contains_key(replaces_id);
    let decision = if replacing {
        PopupDecision::Show
    } else {
        history.popup_decision(app_name, urgency)
    };
    if decision == PopupDecision::Show && !replacing && notifications.len() >= MAX_NOTIFICATIONS {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.LimitsExceeded",
            "too many active notifications",
        );
    }
    let id = if replacing {
        *replaces_id
    } else {
        let Some(id) = allocate_id(notifications, next_id) else {
            return error_reply(
                message,
                "org.freedesktop.DBus.Error.LimitsExceeded",
                "notification identifiers exhausted",
            );
        };
        id
    };
    let view = NotificationView {
        id,
        app_name: app_name.clone(),
        summary: summary.clone(),
        body: body.clone(),
        urgency,
        progress,
    };
    if !replacing {
        history.record(view.clone(), epoch_millis());
    }
    if decision == PopupDecision::Show {
        notifications.insert(
            id,
            Notification {
                view: view.clone(),
                expires_at: expiration(*timeout).map(|duration| now + duration),
            },
        );
        push_update(updates, notifications, NotificationUiUpdate::Upsert(view));
    }
    MessageDisposition::Reply(message.method_return().append1(id))
}

fn epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn close_notification(
    message: &Message,
    notifications: &mut BTreeMap<u32, Notification>,
    updates: &mut VecDeque<NotificationUiUpdate>,
    signals: &mut Vec<Message>,
) -> MessageDisposition {
    let items = message.get_items();
    let [MessageItem::UInt32(id)] = items.as_slice() else {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.InvalidArgs",
            "invalid CloseNotification arguments",
        );
    };
    if notifications.remove(id).is_some() {
        push_update(
            updates,
            notifications,
            NotificationUiUpdate::Close {
                id: *id,
                reason: NotificationCloseReason::Closed,
            },
        );
        signals.push(closed_signal(*id, NotificationCloseReason::Closed));
    }
    MessageDisposition::Reply(message.method_return())
}

fn push_update(
    updates: &mut VecDeque<NotificationUiUpdate>,
    notifications: &BTreeMap<u32, Notification>,
    update: NotificationUiUpdate,
) {
    if updates.len() >= MAX_UI_UPDATES {
        updates.clear();
        updates.push_back(NotificationUiUpdate::Reset(
            notifications
                .values()
                .map(|notification| notification.view.clone())
                .collect(),
        ));
    } else {
        updates.push_back(update);
    }
}

fn parse_hints(
    hints: &dbus::arg::messageitem::MessageItemDict,
) -> (NotificationUrgency, Option<u8>) {
    let mut urgency = NotificationUrgency::Normal;
    let mut progress = None;
    for (key, value) in hints.clone().into_vec() {
        let MessageItem::Str(key) = key else { continue };
        let value = unwrap_variant(value);
        match (key.as_str(), value) {
            ("urgency", MessageItem::Byte(0)) => urgency = NotificationUrgency::Low,
            ("urgency", MessageItem::Byte(1)) => urgency = NotificationUrgency::Normal,
            ("urgency", MessageItem::Byte(2)) => urgency = NotificationUrgency::Critical,
            ("value", MessageItem::Int32(value)) if (0..=100).contains(&value) => {
                progress = Some(value as u8);
            }
            _ => {}
        }
    }
    (urgency, progress)
}

fn unwrap_variant(value: MessageItem) -> MessageItem {
    match value {
        MessageItem::Variant(value) => unwrap_variant(*value),
        value => value,
    }
}

fn expiration(timeout_ms: i32) -> Option<Duration> {
    if timeout_ms == 0 {
        return None;
    }
    let duration = if timeout_ms < 0 {
        DEFAULT_TIMEOUT
    } else {
        Duration::from_millis(timeout_ms as u64)
    };
    Some(duration.min(MAX_TIMEOUT))
}

fn allocate_id(notifications: &BTreeMap<u32, Notification>, next_id: &mut u32) -> Option<u32> {
    for _ in 0..=notifications.len() {
        let id = *next_id;
        *next_id = next_id.wrapping_add(1).max(1);
        if id != 0 && !notifications.contains_key(&id) {
            return Some(id);
        }
    }
    None
}

fn closed_signal(id: u32, reason: NotificationCloseReason) -> Message {
    Message::new_signal(
        NOTIFICATIONS_PATH,
        NOTIFICATIONS_INTERFACE,
        "NotificationClosed",
    )
    .expect("constant notification signal identifiers are valid")
    .append2(id, reason.wire_value())
}

fn error_reply(message: &Message, name: &str, text: &str) -> MessageDisposition {
    if message.get_no_reply() {
        return MessageDisposition::Handled;
    }
    let Ok(name) = ErrorName::new(name) else {
        return MessageDisposition::Handled;
    };
    let Ok(text) = CString::new(text) else {
        return MessageDisposition::Handled;
    };
    MessageDisposition::Reply(message.error(&name, &text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(items: Vec<(String, MessageItem)>) -> dbus::arg::messageitem::MessageItemDict {
        let dict = MessageItem::from_dict(items.into_iter().map(Ok::<_, ()>)).unwrap();
        let MessageItem::Dict(dict) = dict else {
            panic!("dictionary")
        };
        dict
    }

    #[test]
    fn urgency_progress_and_timeout_hints_are_bounded() {
        let dict = hints(vec![
            ("urgency".to_owned(), MessageItem::Byte(2)),
            ("value".to_owned(), MessageItem::Int32(55)),
        ]);
        assert_eq!(
            parse_hints(&dict),
            (NotificationUrgency::Critical, Some(55))
        );
        assert_eq!(expiration(-1), Some(DEFAULT_TIMEOUT));
        assert_eq!(expiration(i32::MAX), Some(MAX_TIMEOUT));
        assert_eq!(expiration(0), None);
        assert_eq!(expiration(-1), Some(DEFAULT_TIMEOUT));
        assert_eq!(expiration(500), Some(Duration::from_millis(500)));
    }

    #[test]
    fn id_allocation_wraps_without_reusing_live_ids() {
        let mut active = BTreeMap::new();
        active.insert(
            1,
            Notification {
                view: NotificationView {
                    id: 1,
                    app_name: String::new(),
                    summary: String::new(),
                    body: String::new(),
                    urgency: NotificationUrgency::Normal,
                    progress: None,
                },
                expires_at: None,
            },
        );
        let mut next = u32::MAX;
        assert_eq!(allocate_id(&active, &mut next), Some(u32::MAX));
        assert_eq!(allocate_id(&active, &mut next), Some(2));
    }

    #[test]
    fn ui_queue_overflow_resets_to_authoritative_active_state() {
        let active = BTreeMap::from([(
            7,
            Notification {
                view: NotificationView {
                    id: 7,
                    app_name: "Files".to_owned(),
                    summary: "Copying".to_owned(),
                    body: "one file".to_owned(),
                    urgency: NotificationUrgency::Normal,
                    progress: Some(50),
                },
                expires_at: None,
            },
        )]);
        let mut updates = VecDeque::from(vec![
            NotificationUiUpdate::Close {
                id: 1,
                reason: NotificationCloseReason::Closed,
            };
            MAX_UI_UPDATES
        ]);
        push_update(
            &mut updates,
            &active,
            NotificationUiUpdate::Close {
                id: 2,
                reason: NotificationCloseReason::Closed,
            },
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates.front(),
            Some(&NotificationUiUpdate::Reset(vec![active[&7].view.clone()]))
        );
    }

    #[test]
    fn close_signal_carries_id_and_standard_reason() {
        let signal = closed_signal(42, NotificationCloseReason::Expired);
        assert_eq!(signal.member().unwrap().to_string(), "NotificationClosed");
        assert_eq!(
            signal.get_items(),
            vec![MessageItem::UInt32(42), MessageItem::UInt32(1)]
        );
    }

    fn notify_call(app: &str, summary: &str, urgency: u8) -> Message {
        let mut call = Message::new_method_call(
            NOTIFICATIONS_SERVICE,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
            "Notify",
        )
        .expect("notify call");
        let hints = MessageItem::from_dict(
            vec![Ok::<_, ()>((
                "urgency".to_owned(),
                MessageItem::Byte(urgency),
            ))]
            .into_iter(),
        )
        .expect("hints");
        // Replies need a non-zero serial, which a real bus assigns on send.
        call.set_serial(1);
        call.append_items(&[
            MessageItem::Str(app.to_owned()),
            MessageItem::UInt32(0),
            MessageItem::Str(String::new()),
            MessageItem::Str(summary.to_owned()),
            MessageItem::Str(String::new()),
            MessageItem::new_array(vec![MessageItem::Str("default".to_owned())]).expect("actions"),
            hints,
            MessageItem::Int32(-1),
        ]);
        call
    }

    struct Inbox {
        notifications: BTreeMap<u32, Notification>,
        updates: VecDeque<NotificationUiUpdate>,
        next_id: u32,
        history: NotificationHistory,
    }

    impl Inbox {
        fn new(history: NotificationHistory) -> Self {
            Self {
                notifications: BTreeMap::new(),
                updates: VecDeque::new(),
                next_id: 1,
                history,
            }
        }

        fn notify(&mut self, app: &str, summary: &str, urgency: u8) -> MessageDisposition {
            handle_message(
                &notify_call(app, summary, urgency),
                true,
                &mut self.notifications,
                &mut self.updates,
                &mut self.next_id,
                &mut self.history,
                &mut Vec::new(),
                Instant::now(),
            )
        }
    }

    #[test]
    fn dnd_records_but_does_not_toast_until_critical() {
        let mut inbox = Inbox::new(NotificationHistory::new(10));
        inbox.history.set_dnd(true);

        assert!(matches!(
            inbox.notify("chat", "quiet", 1),
            MessageDisposition::Reply(_)
        ));
        assert!(inbox.notifications.is_empty());
        assert!(inbox.updates.is_empty());
        assert_eq!(inbox.history.len(), 1);

        inbox.notify("chat", "alarm", 2);
        assert_eq!(inbox.notifications.len(), 1);
        assert_eq!(inbox.updates.len(), 1);
        assert_eq!(inbox.history.len(), 2);
    }

    #[test]
    fn muted_app_is_recorded_without_toast_even_when_critical() {
        let mut history = NotificationHistory::new(10);
        history.set_app_muted("spam", true);
        let mut inbox = Inbox::new(history);

        inbox.notify("spam", "urgent", 2);
        assert!(inbox.notifications.is_empty());
        assert_eq!(inbox.history.len(), 1);
        assert!(inbox.history.groups()[0].muted);
    }

    #[test]
    fn normal_notification_toasts_and_is_recorded_once() {
        let mut inbox = Inbox::new(NotificationHistory::new(10));
        inbox.notify("mail", "hello", 1);
        assert_eq!(inbox.notifications.len(), 1);
        assert_eq!(inbox.history.len(), 1);
        assert_eq!(
            inbox.history.entries().next().unwrap().view.summary,
            "hello"
        );
    }
}
