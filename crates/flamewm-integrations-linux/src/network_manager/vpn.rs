//! `NetworkManager` VPN profile lookup (`net-vpn-lookup`, slice 1 of `docs/backlog/network.md`).
//!
//! Pure message builder and reply parser for `Settings.GetConnectionByUuid`, which resolves a
//! profile UUID to its settings object path. No bus I/O, no event source, no UI: nothing in
//! `NetworkManagerProvider` sends these yet. Activation and deactivation are later slices.

use dbus::arg::messageitem::MessageItem;
use dbus::{Message, MessageType};
use flamewm_api::{FlameError, FlameResult};

use super::NETWORK_MANAGER_SERVICE;

const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_INTERFACE: &str = "org.freedesktop.NetworkManager.Settings";
const GET_CONNECTION_BY_UUID: &str = "GetConnectionByUuid";

/// True for the canonical 8-4-4-4-12 hex form that `NetworkManager` uses for profile UUIDs.
#[must_use]
pub fn is_connection_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// `GetConnectionByUuid(s) -> o` on the `NetworkManager` settings object. Rejects malformed UUIDs
/// before any message is built, so a bad profile id never reaches the bus.
pub fn get_connection_by_uuid_message(uuid: &str) -> FlameResult<Message> {
    if !is_connection_uuid(uuid) {
        return Err(FlameError::invalid("network manager connection uuid"));
    }
    let mut message = Message::new_method_call(
        NETWORK_MANAGER_SERVICE,
        SETTINGS_PATH,
        SETTINGS_INTERFACE,
        GET_CONNECTION_BY_UUID,
    )
    .map_err(|error| FlameError::invalid(format!("network manager message: {error}")))?;
    message.append_items(&[MessageItem::Str(uuid.to_owned())]);
    Ok(message)
}

/// Settings object path from a `GetConnectionByUuid` reply. Pure. Error replies and any reply
/// that is not exactly one object path are rejected.
pub fn parse_connection_by_uuid_reply(reply: &Message) -> FlameResult<String> {
    if reply.msg_type() != MessageType::MethodReturn {
        return Err(FlameError::invalid(
            "network manager connection lookup reply",
        ));
    }
    match reply.get_items().as_slice() {
        [MessageItem::ObjectPath(path)] => Ok(path.to_string()),
        _ => Err(FlameError::invalid(
            "network manager connection lookup reply",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbus::strings::{ErrorName, Path};
    use std::ffi::CString;

    const UUID: &str = "3f1c2d4e-5a6b-4c7d-8e9f-0a1b2c3d4e5f";
    const PROFILE_PATH: &str = "/org/freedesktop/NetworkManager/Settings/7";

    /// Replies need a non-zero serial on the call, as on the wire; libdbus aborts otherwise.
    fn sent_call() -> Message {
        let mut call = get_connection_by_uuid_message(UUID).expect("valid uuid");
        call.set_serial(1);
        call
    }

    #[test]
    fn uuid_check_accepts_canonical_form_only() {
        assert!(is_connection_uuid(UUID));
        assert!(!is_connection_uuid(""));
        assert!(!is_connection_uuid("3f1c2d4e5a6b4c7d8e9f0a1b2c3d4e5f"));
        assert!(!is_connection_uuid("3f1c2d4e-5a6b-4c7d-8e9f-0a1b2c3d4e5g"));
        assert!(!is_connection_uuid("3f1c2d4e-5a6b-4c7d-8e9f-0a1b2c3d4e5f0"));
        assert!(!is_connection_uuid(
            "/org/freedesktop/NetworkManager/Settings/7"
        ));
    }

    #[test]
    fn lookup_message_targets_settings_with_uuid_argument() {
        let message = get_connection_by_uuid_message(UUID).expect("valid uuid");
        assert_eq!(
            message.destination().as_deref(),
            Some(NETWORK_MANAGER_SERVICE)
        );
        assert_eq!(message.path().as_deref(), Some(SETTINGS_PATH));
        assert_eq!(message.interface().as_deref(), Some(SETTINGS_INTERFACE));
        assert_eq!(message.member().as_deref(), Some(GET_CONNECTION_BY_UUID));
        assert_eq!(message.get_items(), vec![MessageItem::Str(UUID.to_owned())]);
    }

    #[test]
    fn lookup_message_rejects_malformed_uuid_before_building() {
        assert!(get_connection_by_uuid_message("not-a-uuid").is_err());
    }

    #[test]
    fn reply_parser_returns_settings_path() {
        let call = sent_call();
        let mut reply = call.method_return();
        reply.append_items(&[MessageItem::ObjectPath(
            Path::new(PROFILE_PATH.to_owned()).expect("path"),
        )]);
        assert_eq!(
            parse_connection_by_uuid_reply(&reply).as_deref(),
            Ok(PROFILE_PATH)
        );
    }

    #[test]
    fn reply_parser_rejects_error_and_wrong_shape() {
        let call = sent_call();
        let name = ErrorName::new("org.freedesktop.NetworkManager.Settings.InvalidConnection")
            .expect("error name");
        let text = CString::new("no such profile").expect("cstring");
        let error = call.error(&name, &text);
        assert!(parse_connection_by_uuid_reply(&error).is_err());

        let empty = call.method_return();
        assert!(parse_connection_by_uuid_reply(&empty).is_err());

        let mut wrong = call.method_return();
        wrong.append_items(&[MessageItem::Str(PROFILE_PATH.to_owned())]);
        assert!(parse_connection_by_uuid_reply(&wrong).is_err());
    }
}
