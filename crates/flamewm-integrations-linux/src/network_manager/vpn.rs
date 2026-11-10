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
    single_object_path(
        &reply.get_items(),
        "network manager connection lookup reply",
    )
}

/// The one object path in a reply body. Used by the reactor, which holds reply items, not messages.
pub(super) fn single_object_path(items: &[MessageItem], context: &str) -> FlameResult<String> {
    match items {
        [MessageItem::ObjectPath(path)] => Ok(path.to_string()),
        _ => Err(FlameError::invalid(context)),
    }
}

const ROOT_PATH: &str = "/org/freedesktop/NetworkManager";
const ROOT_INTERFACE: &str = "org.freedesktop.NetworkManager";
const ACTIVATE_CONNECTION: &str = "ActivateConnection";
/// `/` is the `NetworkManager` wildcard for "no device" and "no specific object", as VPN profiles need.
const NO_OBJECT_PATH: &str = "/";

/// `ActivateConnection(o connection, o device, o specific_object) -> o active_connection` for a
/// VPN profile. Takes a settings path from `GetConnectionByUuid`, so it must sit under the
/// settings object. Device and specific object are both `/`: VPN profiles bind to no device.
pub fn activate_vpn_message(profile_path: &str) -> FlameResult<Message> {
    let profile = dbus::Path::new(profile_path.to_owned())
        .map_err(|error| FlameError::invalid(format!("network manager profile path: {error}")))?;
    if !profile_path.starts_with(&format!("{SETTINGS_PATH}/")) {
        return Err(FlameError::invalid("network manager profile path"));
    }
    let no_object = dbus::Path::new(NO_OBJECT_PATH.to_owned())
        .map_err(|error| FlameError::invalid(format!("network manager message: {error}")))?;
    let mut message = Message::new_method_call(
        NETWORK_MANAGER_SERVICE,
        ROOT_PATH,
        ROOT_INTERFACE,
        ACTIVATE_CONNECTION,
    )
    .map_err(|error| FlameError::invalid(format!("network manager message: {error}")))?;
    message.append_items(&[
        MessageItem::ObjectPath(profile),
        MessageItem::ObjectPath(no_object.clone()),
        MessageItem::ObjectPath(no_object),
    ]);
    Ok(message)
}

/// Active-connection path from an `ActivateConnection` reply. Pure. Error replies and any reply
/// that is not exactly one object path are rejected.
pub fn parse_activate_vpn_reply(reply: &Message) -> FlameResult<String> {
    if reply.msg_type() != MessageType::MethodReturn {
        return Err(FlameError::invalid("network manager activate reply"));
    }
    single_object_path(&reply.get_items(), "network manager activate reply")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbus::strings::{ErrorName, Path};
    use std::ffi::CString;

    const UUID: &str = "3f1c2d4e-5a6b-4c7d-8e9f-0a1b2c3d4e5f";
    const PROFILE_PATH: &str = "/org/freedesktop/NetworkManager/Settings/7";
    const ACTIVE_PATH: &str = "/org/freedesktop/NetworkManager/ActiveConnection/3";

    #[test]
    fn activate_message_passes_profile_with_wildcard_device_and_object() {
        let message = activate_vpn_message(PROFILE_PATH).expect("settings path");
        assert_eq!(
            message.destination().as_deref(),
            Some(NETWORK_MANAGER_SERVICE)
        );
        assert_eq!(message.path().as_deref(), Some(ROOT_PATH));
        assert_eq!(message.interface().as_deref(), Some(ROOT_INTERFACE));
        assert_eq!(message.member().as_deref(), Some(ACTIVATE_CONNECTION));
        let root = Path::new(NO_OBJECT_PATH.to_owned()).expect("path");
        assert_eq!(
            message.get_items(),
            vec![
                MessageItem::ObjectPath(Path::new(PROFILE_PATH.to_owned()).expect("path")),
                MessageItem::ObjectPath(root.clone()),
                MessageItem::ObjectPath(root),
            ]
        );
    }

    #[test]
    fn activate_message_rejects_paths_outside_settings_or_malformed() {
        assert!(activate_vpn_message(ROOT_PATH).is_err());
        assert!(
            activate_vpn_message("/org/freedesktop/NetworkManager/ActiveConnection/3").is_err()
        );
        assert!(activate_vpn_message(UUID).is_err());
        assert!(activate_vpn_message("").is_err());
    }

    #[test]
    fn activate_reply_parser_returns_active_connection_path() {
        let mut call = activate_vpn_message(PROFILE_PATH).expect("settings path");
        call.set_serial(1);
        let mut reply = call.method_return();
        reply.append_items(&[MessageItem::ObjectPath(
            Path::new(ACTIVE_PATH.to_owned()).expect("path"),
        )]);
        assert_eq!(parse_activate_vpn_reply(&reply).as_deref(), Ok(ACTIVE_PATH));
    }

    #[test]
    fn lookup_items_feed_activate_message_for_the_resolved_profile() {
        let items = [MessageItem::ObjectPath(
            Path::new(PROFILE_PATH.to_owned()).expect("path"),
        )];
        let profile = single_object_path(&items, "lookup").expect("one object path");
        let message = activate_vpn_message(&profile).expect("settings path");
        assert_eq!(message.member().as_deref(), Some(ACTIVATE_CONNECTION));
        assert!(
            single_object_path(&[MessageItem::Str(PROFILE_PATH.to_owned())], "lookup").is_err()
        );
        assert!(single_object_path(&[], "lookup").is_err());
    }

    #[test]
    fn activate_reply_parser_rejects_error_and_wrong_shape() {
        let mut call = activate_vpn_message(PROFILE_PATH).expect("settings path");
        call.set_serial(1);
        let name =
            ErrorName::new("org.freedesktop.NetworkManager.UnknownConnection").expect("error name");
        let text = CString::new("no such profile").expect("cstring");
        assert!(parse_activate_vpn_reply(&call.error(&name, &text)).is_err());
        assert!(parse_activate_vpn_reply(&call.method_return()).is_err());
        let mut wrong = call.method_return();
        wrong.append_items(&[MessageItem::Str(ACTIVE_PATH.to_owned())]);
        assert!(parse_activate_vpn_reply(&wrong).is_err());
    }

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
