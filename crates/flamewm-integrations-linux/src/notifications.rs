//! Minimal freedesktop notification server backed by `FlameWM`'s session D-Bus reactor.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::io::Read;
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
const LOW_URGENCY_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const HISTORY_CAPACITY: usize = 200;
const HISTORY_FILE: &str = "flamewm/notification-history.tsv";
/// Action buttons shown per toast or row; the `default` action is not counted here.
pub const MAX_VISIBLE_ACTIONS: usize = 3;
pub const DEFAULT_ACTION_KEY: &str = "default";
/// Image hints above these caps are ignored before any pixel data is decoded.
const MAX_IMAGE_EDGE: i32 = 512;
const MAX_IMAGE_BYTES: usize = 1024 * 1024;
/// Largest PNG file read for an absolute `image-path`; the same edge cap as `image-data`.
const MAX_IMAGE_PATH_BYTES: u64 = 1024 * 1024;
/// Largest edge of a decoded notification icon; larger images are downscaled once on receipt.
pub const NOTIFICATION_ICON_EDGE: u32 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationCloseReason {
    /// The timeout elapsed (spec reason 1).
    Expired,
    /// Dismissed by the user, or closed after an action was invoked (spec reason 2).
    Dismissed,
    /// Closed by a `CloseNotification` call (spec reason 3).
    Closed,
    /// Closed by the server for an undefined reason (spec reason 4).
    Undefined,
}

impl NotificationCloseReason {
    #[must_use]
    pub const fn wire_value(self) -> u32 {
        match self {
            Self::Expired => 1,
            Self::Dismissed => 2,
            Self::Closed => 3,
            Self::Undefined => 4,
        }
    }
}

/// One `(key, label)` pair from the `actions` argument. `default` is carried by
/// `NotificationView::default_action` instead of appearing in the visible button list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationView {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub urgency: NotificationUrgency,
    pub progress: Option<u8>,
    /// At most `MAX_VISIBLE_ACTIONS` non-default actions, in client order.
    pub actions: Vec<NotificationAction>,
    /// True when the client sent a `default` action, invoked by clicking the toast body.
    pub default_action: bool,
    /// Decoded `image-data` hint, already downscaled to `NOTIFICATION_ICON_EDGE`.
    pub image: Option<NotificationImage>,
    /// Icon-theme name from `image-path`, `app_icon`, or `desktop-entry`, in that order.
    pub icon_name: Option<String>,
}

/// Straight RGBA8 pixels, row-major, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
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
    /// Resident notifications stay on screen after one of their actions is invoked.
    resident: bool,
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
                eprintln!("flamewm-notifications: saving notification history failed: {error}");
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
                reason: NotificationCloseReason::Undefined,
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

    /// Invokes `key` on an active notification from the UI. Emits `ActionInvoked`, then closes
    /// the notification with reason 2 unless it is resident. Returns false for unknown ids and
    /// keys that the notification did not advertise.
    pub fn invoke_action(&mut self, id: u32, key: &str) -> FlameResult<bool> {
        let Some(notification) = self.notifications.get(&id) else {
            return Ok(false);
        };
        if !action_advertised(&notification.view, key) {
            return Ok(false);
        }
        let resident = notification.resident;
        self.pump.send(action_invoked_signal(id, key))?;
        if !resident {
            self.notifications.remove(&id);
            self.push_update(NotificationUiUpdate::Close {
                id,
                reason: NotificationCloseReason::Dismissed,
            });
            self.pump
                .send(closed_signal(id, NotificationCloseReason::Dismissed))?;
        }
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
            // Only what is implemented: body text is plain (markup stripped), and the shell
            // renders action buttons. No `body-markup`, `icon-static` or `persistence`.
            MessageDisposition::Reply(
                message
                    .method_return()
                    .append1(vec!["actions".to_owned(), "body".to_owned()]),
            )
        }
        Some("GetServerInformation") if message.get_items().is_empty() => {
            MessageDisposition::Reply(
                message
                    .method_return()
                    .append1("FlameWM")
                    .append1("ArkFlame Studios")
                    .append1(env!("CARGO_PKG_VERSION"))
                    .append1("1.2"),
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
        MessageItem::Str(app_icon),
        MessageItem::Str(summary),
        MessageItem::Str(body),
        MessageItem::Array(actions),
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
    let parsed = parse_hints(hints);
    let urgency = parsed.urgency;
    let (actions, default_action) = parse_actions(actions);
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
    let image = parsed
        .image
        .clone()
        .or_else(|| parsed.image_path.as_deref().and_then(image_path_hint));
    let view = NotificationView {
        id,
        app_name: app_name.clone(),
        summary: sanitize_markup(summary),
        body: sanitize_markup(body),
        urgency,
        progress: parsed.progress,
        actions,
        default_action,
        image: image.clone(),
        icon_name: if image.is_some() {
            None
        } else {
            icon_name_for(&parsed, app_icon)
        },
    };
    // Transient notifications never enter history; a replacement is not a new history entry.
    if !replacing && !parsed.transient {
        history.record(view.clone(), epoch_millis());
    }
    if decision == PopupDecision::Show {
        notifications.insert(
            id,
            Notification {
                view: view.clone(),
                expires_at: expiration(*timeout, urgency).map(|duration| now + duration),
                resident: parsed.resident,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hints {
    urgency: NotificationUrgency,
    progress: Option<u8>,
    transient: bool,
    resident: bool,
    image: Option<NotificationImage>,
    image_path: Option<String>,
    desktop_entry: Option<String>,
}

fn parse_hints(hints: &dbus::arg::messageitem::MessageItemDict) -> Hints {
    let mut parsed = Hints {
        urgency: NotificationUrgency::Normal,
        progress: None,
        transient: false,
        resident: false,
        image: None,
        image_path: None,
        desktop_entry: None,
    };
    for (key, value) in hints.clone().into_vec() {
        let MessageItem::Str(key) = key else { continue };
        let value = unwrap_variant(value);
        match (key.as_str(), value) {
            ("urgency", MessageItem::Byte(0)) => parsed.urgency = NotificationUrgency::Low,
            ("urgency", MessageItem::Byte(1)) => parsed.urgency = NotificationUrgency::Normal,
            ("urgency", MessageItem::Byte(2)) => parsed.urgency = NotificationUrgency::Critical,
            ("value", MessageItem::Int32(value)) if (0..=100).contains(&value) => {
                parsed.progress = Some(value as u8);
            }
            ("transient", MessageItem::Bool(value)) => parsed.transient = value,
            ("resident", MessageItem::Bool(value)) => parsed.resident = value,
            ("image-data" | "image_data" | "icon_data", value @ MessageItem::Struct(_)) => {
                parsed.image = image_data_hint(&value);
            }
            ("image-path" | "image_path", MessageItem::Str(path)) => {
                parsed.image_path = Some(path);
            }
            ("desktop-entry", MessageItem::Str(entry)) => parsed.desktop_entry = Some(entry),
            // `x-canonical-private-synchronous`, `sound-*`, and unknown hints are ignored.
            _ => {}
        }
    }
    parsed
}

/// Parses the flat `(key, label, key, label, ...)` array. Returns at most
/// `MAX_VISIBLE_ACTIONS` visible actions and whether a `default` action was present.
fn parse_actions(items: &[MessageItem]) -> (Vec<NotificationAction>, bool) {
    let mut actions = Vec::new();
    let mut default_action = false;
    for pair in items.chunks_exact(2) {
        let (MessageItem::Str(key), MessageItem::Str(label)) = (&pair[0], &pair[1]) else {
            continue;
        };
        if key == DEFAULT_ACTION_KEY {
            default_action = true;
        } else if actions.len() < MAX_VISIBLE_ACTIONS && !key.is_empty() {
            actions.push(NotificationAction {
                key: key.clone(),
                label: sanitize_markup(label),
            });
        }
    }
    (actions, default_action)
}

/// True when a client-advertised action key may be invoked on this notification.
fn action_advertised(view: &NotificationView, key: &str) -> bool {
    if key == DEFAULT_ACTION_KEY {
        view.default_action
    } else {
        view.actions.iter().any(|action| action.key == key)
    }
}

/// Plain text for a body or summary: spec markup tags are removed, entities are decoded, and
/// the result is only ever shown as text nodes, so it cannot inject markup or links.
#[must_use]
pub fn sanitize_markup(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        push_decoded(&mut out, &rest[..start]);
        let tag = &rest[start..];
        let is_tag = tag[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        match tag.find('>') {
            Some(end) if is_tag => {
                if tag[1..end].trim().eq_ignore_ascii_case("br")
                    || tag[1..end].trim().eq_ignore_ascii_case("br/")
                {
                    out.push('\n');
                }
                rest = &tag[end + 1..];
            }
            _ => {
                // A lone `<` or an unterminated tag is text.
                out.push('<');
                rest = &tag[1..];
            }
        }
    }
    push_decoded(&mut out, rest);
    out
}

fn push_decoded(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        let Some(semi) = after.find(';').filter(|&semi| semi <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let name = &after[1..semi];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix('#')
                .and_then(|number| {
                    number.strip_prefix(['x', 'X']).map_or_else(
                        || number.parse().ok(),
                        |hex| u32::from_str_radix(hex, 16).ok(),
                    )
                })
                .and_then(char::from_u32)
                .filter(|c| !c.is_control() || *c == '\n' || *c == '\t'),
        };
        if let Some(c) = decoded {
            out.push(c);
            rest = &after[semi + 1..];
        } else {
            out.push('&');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
}

/// Image hint caps, checked from the `(iiibiiay)` header before any pixel bytes are read.
/// Rejects non-8-bit or non RGB/RGBA data, edges over `MAX_IMAGE_EDGE`, a rowstride or byte
/// count that does not match the declared size, and payloads over `MAX_IMAGE_BYTES`.
#[must_use]
pub fn image_data_within_caps(
    width: i32,
    height: i32,
    rowstride: i32,
    bits_per_sample: i32,
    channels: i32,
    byte_len: usize,
) -> bool {
    if !(1..=MAX_IMAGE_EDGE).contains(&width) || !(1..=MAX_IMAGE_EDGE).contains(&height) {
        return false;
    }
    if bits_per_sample != 8 || !matches!(channels, 3 | 4) {
        return false;
    }
    let (Ok(width), Ok(height), Ok(stride)) = (
        usize::try_from(width),
        usize::try_from(height),
        usize::try_from(rowstride),
    ) else {
        return false;
    };
    let min_stride = width * usize::try_from(channels).unwrap_or(4);
    stride >= min_stride && byte_len <= MAX_IMAGE_BYTES && byte_len == stride.saturating_mul(height)
}

fn unwrap_variant(value: MessageItem) -> MessageItem {
    match value {
        MessageItem::Variant(value) => unwrap_variant(*value),
        value => value,
    }
}

/// Spec `expire_timeout`: 0 never expires, -1 uses the server default, otherwise milliseconds.
/// Critical notifications never auto-expire; low urgency uses a shorter default.
fn expiration(timeout_ms: i32, urgency: NotificationUrgency) -> Option<Duration> {
    if timeout_ms == 0 || urgency == NotificationUrgency::Critical {
        return None;
    }
    let duration = if timeout_ms < 0 {
        match urgency {
            NotificationUrgency::Low => LOW_URGENCY_TIMEOUT,
            _ => DEFAULT_TIMEOUT,
        }
    } else {
        Duration::from_millis(u64::from(timeout_ms.unsigned_abs()))
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

/// Validates the `(iiibiiay)` header and payload type before copying any bytes, then
/// downscales. Invalid or oversized images are dropped without an error.
fn image_data_hint(value: &MessageItem) -> Option<NotificationImage> {
    let MessageItem::Struct(fields) = value else {
        return None;
    };
    let [
        MessageItem::Int32(width),
        MessageItem::Int32(height),
        MessageItem::Int32(rowstride),
        MessageItem::Bool(_),
        MessageItem::Int32(bits),
        MessageItem::Int32(channels),
        MessageItem::Array(data),
    ] = fields.as_slice()
    else {
        return None;
    };
    if !image_data_within_caps(*width, *height, *rowstride, *bits, *channels, data.len()) {
        return None;
    }
    let bytes = data
        .iter()
        .map(|item| match item {
            MessageItem::Byte(byte) => Some(*byte),
            _ => None,
        })
        .collect::<Option<Vec<u8>>>()?;
    downscale_to_icon(
        u32::try_from(*width).ok()?,
        u32::try_from(*height).ok()?,
        usize::try_from(*channels).ok()?,
        usize::try_from(*rowstride).ok()?,
        &bytes,
    )
}

/// Nearest-neighbour downscale of a validated buffer so that neither edge exceeds
/// `NOTIFICATION_ICON_EDGE`. Output is always RGBA with alpha 255 when the source has none.
#[must_use]
pub fn downscale_to_icon(
    width: u32,
    height: u32,
    channels: usize,
    rowstride: usize,
    data: &[u8],
) -> Option<NotificationImage> {
    if width == 0 || height == 0 || !(3..=4).contains(&channels) {
        return None;
    }
    let step = width.max(height).div_ceil(NOTIFICATION_ICON_EDGE).max(1);
    let out_width = width.div_ceil(step);
    let out_height = height.div_ceil(step);
    let mut rgba = Vec::with_capacity((out_width * out_height * 4) as usize);
    for out_y in 0..out_height {
        let src_y = (out_y * step) as usize;
        for out_x in 0..out_width {
            let offset = src_y * rowstride + (out_x * step) as usize * channels;
            let pixel = data.get(offset..offset + channels)?;
            rgba.extend_from_slice(&[
                pixel[0],
                pixel[1],
                pixel[2],
                if channels == 4 { pixel[3] } else { 255 },
            ]);
        }
    }
    Some(NotificationImage {
        width: out_width,
        height: out_height,
        rgba,
    })
}

/// Decodes an absolute PNG `image-path` (plain path or `file://` URI) into a toast icon. The file
/// must be a regular file within `MAX_IMAGE_PATH_BYTES`, and its IHDR edges must fit
/// `MAX_IMAGE_EDGE`, before pixels are decoded. The result is downscaled once, as `image-data` is.
/// Anything else returns `None`, and the named-icon fallback applies.
fn image_path_hint(reference: &str) -> Option<NotificationImage> {
    let reference = reference.trim();
    let path = match reference.strip_prefix("file://") {
        Some(rest) => percent_decode(rest)?,
        None => reference.to_owned(),
    };
    if !path.starts_with('/') || !path.to_ascii_lowercase().ends_with(".png") {
        return None;
    }
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_PATH_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(MAX_IMAGE_PATH_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_IMAGE_PATH_BYTES {
        return None;
    }
    // A PNG's IHDR carries big-endian width and height at offsets 16 and 20.
    let header = bytes.get(16..24)?;
    let width = u32::from_be_bytes(header[..4].try_into().ok()?);
    let height = u32::from_be_bytes(header[4..].try_into().ok()?);
    if width == 0 || height == 0 || i64::from(width.max(height)) > i64::from(MAX_IMAGE_EDGE) {
        return None;
    }
    let decoded = flamewm_image_core::png::decode(&bytes).ok()?;
    let stride = usize::try_from(decoded.width).ok()?.checked_mul(4)?;
    downscale_to_icon(decoded.width, decoded.height, 4, stride, &decoded.pixels)
}

/// Decodes `%XX` escapes of a `file://` URI path. Malformed escapes or non-UTF-8 results are
/// rejected.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Icon-theme names are plain names; absolute paths and URIs are not decoded here.
fn icon_name_hint(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && !name.contains('/') && name.len() <= 255).then(|| name.to_owned())
}

fn icon_name_for(parsed: &Hints, app_icon: &str) -> Option<String> {
    parsed
        .image_path
        .as_deref()
        .and_then(icon_name_hint)
        .or_else(|| icon_name_hint(app_icon))
        .or_else(|| parsed.desktop_entry.as_deref().and_then(icon_name_hint))
}

fn action_invoked_signal(id: u32, key: &str) -> Message {
    Message::new_signal(NOTIFICATIONS_PATH, NOTIFICATIONS_INTERFACE, "ActionInvoked")
        .expect("constant notification signal identifiers are valid")
        .append2(id, key.to_owned())
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
            ("transient".to_owned(), MessageItem::Bool(true)),
            ("resident".to_owned(), MessageItem::Bool(true)),
        ]);
        assert_eq!(
            parse_hints(&dict),
            Hints {
                urgency: NotificationUrgency::Critical,
                progress: Some(55),
                transient: true,
                resident: true,
                image: None,
                image_path: None,
                desktop_entry: None,
            }
        );
        let out_of_range = hints(vec![("value".to_owned(), MessageItem::Int32(101))]);
        assert_eq!(parse_hints(&out_of_range).progress, None);
        assert_eq!(
            expiration(i32::MAX, NotificationUrgency::Normal),
            Some(MAX_TIMEOUT)
        );
        assert_eq!(expiration(0, NotificationUrgency::Normal), None);
        assert_eq!(
            expiration(-1, NotificationUrgency::Normal),
            Some(DEFAULT_TIMEOUT)
        );
        assert_eq!(
            expiration(500, NotificationUrgency::Normal),
            Some(Duration::from_millis(500))
        );
    }

    #[test]
    fn image_data_is_downscaled_once_to_icon_edge() {
        // 96x2 RGB: step 2 gives 48x1, sampling every other column.
        let mut data = Vec::new();
        for x in 0..96u32 {
            data.extend_from_slice(&[x as u8, 0, 0]);
        }
        let image = downscale_to_icon(96, 1, 3, 96 * 3, &data).expect("valid image");
        assert_eq!((image.width, image.height), (48, 1));
        assert_eq!(image.rgba.len(), 48 * 4);
        assert_eq!(&image.rgba[..4], &[0, 0, 0, 255]);
        assert_eq!(&image.rgba[4..8], &[2, 0, 0, 255]);

        // Small RGBA images are kept at native size and keep their alpha.
        let small = downscale_to_icon(1, 1, 4, 4, &[9, 8, 7, 6]).expect("valid image");
        assert_eq!(small.rgba, [9, 8, 7, 6]);
        assert!(downscale_to_icon(0, 1, 4, 4, &[]).is_none());
        assert!(downscale_to_icon(1, 1, 2, 2, &[0, 0]).is_none());
    }

    #[test]
    fn image_data_hint_rejects_oversized_and_mismatched_payloads() {
        let image = |width: i32, height: i32, stride: i32, bytes: usize| {
            MessageItem::Struct(vec![
                MessageItem::Int32(width),
                MessageItem::Int32(height),
                MessageItem::Int32(stride),
                MessageItem::Bool(true),
                MessageItem::Int32(8),
                MessageItem::Int32(4),
                MessageItem::new_array((0..bytes).map(|_| MessageItem::Byte(0)).collect())
                    .expect("bytes"),
            ])
        };
        let accepted = image_data_hint(&image(2, 2, 8, 16)).expect("2x2 RGBA");
        assert_eq!(
            (accepted.width, accepted.height, accepted.rgba.len()),
            (2, 2, 16)
        );
        assert!(image_data_hint(&image(513, 1, 2052, 2052)).is_none());
        assert!(image_data_hint(&image(2, 2, 8, 15)).is_none());
        assert!(image_data_hint(&MessageItem::Str("x".to_owned())).is_none());
    }

    #[test]
    fn icon_name_precedence_is_image_path_then_app_icon_then_desktop_entry() {
        let base = Hints {
            urgency: NotificationUrgency::Normal,
            progress: None,
            transient: false,
            resident: false,
            image: None,
            image_path: None,
            desktop_entry: Some("org.example.App".to_owned()),
        };
        assert_eq!(icon_name_for(&base, "mail").as_deref(), Some("mail"));
        let with_path = Hints {
            image_path: Some("camera-photo".to_owned()),
            ..base.clone()
        };
        assert_eq!(
            icon_name_for(&with_path, "mail").as_deref(),
            Some("camera-photo")
        );
        assert_eq!(icon_name_for(&base, "").as_deref(), Some("org.example.App"));
        let absolute = Hints {
            image_path: Some("/tmp/x.png".to_owned()),
            ..base.clone()
        };
        assert_eq!(icon_name_for(&absolute, "mail").as_deref(), Some("mail"));
    }

    /// Stored-deflate RGBA PNG whose pixels are all opaque grey, for `image-path` tests.
    fn png_fixture(width: u32, height: u32) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
            out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }
        let mut raw = Vec::new();
        for _ in 0..height {
            raw.push(0);
            raw.extend(std::iter::repeat_n(0x80u8, width as usize * 4));
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::none());
        std::io::Write::write_all(&mut encoder, &raw).unwrap();
        let idat = encoder.finish().unwrap();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut out, *b"IHDR", &ihdr);
        chunk(&mut out, *b"IDAT", &idat);
        chunk(&mut out, *b"IEND", &[]);
        out
    }

    #[test]
    fn image_path_decodes_a_bounded_png_downscaled_once() {
        let dir = std::env::temp_dir().join(format!("flamewm-nf-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wide = dir.join("wide.png");
        std::fs::write(&wide, png_fixture(96, 2)).unwrap();
        let image = image_path_hint(wide.to_str().unwrap()).expect("96x2 png decodes");
        assert_eq!(
            (image.width, image.height),
            (48, 1),
            "one step of 2 per axis"
        );
        assert_eq!(image.rgba.len(), 48 * 4);
        assert!(image_path_hint(&format!("file://{}", wide.display())).is_some());
        assert!(
            image_path_hint("camera-photo").is_none(),
            "names use the icon theme"
        );
        assert!(image_path_hint("relative/x.png").is_none());
        assert!(
            image_path_hint("/dev/null").is_none(),
            "only regular files are read"
        );
        assert!(image_path_hint(dir.join("missing.png").to_str().unwrap()).is_none());
        let text = dir.join("text.png");
        std::fs::write(&text, b"plain text that is not a png file at all").unwrap();
        assert!(image_path_hint(text.to_str().unwrap()).is_none());
        let too_wide = dir.join("too-wide.png");
        std::fs::write(&too_wide, png_fixture(513, 1)).unwrap();
        assert!(image_path_hint(too_wide.to_str().unwrap()).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn percent_decode_handles_file_uri_escapes() {
        assert_eq!(
            percent_decode("/a%20b/c.png").as_deref(),
            Some("/a b/c.png")
        );
        assert_eq!(percent_decode("/a%2"), None);
        assert_eq!(percent_decode("/a%zz"), None);
    }

    #[test]
    fn expire_timeout_follows_urgency_policy() {
        // Critical never auto-expires, even with an explicit timeout.
        assert_eq!(expiration(-1, NotificationUrgency::Critical), None);
        assert_eq!(expiration(4000, NotificationUrgency::Critical), None);
        // Low urgency defaults shorter than normal; an explicit timeout is honored.
        assert_eq!(
            expiration(-1, NotificationUrgency::Low),
            Some(LOW_URGENCY_TIMEOUT)
        );
        assert_eq!(
            expiration(9000, NotificationUrgency::Low),
            Some(Duration::from_secs(9))
        );
    }

    #[test]
    fn close_reason_codes_match_the_spec() {
        assert_eq!(NotificationCloseReason::Expired.wire_value(), 1);
        assert_eq!(NotificationCloseReason::Dismissed.wire_value(), 2);
        assert_eq!(NotificationCloseReason::Closed.wire_value(), 3);
        assert_eq!(NotificationCloseReason::Undefined.wire_value(), 4);
    }

    #[test]
    fn actions_parse_pairs_with_default_and_visible_cap() {
        let items = [
            MessageItem::Str("default".to_owned()),
            MessageItem::Str("Open".to_owned()),
            MessageItem::Str("reply".to_owned()),
            MessageItem::Str("Reply".to_owned()),
            MessageItem::Str("a".to_owned()),
            MessageItem::Str("A".to_owned()),
            MessageItem::Str("b".to_owned()),
            MessageItem::Str("B".to_owned()),
            MessageItem::Str("c".to_owned()),
            MessageItem::Str("C".to_owned()),
            MessageItem::Str("dangling".to_owned()),
        ];
        let (actions, default_action) = parse_actions(&items);
        assert!(default_action);
        let keys: Vec<_> = actions.iter().map(|action| action.key.as_str()).collect();
        assert_eq!(keys, ["reply", "a", "b"]);
        assert_eq!(actions[0].label, "Reply");

        let view = NotificationView {
            id: 1,
            app_name: String::new(),
            summary: String::new(),
            body: String::new(),
            urgency: NotificationUrgency::Normal,
            progress: None,
            actions,
            default_action,
            image: None,
            icon_name: None,
        };
        assert!(action_advertised(&view, "default"));
        assert!(action_advertised(&view, "b"));
        assert!(
            !action_advertised(&view, "c"),
            "hidden fourth action is not invokable"
        );
        assert!(!action_advertised(&view, "missing"));
    }

    #[test]
    fn markup_is_reduced_to_safe_plain_text() {
        assert_eq!(
            sanitize_markup("<b>bold</b> <i>it</i> <u>u</u>"),
            "bold it u"
        );
        assert_eq!(
            sanitize_markup(r#"<a href="javascript:alert(1)">link</a>"#),
            "link"
        );
        assert_eq!(sanitize_markup("<img src=x onerror=y>x"), "x");
        assert_eq!(
            sanitize_markup("a &amp; b &lt;c&gt; &quot;q&quot; &#65;&#x42;"),
            "a & b <c> \"q\" AB"
        );
        // Entities decode to text only; decoded markup is never re-parsed.
        assert_eq!(sanitize_markup("&lt;script&gt;"), "<script>");
        assert_eq!(sanitize_markup("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
        assert_eq!(sanitize_markup("line<br>two"), "line\ntwo");
        assert_eq!(sanitize_markup("unterminated <b"), "unterminated <b");
        // Control characters and unknown names stay literal instead of decoding.
        assert_eq!(sanitize_markup("&#0;&unknown;"), "&#0;&unknown;");
    }

    #[test]
    fn image_hint_caps_are_checked_before_decode() {
        // 2x2 RGBA with a matching rowstride and byte count is accepted.
        assert!(image_data_within_caps(2, 2, 8, 8, 4, 16));
        assert!(!image_data_within_caps(513, 1, 2052, 8, 4, 2052));
        assert!(
            !image_data_within_caps(2, 2, 8, 16, 4, 16),
            "only 8-bit samples"
        );
        assert!(
            !image_data_within_caps(2, 2, 8, 8, 2, 16),
            "only RGB or RGBA"
        );
        assert!(
            !image_data_within_caps(2, 2, 8, 8, 4, 15),
            "byte count mismatch"
        );
        assert!(
            !image_data_within_caps(2, 2, 7, 8, 4, 14),
            "rowstride too small"
        );
        assert!(!image_data_within_caps(
            512,
            512,
            2048,
            8,
            4,
            512 * 2048 + 1
        ));
        assert!(image_data_within_caps(512, 512, 2048, 8, 4, 512 * 2048));
        assert!(!image_data_within_caps(0, 2, 8, 8, 4, 0));
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
                    actions: Vec::new(),
                    default_action: false,
                    image: None,
                    icon_name: None,
                },
                expires_at: None,
                resident: false,
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
                    actions: Vec::new(),
                    default_action: false,
                    image: None,
                    icon_name: None,
                },
                expires_at: None,
                resident: false,
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

    #[test]
    fn transient_skips_history_and_markup_is_stripped_on_notify() {
        let mut inbox = Inbox::new(NotificationHistory::new(10));
        let transient = MessageItem::from_dict(
            vec![
                Ok::<_, ()>(("transient".to_owned(), MessageItem::Bool(true))),
                Ok(("resident".to_owned(), MessageItem::Bool(true))),
            ]
            .into_iter(),
        )
        .expect("hints");
        let mut call = Message::new_method_call(
            NOTIFICATIONS_SERVICE,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
            "Notify",
        )
        .expect("notify call");
        call.set_serial(1);
        call.append_items(&[
            MessageItem::Str("chat".to_owned()),
            MessageItem::UInt32(0),
            MessageItem::Str(String::new()),
            MessageItem::Str("<b>hi</b>".to_owned()),
            MessageItem::Str(String::new()),
            MessageItem::new_array(vec![
                MessageItem::Str("default".to_owned()),
                MessageItem::Str("Open".to_owned()),
            ])
            .expect("actions"),
            transient,
            MessageItem::Int32(-1),
        ]);
        assert!(matches!(
            handle_message(
                &call,
                true,
                &mut inbox.notifications,
                &mut inbox.updates,
                &mut inbox.next_id,
                &mut inbox.history,
                &mut Vec::new(),
                Instant::now(),
            ),
            MessageDisposition::Reply(_)
        ));
        assert_eq!(inbox.history.len(), 0, "transient is not recorded");
        let view = &inbox.notifications[&1].view;
        assert_eq!(view.summary, "hi");
        assert!(inbox.notifications[&1].resident);
    }
}
