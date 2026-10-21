use super::actions::*;
use super::reply::*;
use super::rules::*;
use super::*;

#[test]
fn properties_rules_exclude_device_statistics_signal() {
    let rules: Vec<String> = NM_PROPERTY_INTERFACES
        .iter()
        .map(|interface| properties_changed_rule(interface))
        .collect();
    assert!(rules.iter().all(|rule| rule.contains("arg0='")));
    assert!(
        rules
            .iter()
            .any(|rule| rule.contains("arg0='org.freedesktop.NetworkManager.Device.Wireless'"))
    );
    assert!(
        rules
            .iter()
            .any(|rule| rule.contains("arg0='org.freedesktop.NetworkManager.Device'"))
    );
    assert!(!rules.iter().any(|rule| rule.contains("Device.Statistics")));
}

#[test]
fn register_with_capabilities_uses_identifier_and_capabilities_only() {
    let message = secret_agent_registration_message().expect("valid registration message");
    assert_eq!(
        message.member().map(|m| m.to_string()).as_deref(),
        Some(REGISTER_WITH_CAPABILITIES)
    );
    let items = message.get_items();
    let signature: String = items
        .iter()
        .map(|item| item.signature().to_string())
        .collect();
    assert_eq!(signature, "su");
    assert!(matches!(&items[0], MessageItem::Str(s) if s == SECRET_AGENT_IDENTIFIER));
    assert!(matches!(&items[1], MessageItem::UInt32(0)));
}

#[test]
fn method_call_helpers_match_direct_construction() {
    let helper = get_all_message(
        NETWORK_MANAGER_SERVICE,
        NETWORK_MANAGER_ROOT,
        NETWORK_MANAGER_INTERFACE,
    )
    .expect("valid GetAll message");
    let direct = Message::new_method_call(
        NETWORK_MANAGER_SERVICE,
        NETWORK_MANAGER_ROOT,
        PROPERTIES_INTERFACE,
        GET_ALL,
    )
    .expect("valid GetAll message")
    .append1(NETWORK_MANAGER_INTERFACE.to_owned());
    assert_eq!(helper.destination(), direct.destination());
    assert_eq!(helper.path(), direct.path());
    assert_eq!(helper.interface(), direct.interface());
    assert_eq!(helper.member(), direct.member());
    assert_eq!(helper.get_items(), direct.get_items());
    assert_eq!(
        helper.member().map(|m| m.to_string()).as_deref(),
        Some(GET_ALL)
    );

    let devices = method_call(
        NETWORK_MANAGER_SERVICE,
        NETWORK_MANAGER_ROOT,
        NETWORK_MANAGER_INTERFACE,
        GET_DEVICES,
    )
    .expect("valid GetDevices message");
    assert_eq!(
        devices.member().map(|m| m.to_string()).as_deref(),
        Some(GET_DEVICES)
    );
    assert_eq!(
        devices.interface().map(|i| i.to_string()).as_deref(),
        Some(NETWORK_MANAGER_INTERFACE)
    );
    assert!(devices.get_items().is_empty());
}

#[test]
fn access_point_parser_decodes_ssid_security_and_strength() {
    let values = vec![
        ("Ssid".to_owned(), MessageItem::from(&b"FlameNet"[..])),
        ("Strength".to_owned(), MessageItem::Byte(87)),
        ("Flags".to_owned(), MessageItem::UInt32(1)),
        ("WpaFlags".to_owned(), MessageItem::UInt32(0)),
        ("RsnFlags".to_owned(), MessageItem::UInt32(0)),
    ];
    let item = MessageItem::from_dict(values.into_iter().map(Ok::<_, FlameError>))
        .expect("valid D-Bus dictionary");
    assert_eq!(
        parse_access_point_properties(Some(&item)).expect("valid access point"),
        AccessPointProperties {
            ssid: "FlameNet".to_owned(),
            strength_percent: 87,
            secured: true,
        }
    );
}

#[test]
fn header_label_never_claims_connection_when_disconnected() {
    assert_eq!(header_label_for(None, false), "Not connected");
    let connected = AccessPoint {
        path: "/nm/ap/1".to_owned(),
        ssid: "FlameNet".to_owned(),
        strength_percent: 80,
        secured: true,
        known: true,
    };
    assert_eq!(header_label_for(Some(&connected), true), "FlameNet");
}

#[test]
fn header_label_keeps_placeholder_free_when_active_ap_absent_from_scan() {
    // Active AP path set but no matching scan entry: no name is known, so the
    // header must not claim "Not connected" (the shell shows "Hidden network").
    assert_eq!(header_label_for(None, true), "");
    assert_eq!(header_label_for(None, false), "Not connected");
}

#[test]
fn profile_parser_ignores_non_wireless_profiles() {
    let connection = MessageItem::from_dict(
        vec![("id".to_owned(), MessageItem::Str("wired".to_owned()))]
            .into_iter()
            .map(Ok::<_, FlameError>),
    )
    .expect("valid profile section");
    let item = MessageItem::new_dict(vec![(
        MessageItem::Str("connection".to_owned()),
        connection,
    )])
    .expect("valid profile settings");
    assert_eq!(
        parse_profile_ssid(Some(&item)).expect("valid profile"),
        None
    );
}

#[test]
fn secret_agent_parser_preserves_requested_setting_without_secret_data() {
    let message = secret_agent_call_with(
        SECRET_AGENT_INTERFACE,
        GET_SECRETS,
        &valid_get_secrets_items(),
    );
    match parse_secret_agent_call(&message).expect("valid SecretAgent call") {
        SecretAgentCall::GetSecrets {
            setting_name,
            secret_key,
            ..
        } => {
            assert_eq!(setting_name, "802-11-wireless-security");
            assert_eq!(secret_key, "psk");
        }
        SecretAgentCall::CancelGetSecrets { .. } => panic!("GetSecrets parsed as cancel"),
        SecretAgentCall::SendReply { .. } => panic!("GetSecrets parsed as reply"),
    }
}

#[test]
fn secret_agent_parser_marks_hintless_get_secrets_as_empty_key() {
    let mut items = valid_get_secrets_items();
    items[3] = MessageItem::from(&[] as &[&str]);
    let message = secret_agent_call_with(SECRET_AGENT_INTERFACE, GET_SECRETS, &items);
    match parse_secret_agent_call(&message).expect("valid SecretAgent call") {
        SecretAgentCall::GetSecrets { secret_key, .. } => assert!(secret_key.is_empty()),
        SecretAgentCall::CancelGetSecrets { .. } => panic!("GetSecrets parsed as cancel"),
        SecretAgentCall::SendReply { .. } => panic!("GetSecrets parsed as reply"),
    }
}

#[test]
fn device_signal_requests_ap_refresh_only_for_changed_ap_or_scan() {
    let cached = WifiDevice {
        active_connection: "/c/3".to_owned(),
        active_access_point: "/ap/7".to_owned(),
        last_scan: Some(10),
    };
    let signal = |ap: Option<&str>, scan: Option<i64>| NetworkManagerProperties {
        active_access_point: ap.map(str::to_owned),
        last_scan: scan,
        ..NetworkManagerProperties::default()
    };
    // New ActiveAccessPoint: device read, which queues the AP list.
    let changed_ap = device_signal_refresh(Some(&cached), &signal(Some("/ap/8"), None));
    assert!(changed_ap.device && changed_ap.access_points);
    // New LastScan only: AP list refresh without a device read.
    let changed_scan = device_signal_refresh(Some(&cached), &signal(None, Some(11)));
    assert!(!changed_scan.device && changed_scan.access_points);
    // ActiveAccessPoint cleared to "/" is a change to "no AP".
    let cleared = device_signal_refresh(Some(&cached), &signal(Some("/"), None));
    assert!(cleared.access_points);
    // Unchanged device: nothing requested.
    let unchanged = device_signal_refresh(Some(&cached), &signal(Some("/ap/7"), Some(10)));
    assert_eq!(unchanged, DeviceRefresh::default());
    // Undiscovered device: device read so the adapter learns it.
    let unknown = device_signal_refresh(None, &signal(None, Some(1)));
    assert!(unknown.device && !unknown.access_points);
}

#[test]
fn failed_scan_queue_does_not_block_next_scan() {
    let mut controller = NetworkController::default();
    controller.service_appeared();
    assert!(matches!(
        controller.request_scan(),
        Ok(Some(
            flamewm_integrations_core::network::NetworkIntent::Scan
        ))
    ));
    let devices = vec!["/org/freedesktop/NetworkManager/Devices/1".to_owned()];
    let result = queue_scan_requests(&mut controller, &devices, |_| {
        Err(FlameError::unavailable("injected queue failure"))
    });
    assert!(result.is_err());
    assert!(matches!(
        controller.request_scan(),
        Ok(Some(
            flamewm_integrations_core::network::NetworkIntent::Scan
        ))
    ));
}

#[test]
fn device_parser_reads_active_access_point_and_treats_slash_as_none() {
    let build = |active_ap: &str| {
        MessageItem::from_dict(
            vec![
                ("DeviceType".to_owned(), MessageItem::UInt32(2)),
                (
                    "ActiveConnection".to_owned(),
                    MessageItem::ObjectPath(
                        object_path(
                            "/org/freedesktop/NetworkManager/ActiveConnection/3".to_owned(),
                        )
                        .expect("valid path"),
                    ),
                ),
                (
                    "ActiveAccessPoint".to_owned(),
                    MessageItem::ObjectPath(object_path(active_ap.to_owned()).expect("valid path")),
                ),
            ]
            .into_iter()
            .map(Ok::<_, FlameError>),
        )
        .expect("valid device properties")
    };
    let item = build("/org/freedesktop/NetworkManager/AccessPoint/7");
    let parsed = parse_device_properties(Some(&item)).expect("valid device");
    assert!(parsed.is_wifi);
    assert_eq!(
        parsed.active_connection,
        "/org/freedesktop/NetworkManager/ActiveConnection/3"
    );
    assert_eq!(
        parsed.active_access_point,
        "/org/freedesktop/NetworkManager/AccessPoint/7"
    );
    let none = build("/");
    assert_eq!(
        parse_device_properties(Some(&none))
            .expect("valid device")
            .active_access_point,
        ""
    );
}

const AP: &str = "/org/freedesktop/NetworkManager/AccessPoint/9";
const DEV: &str = "/org/freedesktop/NetworkManager/Devices/2";
const PROFILE: &str = "/org/freedesktop/NetworkManager/Settings/5";

#[test]
fn successful_new_network_attempt_keeps_its_profile() {
    let mut attempts = ConnectAttempts::default();
    let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    assert_eq!(
        attempts.on_profile_created(id, PROFILE.to_owned()),
        AttemptEffect::None
    );
    assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
    assert_eq!(attempts.on_device_state(DEV, 100, 0), AttemptEffect::None);
    assert!(!attempts.is_busy());
}

#[test]
fn rejected_new_network_deletes_only_its_created_profile_and_reprompts() {
    let mut attempts = ConnectAttempts::default();
    let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    let _ = attempts.on_profile_created(id, PROFILE.to_owned());
    assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
    assert_eq!(
        attempts.on_device_state(DEV, 120, DEVICE_REASON_SUPPLICANT_DISCONNECT),
        AttemptEffect::RetryNewNetwork {
            access_point: AP.to_owned(),
            profile: Some(PROFILE.to_owned()),
        }
    );
    assert!(!attempts.is_busy());
}

#[test]
fn rejection_before_profile_reply_defers_the_delete_until_the_profile_is_known() {
    let mut attempts = ConnectAttempts::default();
    let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
    assert_eq!(
        attempts.on_device_state(DEV, 120, DEVICE_REASON_NO_SECRETS),
        AttemptEffect::None
    );
    assert!(attempts.is_busy());
    assert_eq!(
        attempts.on_profile_created(id, PROFILE.to_owned()),
        AttemptEffect::RetryNewNetwork {
            access_point: AP.to_owned(),
            profile: Some(PROFILE.to_owned()),
        }
    );
    assert!(!attempts.is_busy());
}

#[test]
fn rejected_add_reprompts_without_deleting_anything() {
    let mut attempts = ConnectAttempts::default();
    let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    assert_eq!(
        attempts.on_request_failed(id),
        AttemptEffect::RetryNewNetwork {
            access_point: AP.to_owned(),
            profile: None,
        }
    );
}

#[test]
fn rejected_saved_profile_reprompts_and_is_never_deleted() {
    let mut attempts = ConnectAttempts::default();
    let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
    assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
    // The effect type carries no profile, so a saved profile cannot be deleted from it.
    assert_eq!(
        attempts.on_device_state(DEV, 120, DEVICE_REASON_SUPPLICANT_CONFIG_FAILED),
        AttemptEffect::RetrySavedNetwork {
            access_point: AP.to_owned(),
        }
    );
    assert!(!attempts.is_busy());
}

#[test]
fn saved_profile_activation_error_is_not_a_password_prompt() {
    let mut attempts = ConnectAttempts::default();
    let id = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
    assert_eq!(attempts.on_request_failed(id), AttemptEffect::None);
    assert!(!attempts.is_busy());
}

#[test]
fn non_credential_failure_clears_without_prompt_or_delete() {
    let mut attempts = ConnectAttempts::default();
    let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    let _ = attempts.on_profile_created(id, PROFILE.to_owned());
    let _ = attempts.on_device_state(DEV, 40, 0);
    // NMDeviceStateReason 5 is IP_CONFIG_UNAVAILABLE, not a credential failure.
    assert_eq!(attempts.on_device_state(DEV, 120, 5), AttemptEffect::None);
    assert!(!attempts.is_busy());
}

#[test]
fn previous_connection_disconnect_before_activation_is_ignored() {
    let mut attempts = ConnectAttempts::default();
    let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    assert_eq!(attempts.on_device_state(DEV, 30, 0), AttemptEffect::None);
    assert!(attempts.is_busy());
}

#[test]
fn stale_profile_reply_and_other_device_signals_are_ignored() {
    let mut attempts = ConnectAttempts::default();
    let stale = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    let current = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    assert_ne!(stale, current);
    assert_eq!(
        attempts.on_profile_created(stale, PROFILE.to_owned()),
        AttemptEffect::None
    );
    assert_eq!(
        attempts.on_device_state("/org/freedesktop/NetworkManager/Devices/7", 120, 8),
        AttemptEffect::None
    );
    assert!(attempts.is_busy());
}

#[test]
fn cancelled_attempt_clears_and_later_failure_has_no_effect() {
    let mut attempts = ConnectAttempts::default();
    let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
    attempts.clear();
    assert!(!attempts.is_busy());
    assert_eq!(
        attempts.on_device_state(DEV, 120, DEVICE_REASON_NO_SECRETS),
        AttemptEffect::None
    );
}

#[test]
fn saved_attempt_answers_agent_requests_but_new_attempt_does_not() {
    let mut attempts = ConnectAttempts::default();
    assert_eq!(attempts.saved_access_point(), None);
    let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
    assert_eq!(attempts.saved_access_point(), None);
    attempts.clear();
    let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
    assert_eq!(attempts.saved_access_point(), Some(AP));
}

#[test]
fn device_state_signal_parses_new_state_and_reason() {
    let items = vec![
        MessageItem::UInt32(120),
        MessageItem::UInt32(40),
        MessageItem::UInt32(DEVICE_REASON_SUPPLICANT_TIMEOUT),
    ];
    match parse_device_state_changed(items, DEV.to_owned()) {
        Ok(Incoming::DeviceStateChanged {
            path,
            new_state,
            reason,
        }) => assert_eq!((path.as_str(), new_state, reason), (DEV, 120, 11)),
        _ => panic!("device StateChanged parsed incorrectly"),
    }
    let short = vec![MessageItem::UInt32(120)];
    assert!(parse_device_state_changed(short, DEV.to_owned()).is_err());
}

#[test]
fn submit_plan_creates_profile_only_for_unknown_networks() {
    assert_eq!(
        secret_submit_plan(&SecretPrompt::NewNetwork {
            access_point: AP.to_owned(),
        }),
        SecretSubmitPlan::CreateProfile {
            access_point: AP.to_owned(),
        }
    );
    assert_eq!(
        secret_submit_plan(&SecretPrompt::SavedNetwork {
            access_point: AP.to_owned(),
        }),
        SecretSubmitPlan::UpdateSavedProfile {
            access_point: AP.to_owned(),
        }
    );
}

#[test]
fn known_network_credential_failure_reprompts_and_submit_updates_not_creates() {
    let mut attempts = ConnectAttempts::default();
    let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
    assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
    let AttemptEffect::RetrySavedNetwork { access_point } =
        attempts.on_device_state(DEV, 120, DEVICE_REASON_NO_SECRETS)
    else {
        panic!("known-network credential failure must reprompt");
    };
    assert_eq!(
        secret_submit_plan(&SecretPrompt::SavedNetwork { access_point }),
        SecretSubmitPlan::UpdateSavedProfile {
            access_point: AP.to_owned(),
        }
    );
}

fn security_entries(settings: &MessageItem) -> Vec<(String, MessageItem)> {
    let sections = dict_entries(settings);
    let (_, security) = sections
        .iter()
        .find(|(name, _)| name == "802-11-wireless-security")
        .expect("security section");
    dict_entries(security)
}

fn dict_entries(item: &MessageItem) -> Vec<(String, MessageItem)> {
    let MessageItem::Dict(dict) = item else {
        panic!("expected a dictionary");
    };
    dict.clone()
        .into_vec()
        .into_iter()
        .map(|(key, value)| {
            (
                item_string(&key, "test key").expect("string key"),
                unwrap_variant(value),
            )
        })
        .collect()
}

fn saved_settings_fixture() -> MessageItem {
    let connection = MessageItem::from_dict(
        vec![
            ("id".to_owned(), MessageItem::Str("Home".to_owned())),
            (
                "type".to_owned(),
                MessageItem::Str("802-11-wireless".to_owned()),
            ),
        ]
        .into_iter()
        .map(Ok::<_, FlameError>),
    )
    .expect("valid connection");
    let security = MessageItem::from_dict(
        vec![
            (
                "key-mgmt".to_owned(),
                MessageItem::Str("wpa-psk".to_owned()),
            ),
            ("psk".to_owned(), MessageItem::Str("old-secret".to_owned())),
            ("psk-flags".to_owned(), MessageItem::UInt32(1)),
        ]
        .into_iter()
        .map(Ok::<_, FlameError>),
    )
    .expect("valid security");
    MessageItem::new_dict(vec![
        (MessageItem::Str("connection".to_owned()), connection),
        (
            MessageItem::Str("802-11-wireless-security".to_owned()),
            security,
        ),
    ])
    .expect("valid settings")
}

#[test]
fn saved_profile_update_replaces_only_psk_and_keeps_other_settings() {
    let updated = saved_profile_update_settings(saved_settings_fixture(), "new-secret")
        .expect("update settings");
    let sections = dict_entries(&updated);
    let (_, connection) = sections
        .iter()
        .find(|(name, _)| name == "connection")
        .expect("connection section");
    assert_eq!(
        dict_entries(connection)
            .iter()
            .find(|(key, _)| key == "id")
            .map(|(_, value)| value.clone()),
        Some(MessageItem::Str("Home".to_owned()))
    );
    let security = security_entries(&updated);
    let keys: Vec<&str> = security.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys.len(), 3, "only key-mgmt, psk and psk-flags: {keys:?}");
    let value = |wanted: &str| {
        security
            .iter()
            .find(|(key, _)| key == wanted)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        value("key-mgmt"),
        Some(MessageItem::Str("wpa-psk".to_owned()))
    );
    assert_eq!(
        value("psk"),
        Some(MessageItem::Str("new-secret".to_owned()))
    );
    assert_eq!(value("psk-flags"), Some(MessageItem::UInt32(0)));
    assert!(!format!("{updated:?}").contains("old-secret"));
}

#[test]
fn saved_profile_update_rejects_profile_without_wireless_security() {
    let open = MessageItem::new_dict(vec![(
        MessageItem::Str("connection".to_owned()),
        MessageItem::from_dict(
            vec![("id".to_owned(), MessageItem::Str("Cafe".to_owned()))]
                .into_iter()
                .map(Ok::<_, FlameError>),
        )
        .expect("valid connection"),
    )])
    .expect("valid settings");
    assert!(saved_profile_update_settings(open, "new-secret").is_err());
}

#[test]
fn update_message_targets_saved_profile_with_psk_flags_zero() {
    let settings = saved_profile_update_settings(saved_settings_fixture(), "new-secret")
        .expect("update settings");
    let message = update_connection_message(PROFILE, settings).expect("valid Update call");
    assert_eq!(
        message.path().map(|path| path.to_string()),
        Some(PROFILE.to_owned())
    );
    assert_eq!(
        message.interface().map(|name| name.to_string()),
        Some(SETTINGS_CONNECTION_INTERFACE.to_owned())
    );
    assert_eq!(
        message.member().map(|name| name.to_string()),
        Some("Update".to_owned())
    );
    let items = message.get_items();
    assert_eq!(items.len(), 1);
    let security = security_entries(&items[0]);
    assert_eq!(security.len(), 3);
    assert!(security.contains(&("psk-flags".to_owned(), MessageItem::UInt32(0))));
    assert!(security.contains(&("psk".to_owned(), MessageItem::Str("new-secret".to_owned()))));
    assert!(!format!("{items:?}").contains("old-secret"));
}

#[test]
fn failing_secret_event_does_not_abort_remaining_batch() {
    let events = vec![
        Incoming::SecretAgent(SecretAgentCall::CancelGetSecrets { reply: None }),
        Incoming::SecretAgent(SecretAgentCall::CancelGetSecrets { reply: None }),
    ];
    let mut applied = 0usize;
    dispatch_incoming(events, |_event| {
        applied += 1;
        if applied == 1 {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "NetworkManager secret request has no saved access point",
            ));
        }
        Ok(())
    });
    assert_eq!(applied, 2, "second queued event was never dispatched");
}

fn settings_item() -> MessageItem {
    MessageItem::ObjectPath(
        object_path("/org/freedesktop/NetworkManager/Settings/1".to_owned()).expect("valid path"),
    )
}

fn peer_call(member: &str) -> Message {
    secret_agent_call_with(
        SECRET_AGENT_INTERFACE,
        member,
        &[empty_dict(), settings_item()],
    )
}

fn assert_empty_return(message: &Message) {
    match parse_secret_agent_call(message).expect("call is not fatal") {
        SecretAgentCall::SendReply { reply: Some(reply) } => {
            assert_eq!(reply.msg_type(), MessageType::MethodReturn);
            assert!(reply.get_items().is_empty());
        }
        _ => panic!("call must produce an empty method return"),
    }
}

#[test]
fn secret_agent_save_and_delete_secrets_reply_with_empty_return() {
    for member in ["SaveSecrets", "DeleteSecrets"] {
        assert_empty_return(&peer_call(member));
    }
}

#[test]
fn secret_agent_unknown_member_replies_unknown_method() {
    assert_error_reply(&peer_call("FutureMethod"), UNKNOWN_METHOD);
}

fn secret_agent_call_with(interface: &str, member: &str, items: &[MessageItem]) -> Message {
    let mut message = Message::new_method_call(
        NETWORK_MANAGER_SERVICE,
        SECRET_AGENT_PATH,
        interface,
        member,
    )
    .expect("valid SecretAgent call");
    // libdbus aborts on method_return()/error() when the call serial is 0.
    message.set_serial(1);
    message.append_items(items);
    message
}

fn valid_get_secrets_items() -> Vec<MessageItem> {
    vec![
        empty_dict(),
        settings_item(),
        MessageItem::Str("802-11-wireless-security".to_owned()),
        MessageItem::from(&["psk", "unused"][..]),
        MessageItem::UInt32(0),
    ]
}

fn assert_error_reply(message: &Message, expected: &str) {
    match parse_secret_agent_call(message).expect("bad SecretAgent call is answered, not fatal") {
        SecretAgentCall::SendReply {
            reply: Some(mut reply),
        } => match reply.as_result() {
            Err(error) => assert_eq!(error.name(), Some(expected)),
            Ok(_) => panic!("bad SecretAgent call must produce an error reply"),
        },
        _ => panic!("bad SecretAgent call must produce an error reply"),
    }
}

#[test]
fn secret_agent_wrong_interface_replies_unknown_interface() {
    let message =
        secret_agent_call_with("org.example.Wrong", GET_SECRETS, &valid_get_secrets_items());
    assert_error_reply(&message, "org.freedesktop.DBus.Error.UnknownInterface");
}

#[test]
fn secret_agent_get_secrets_wrong_arity_replies_invalid_args() {
    let mut items = valid_get_secrets_items();
    items.pop();
    let message = secret_agent_call_with(SECRET_AGENT_INTERFACE, GET_SECRETS, &items);
    assert_error_reply(&message, "org.freedesktop.DBus.Error.InvalidArgs");
}

#[test]
fn secret_agent_get_secrets_non_dict_connection_replies_invalid_args() {
    let mut items = valid_get_secrets_items();
    items[0] = MessageItem::Str("not-a-dict".to_owned());
    let message = secret_agent_call_with(SECRET_AGENT_INTERFACE, GET_SECRETS, &items);
    assert_error_reply(&message, "org.freedesktop.DBus.Error.InvalidArgs");
}

#[test]
fn secret_agent_cancel_with_args_replies_invalid_args() {
    let message = secret_agent_call_with(
        SECRET_AGENT_INTERFACE,
        CANCEL_GET_SECRETS,
        &[MessageItem::Str("unexpected".to_owned())],
    );
    assert_error_reply(&message, "org.freedesktop.DBus.Error.InvalidArgs");
}
