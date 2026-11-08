//! D-Bus notification hint, action and markup parsing.

use std::time::Duration;

use dbus::arg::messageitem::MessageItem;

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Hints {
    pub(super) urgency: NotificationUrgency,
    pub(super) progress: Option<u8>,
    pub(super) transient: bool,
    pub(super) resident: bool,
    pub(super) image: Option<NotificationImage>,
    pub(super) image_path: Option<String>,
    pub(super) desktop_entry: Option<String>,
}

pub(super) fn parse_hints(hints: &dbus::arg::messageitem::MessageItemDict) -> Hints {
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
pub(super) fn parse_actions(items: &[MessageItem]) -> (Vec<NotificationAction>, bool) {
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
pub(super) fn action_advertised(view: &NotificationView, key: &str) -> bool {
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

pub(super) fn push_decoded(out: &mut String, text: &str) {
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

pub(super) fn unwrap_variant(value: MessageItem) -> MessageItem {
    match value {
        MessageItem::Variant(value) => unwrap_variant(*value),
        value => value,
    }
}

/// Spec `expire_timeout`: 0 never expires, -1 uses the server default, otherwise milliseconds.
/// Critical notifications never auto-expire; low urgency uses a shorter default.
pub(super) fn expiration(timeout_ms: i32, urgency: NotificationUrgency) -> Option<Duration> {
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
