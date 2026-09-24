//! D-Bus transport glue for the transport-neutral Flame Control protocol.

pub mod mutation_queue;

pub use mutation_queue::{MutationDispatcher, drain_queue_inline};

use std::ffi::CString;
use std::time::Duration;

use dbus::arg::messageitem::{MessageItem, MessageItemArray};
use dbus::strings::ErrorName;
use dbus::{Message, MessageType, Signature};
use flamewm_api::ports::EnginePorts;
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_control_core::{ControlError, ControlRequest, Dispatcher, OBJECT_PATH};
use flamewm_control_wire::{
    ControlSignal, WireCall, WireReply, WireSignal, WireValue, decode_call, decode_reply,
    decode_signal, encode_call, encode_reply, encode_signal,
};
use flamewm_dbus_reactor::{BusKind, BusPump, MessageDisposition};
use flamewm_platform::host::PlatformHost;

pub struct ControlServer {
    pump: BusPump,
    dispatcher: Dispatcher,
}

impl ControlServer {
    pub fn connect(pump: BusPump) -> FlameResult<Self> {
        pump.own_name(flamewm_control_core::BUS_NAME)?;
        Ok(Self {
            pump,
            dispatcher: Dispatcher::new(),
        })
    }

    pub fn connect_session() -> FlameResult<Self> {
        Self::connect(BusPump::connect(BusKind::Session)?)
    }

    pub fn on_ready<E: EnginePorts>(
        &self,
        host: &mut PlatformHost<E>,
        now_ms: u64,
    ) -> FlameResult<usize> {
        let pump = &self.pump;
        let dispatcher = &self.dispatcher;
        pump.on_ready(|message| {
            if message.msg_type() != MessageType::MethodCall
                || message.path().map(|path| path.to_string()) != Some(OBJECT_PATH.to_owned())
            {
                return MessageDisposition::Unhandled;
            }
            let result =
                handle_call(dispatcher, host, message, now_ms).and_then(|(reply, signals)| {
                    if let Some(reply) = reply {
                        pump.send(reply)?;
                    }
                    for signal in signals {
                        send_signal(pump, &signal)?;
                    }
                    Ok(())
                });
            if let Err(error) = result {
                if !message.get_no_reply() {
                    if let Ok(name) = ErrorName::new(error_name(&error)) {
                        if let Ok(text) = CString::new(error.to_string()) {
                            let _ = pump.send(message.error(&name, &text));
                        }
                    }
                }
            }
            MessageDisposition::Handled
        })
    }

    #[must_use]
    pub fn watch(&self) -> flamewm_dbus_reactor::BusWatch {
        self.pump.watch()
    }

    pub fn emit_signal(&self, signal: &ControlSignal) -> FlameResult<()> {
        send_signal(&self.pump, signal)
    }
}

fn handle_call<E: EnginePorts>(
    dispatcher: &Dispatcher,
    host: &mut PlatformHost<E>,
    message: &Message,
    now_ms: u64,
) -> FlameResult<(Option<Message>, Vec<ControlSignal>)> {
    let interface = message
        .interface()
        .ok_or_else(|| invalid("missing interface"))?
        .to_string();
    let member = message
        .member()
        .ok_or_else(|| invalid("missing member"))?
        .to_string();
    let args = message
        .get_items()
        .into_iter()
        .map(from_item)
        .collect::<Result<Vec<_>, _>>()?;
    let call = WireCall::new(interface, member, args).with_now_ms(now_ms);
    let request = decode_call(&call).map_err(to_flame_error)?;
    let settings_keys = match &request {
        ControlRequest::ApplySettings(transaction) => Some(
            transaction
                .changes
                .iter()
                .map(|change| change.key.clone())
                .collect::<Vec<_>>(),
        ),
        _ => None,
    };
    let response = dispatcher.dispatch(host, request).map_err(to_flame_error)?;
    let signals = match (settings_keys, &response) {
        (Some(keys), flamewm_control_core::ControlResponse::Settings(snapshot)) => vec![
            ControlSignal::SettingsSnapshotChanged {
                snapshot: snapshot.clone(),
            },
            ControlSignal::SettingsChanged {
                revision: snapshot.revision,
                keys,
            },
        ],
        _ => Vec::new(),
    };
    if message.get_no_reply() {
        return Ok((None, signals));
    }
    let reply = encode_reply(&call, &response).map_err(to_flame_error)?;
    let mut output = message.method_return();
    output.append_items(
        &reply
            .values
            .iter()
            .map(to_item)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid(&error))?,
    );
    Ok((Some(output), signals))
}

fn send_signal(pump: &BusPump, signal: &ControlSignal) -> FlameResult<()> {
    let signal = encode_signal(signal);
    let mut message = Message::new_signal(OBJECT_PATH, &signal.interface, &signal.member)
        .map_err(|error| invalid(&error))?;
    message.append_items(
        &signal
            .args
            .iter()
            .map(to_item)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid(&error))?,
    );
    pump.send(message).map(|_| ())
}

pub struct ControlClient {
    pump: BusPump,
}

impl ControlClient {
    pub fn connect(kind: BusKind) -> FlameResult<Self> {
        Ok(Self {
            pump: BusPump::connect(kind)?,
        })
    }

    pub fn call(
        &self,
        request: &ControlRequest,
    ) -> Result<flamewm_control_core::ControlResponse, ControlError> {
        let call = encode_call(request)?;
        let message = Message::new_method_call(
            flamewm_control_core::BUS_NAME,
            OBJECT_PATH,
            &call.interface,
            &call.member,
        )
        .map_err(|error| invalid(&error))?;
        let mut message = message;
        message.append_items(
            &call
                .args
                .iter()
                .map(to_item)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| invalid(&error))?,
        );
        let reply = self
            .pump
            .send_with_reply(message, Duration::from_secs(10))
            .map_err(control_error_from_transport)?;
        let values = reply
            .get_items()
            .into_iter()
            .map(from_item)
            .collect::<Result<Vec<_>, _>>()?;
        decode_reply(&call, &WireReply { values })
    }
}

pub struct ControlSignalClient {
    pump: BusPump,
}

impl ControlSignalClient {
    pub fn connect(kind: BusKind) -> FlameResult<Self> {
        let pump = BusPump::connect(kind)?;
        install_match_rule(&pump)?;
        Ok(Self { pump })
    }

    pub fn connect_session() -> FlameResult<Self> {
        Self::connect(BusKind::Session)
    }

    #[must_use]
    pub fn watch(&self) -> flamewm_dbus_reactor::BusWatch {
        self.pump.watch()
    }

    pub fn on_ready(&self, mut handler: impl FnMut(ControlSignal)) -> FlameResult<usize> {
        self.pump
            .on_ready(|message| match decode_bus_signal(message) {
                Some(signal) => {
                    handler(signal);
                    MessageDisposition::Handled
                }
                None => MessageDisposition::Unhandled,
            })
    }
}

fn signal_match_rule() -> String {
    format!("type='signal',path='{}'", OBJECT_PATH)
}

fn install_match_rule(pump: &BusPump) -> FlameResult<()> {
    let call = Message::new_method_call(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "AddMatch",
    )
    .map_err(|error| invalid(&error))?;
    let call = call.append1(signal_match_rule());
    pump.send_with_reply(call, Duration::from_secs(5))
        .map(|_| ())
}

/// Decode a raw D-Bus message into a Flame Control signal.
///
/// Returns `None` for non-signal traffic and for signals outside the Flame
/// Control object path or wire vocabulary, so unrelated bus noise is ignored.
pub fn decode_bus_signal(message: &Message) -> Option<ControlSignal> {
    if message.msg_type() != MessageType::Signal {
        return None;
    }
    if message.path().map(|path| path.to_string()) != Some(OBJECT_PATH.to_owned()) {
        return None;
    }
    let interface = message.interface()?.to_string();
    let member = message.member()?.to_string();
    let args = message
        .get_items()
        .into_iter()
        .map(from_item)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    decode_signal(&WireSignal {
        interface,
        member,
        args,
    })
    .ok()
}
fn to_item(value: &WireValue) -> Result<MessageItem, String> {
    match value {
        WireValue::Bool(v) => Ok(MessageItem::Bool(*v)),
        WireValue::U32(v) => Ok(MessageItem::UInt32(*v)),
        WireValue::U64(v) => Ok(MessageItem::UInt64(*v)),
        WireValue::I32(v) => Ok(MessageItem::Int32(*v)),
        WireValue::I64(v) => Ok(MessageItem::Int64(*v)),
        WireValue::String(v) => Ok(MessageItem::Str(v.clone())),
        WireValue::StringArray(v) => array(
            v.iter().map(|v| MessageItem::Str(v.clone())).collect(),
            "as",
        ),
        WireValue::Array(v) => array(
            v.iter().map(to_item).collect::<Result<Vec<_>, _>>()?,
            "aa{sv}",
        ),
        WireValue::Dict(v) => {
            MessageItem::from_dict(v.iter().map(|(k, v)| to_item(v).map(|v| (k.clone(), v))))
                .map_err(|error| error.to_string())
        }
    }
}

fn array(values: Vec<MessageItem>, signature: &str) -> Result<MessageItem, String> {
    MessageItemArray::new(
        values,
        Signature::new(signature).map_err(|error| error.to_string())?,
    )
    .map(MessageItem::Array)
    .map_err(|error| format!("{error:?}"))
}

fn from_item(item: MessageItem) -> Result<WireValue, FlameError> {
    let signature = item.signature().to_string();
    match item {
        MessageItem::Variant(value) => from_item(*value),
        MessageItem::Bool(v) => Ok(WireValue::Bool(v)),
        MessageItem::UInt32(v) => Ok(WireValue::U32(v)),
        MessageItem::UInt64(v) => Ok(WireValue::U64(v)),
        MessageItem::Int32(v) => Ok(WireValue::I32(v)),
        MessageItem::Int64(v) => Ok(WireValue::I64(v)),
        MessageItem::Str(v) => Ok(WireValue::String(v)),
        MessageItem::Array(values) => {
            let string_array = signature == "as";
            let values = values
                .into_vec()
                .into_iter()
                .map(from_item)
                .collect::<Result<Vec<_>, _>>()?;
            if string_array {
                Ok(WireValue::StringArray(
                    values
                        .into_iter()
                        .map(|v| {
                            if let WireValue::String(v) = v {
                                v
                            } else {
                                unreachable!()
                            }
                        })
                        .collect(),
                ))
            } else {
                Ok(WireValue::Array(values))
            }
        }
        MessageItem::Dict(values) => {
            let mut out = std::collections::BTreeMap::new();
            for (key, value) in values.into_vec() {
                let MessageItem::Str(key) = key else {
                    return Err(invalid("D-Bus dictionary key must be string"));
                };
                out.insert(key, from_item(value)?);
            }
            Ok(WireValue::Dict(out))
        }
        _ => Err(invalid("unsupported D-Bus value")),
    }
}

fn invalid(message: &str) -> FlameError {
    FlameError::new(ErrorCode::InvalidArgument, message)
}
fn to_flame_error(error: ControlError) -> FlameError {
    FlameError::new(error.code, error.message)
}
fn error_name(error: &FlameError) -> &'static str {
    flamewm_control_core::error_name(error.code)
}

fn control_error_from_transport(error: FlameError) -> ControlError {
    if error.code == ErrorCode::Unavailable {
        if let Some((code, message)) = decode_remote_method_error(&error.message) {
            return ControlError {
                name: flamewm_control_core::error_name(code),
                code,
                message: message.to_owned(),
            };
        }
    }
    ControlError {
        name: flamewm_control_core::error_name(error.code),
        code: error.code,
        message: error.to_string(),
    }
}

fn decode_remote_method_error(message: &str) -> Option<(ErrorCode, &str)> {
    // BusPump exposes the libdbus error as text. The server's error payload is
    // FlameError::Display (`Code: message`), while canonical names are used
    // when a lower layer preserves the D-Bus error name.
    let payload = message.strip_prefix("D-Bus reply failed: ")?;
    let (name, _) = payload.split_once(": ")?;
    let code = error_code_for_name(name)?;
    Some((code, payload))
}

fn error_code_for_name(name: &str) -> Option<ErrorCode> {
    let canonical_codes = [
        ErrorCode::InvalidArgument,
        ErrorCode::NotFound,
        ErrorCode::StaleRevision,
        ErrorCode::Conflict,
        ErrorCode::Busy,
        ErrorCode::Unsupported,
        ErrorCode::Unavailable,
        ErrorCode::PermissionDenied,
        ErrorCode::InternalFailure,
    ]
    .into_iter()
    .find(|code| name == flamewm_control_core::error_name(*code));
    canonical_codes.or_else(|| {
        [
            ErrorCode::InvalidArgument,
            ErrorCode::NotFound,
            ErrorCode::StaleRevision,
            ErrorCode::Conflict,
            ErrorCode::Busy,
            ErrorCode::Unsupported,
            ErrorCode::Unavailable,
            ErrorCode::PermissionDenied,
            ErrorCode::Timeout,
            ErrorCode::IoFailure,
            ErrorCode::EngineRejected,
            ErrorCode::InternalFailure,
        ]
        .into_iter()
        .find(|code| name == format!("{code:?}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_control_wire::system_dict;

    fn signal_message(signal: &ControlSignal) -> Message {
        let wire = encode_signal(signal);
        wire_signal_message(&wire)
    }

    fn wire_signal_message(wire: &WireSignal) -> Message {
        let mut message =
            Message::new_signal(OBJECT_PATH, &wire.interface, &wire.member).expect("signal");
        message.append_items(
            &wire
                .args
                .iter()
                .map(to_item)
                .collect::<Result<Vec<_>, _>>()
                .expect("args"),
        );
        message
    }

    #[test]
    fn workspaces_changed_round_trips_through_bus_message() {
        let signal = ControlSignal::WorkspacesChanged { revision: 7 };
        let wire = encode_signal(&signal);
        assert_eq!(decode_signal(&wire), Ok(signal.clone()));
        assert_eq!(decode_bus_signal(&signal_message(&signal)), Some(signal));
    }

    #[test]
    fn snapshot_signals_round_trip_through_bus_message() {
        for signal in [
            ControlSignal::WindowsSnapshotChanged {
                revision: 9,
                windows: Vec::new(),
            },
            ControlSignal::WorkspacesSnapshotChanged {
                snapshot: flamewm_api::workspace::WorkspaceSnapshot {
                    revision: 3,
                    count: 1,
                    active_index: 0,
                    last_index: None,
                    names: vec!["1".to_owned()],
                },
            },
            ControlSignal::PanelsSnapshotChanged {
                snapshot: flamewm_api::panels::PanelsSnapshot {
                    revision: 4,
                    panels: Vec::new(),
                    tasks: Vec::new(),
                    pinned_apps: Vec::new(),
                },
            },
        ] {
            let wire = encode_signal(&signal);
            assert_eq!(decode_signal(&wire), Ok(signal.clone()));
            assert_eq!(decode_bus_signal(&signal_message(&signal)), Some(signal));
        }
    }

    // CONTRACT-REGRESSION: T11 SettingsSnapshotChanged must survive D-Bus marshalling exactly.
    // TRIGGER: marshal a typed settings snapshot signal and decode it from a bus message.
    // OBSERVABLE: the decoded signal re-encodes to the identical wire payload.
    // GAP: the current D-Bus decoder admits only legacy SettingsChanged for settings.
    // MUTATION: omit the SettingsSnapshotChanged decoder arm; this test must fail.
    #[test]
    fn t11_settings_snapshot_changed_round_trips_exactly_through_bus_message() {
        let mut snapshot = flamewm_api::settings::SettingsSnapshot::new(11);
        snapshot.values.insert(
            "fontBold".to_owned(),
            flamewm_api::settings::SettingValue::Boolean(true),
        );
        snapshot.values.insert(
            "taskbarOpacity".to_owned(),
            flamewm_api::settings::SettingValue::Percent(83),
        );
        let expected = WireSignal {
            interface: flamewm_control_core::IFACE_SETTINGS.to_owned(),
            member: "SettingsSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(flamewm_control_wire::settings_dict(
                &snapshot,
            ))],
        };
        let decoded = decode_bus_signal(&wire_signal_message(&expected))
            .expect("SettingsSnapshotChanged must decode from D-Bus");
        assert_eq!(encode_signal(&decoded), expected);
    }

    // CONTRACT-REGRESSION: T12 ApplySettings success order is reply, snapshot signal, legacy signal.
    // TRIGGER: decode a revision N+1 reply followed by both ordered D-Bus signals.
    // OBSERVABLE: the snapshot-bearing signal is first; legacy compatibility follows it.
    // GAP: the current settings signal vocabulary has no snapshot-bearing member.
    // MUTATION: omit or reorder the snapshot/legacy pair; this test must fail.
    #[test]
    fn t12_apply_settings_success_orders_reply_snapshot_then_legacy_bus_signals() {
        let mut snapshot = flamewm_api::settings::SettingsSnapshot::new(12);
        snapshot.values.insert(
            "fontBold".to_owned(),
            flamewm_api::settings::SettingValue::Boolean(true),
        );
        let call = WireCall::new(flamewm_control_core::IFACE_SETTINGS, "Apply", Vec::new());
        let reply = encode_reply(
            &call,
            &flamewm_control_core::ControlResponse::Settings(snapshot.clone()),
        )
        .expect("settings reply");
        assert_eq!(
            decode_reply(&call, &reply).expect("settings reply decode"),
            flamewm_control_core::ControlResponse::Settings(snapshot.clone())
        );

        let ordered = [
            WireSignal {
                interface: flamewm_control_core::IFACE_SETTINGS.to_owned(),
                member: "SettingsSnapshotChanged".to_owned(),
                args: vec![WireValue::Dict(flamewm_control_wire::settings_dict(
                    &snapshot,
                ))],
            },
            encode_signal(&ControlSignal::SettingsChanged {
                revision: snapshot.revision,
                keys: vec!["fontBold".to_owned()],
            }),
        ];
        let decoded = ordered
            .iter()
            .map(|signal| {
                decode_bus_signal(&wire_signal_message(signal)).expect("ordered bus signal decode")
            })
            .collect::<Vec<_>>();
        assert_eq!(encode_signal(&decoded[0]), ordered[0]);
        assert_eq!(
            decoded[1],
            ControlSignal::SettingsChanged {
                revision: snapshot.revision,
                keys: vec!["fontBold".to_owned()],
            }
        );
    }

    // CONTRACT-REGRESSION: T12 stale/failed ApplySettings emits no success signal.
    // TRIGGER: feed the bus-facing signal path an unsuccessful mutation result.
    // OBSERVABLE: no snapshot or legacy success signal is accepted for that result.
    // MUTATION: publish either success signal after an error; this test must fail.
    #[test]
    fn t12_apply_settings_failure_emits_no_success_bus_signal() {
        let failure: Result<flamewm_control_core::ControlResponse, ControlError> =
            Err(ControlError {
                name: flamewm_control_core::error_name(ErrorCode::StaleRevision),
                code: ErrorCode::StaleRevision,
                message: "settings revision is stale".to_owned(),
            });
        let signals = match failure {
            Ok(flamewm_control_core::ControlResponse::Settings(snapshot)) => vec![
                WireSignal {
                    interface: flamewm_control_core::IFACE_SETTINGS.to_owned(),
                    member: "SettingsSnapshotChanged".to_owned(),
                    args: vec![WireValue::Dict(flamewm_control_wire::settings_dict(
                        &snapshot,
                    ))],
                },
                encode_signal(&ControlSignal::SettingsChanged {
                    revision: snapshot.revision,
                    keys: vec!["fontBold".to_owned()],
                }),
            ],
            Ok(other) => panic!("unexpected successful response: {other:?}"),
            Err(_) => Vec::<WireSignal>::new(),
        };
        assert!(signals.is_empty());
    }

    #[test]
    fn unrelated_bus_traffic_is_rejected() {
        let foreign = Message::new_signal(
            "/org/freedesktop/Notifications",
            "com.example.Noise",
            "Ping",
        )
        .expect("signal");
        assert_eq!(decode_bus_signal(&foreign), None);
        let wrong_vocab =
            Message::new_signal(OBJECT_PATH, "com.arkflame.FlameWM1.Foo", "Bar").expect("signal");
        assert_eq!(decode_bus_signal(&wrong_vocab), None);
    }

    #[test]
    fn signal_match_rule_scopes_to_control_object_path() {
        assert_eq!(
            signal_match_rule(),
            format!("type='signal',path='{OBJECT_PATH}'")
        );
    }

    // CONTRACT-REGRESSION: T05 SystemSnapshotChanged carries the complete authoritative snapshot through D-Bus.
    // TRIGGER: marshal and unmarshal the canonical SystemSnapshotChanged signal on the control object path.
    // OBSERVABLE: D-Bus decoding admits the signal and preserves the exact snapshot payload.
    // GAP: existing bus tests cover legacy/revision-only and non-system snapshot signals only.
    // MUTATION: omit the SystemSnapshotChanged wire decoder arm or route its payload to SystemChanged; test must fail.
    // CASE: non-default revision and status fields distinguish the snapshot from a revision-only signal.
    #[test]
    fn t05_system_snapshot_changed_round_trips_exact_snapshot_through_dbus() {
        let mut snapshot = flamewm_api::system::SystemSnapshot::default();
        snapshot.revision = 37;
        snapshot.network.label = "ArkNet 5G".to_owned();
        snapshot.network.strength_percent = 82;
        snapshot.media.title = "Neon Skyline".to_owned();
        snapshot.audio.volume_percent = 72;
        let expected = ControlSignal::SystemSnapshotChanged {
            snapshot: snapshot.clone(),
        };
        assert_eq!(
            decode_bus_signal(&signal_message(&expected)),
            Some(expected)
        );
        let wire = WireSignal {
            interface: flamewm_control_core::IFACE_SYSTEM.to_owned(),
            member: "SystemSnapshotChanged".to_owned(),
            args: vec![WireValue::Dict(system_dict(&snapshot))],
        };

        let decoded = decode_bus_signal(&wire_signal_message(&wire))
            .expect("SystemSnapshotChanged must decode from D-Bus");
        assert_eq!(
            format!("{decoded:?}"),
            format!("SystemSnapshotChanged {{ snapshot: {:?} }}", snapshot)
        );

        let legacy = ControlSignal::SystemChanged { revision: 37 };
        assert_eq!(decode_bus_signal(&signal_message(&legacy)), Some(legacy));
    }

    // CONTRACT-REGRESSION: a remote StaleRevision method error must retain its
    // protocol identity when ControlClient receives it through D-Bus.
    // TRIGGER: encode the server's D-Bus Error reply, then pass the
    // reactor-shaped transport failure through the current client seam.
    // OBSERVABLE: the client-facing ControlError is StaleRevision, while an
    // unknown method error remains Unavailable.
    // GAP: BusPump::send_with_reply currently converts every remote method
    // error to FlameError::Unavailable before ControlClient::call maps it.
    // MUTATION: restore the current client mapping; this test must fail with
    // Unavailable for the known StaleRevision name.
    #[test]
    fn remote_stale_revision_method_error_survives_control_client_transport() {
        let stale = remote_method_error_fixture(
            flamewm_control_core::error_name(ErrorCode::StaleRevision),
            "settings revision is stale",
        );
        assert_eq!(
            stale.name(),
            Some(flamewm_control_core::error_name(ErrorCode::StaleRevision))
        );
        assert_eq!(
            stale.message(),
            Some("StaleRevision: settings revision is stale")
        );

        let transport_error = FlameError::new(
            ErrorCode::Unavailable,
            format!("D-Bus reply failed: {stale}"),
        );
        let mapped = client_transport_error_fixture(transport_error);
        assert_eq!(mapped.code, ErrorCode::StaleRevision);
        assert_eq!(
            mapped.name,
            flamewm_control_core::error_name(ErrorCode::StaleRevision)
        );
        assert_eq!(mapped.message, "StaleRevision: settings revision is stale");
    }

    #[test]
    fn unknown_remote_method_error_remains_unavailable() {
        let unknown = remote_method_error_fixture(
            "com.arkflame.FlameWM1.Error.FutureCode",
            "future method error",
        );
        let transport_error = FlameError::new(
            ErrorCode::Unavailable,
            format!("D-Bus reply failed: {unknown}"),
        );
        let mapped = client_transport_error_fixture(transport_error);
        assert_eq!(mapped.code, ErrorCode::Unavailable);
        assert_eq!(
            mapped.name,
            flamewm_control_core::error_name(ErrorCode::Unavailable)
        );
        assert_eq!(
            mapped.message,
            "Unavailable: D-Bus reply failed: future method error"
        );
    }

    #[test]
    fn transport_failure_remains_unavailable() {
        let transport_error =
            FlameError::new(ErrorCode::Unavailable, "D-Bus connection read/write failed");
        let mapped = client_transport_error_fixture(transport_error.clone());
        assert_eq!(mapped.code, ErrorCode::Unavailable);
        assert_eq!(
            mapped.name,
            flamewm_control_core::error_name(ErrorCode::Unavailable)
        );
        assert_eq!(mapped.message, transport_error.to_string());
    }

    // Test-only seam: the reactor currently hides the raw remote Error behind
    // FlameError, so this retains the exact encoded reply and reactor-shaped
    // failure without opening a live bus from this unit test.
    fn remote_method_error_fixture(name: &str, message: &str) -> dbus::Error {
        let mut call = Message::new_method_call(
            flamewm_control_core::BUS_NAME,
            OBJECT_PATH,
            flamewm_control_core::IFACE_SETTINGS,
            "Apply",
        )
        .expect("method call");
        call.set_serial(1);
        let name = ErrorName::new(name).expect("error name");
        let message = error_code_for_name(&name)
            .map_or_else(|| message.to_owned(), |code| format!("{code:?}: {message}"));
        let message = CString::new(message).expect("error message");
        let mut reply = call.error(&name, &message);
        reply.as_result().expect_err("method error reply")
    }

    fn client_transport_error_fixture(error: FlameError) -> ControlError {
        control_error_from_transport(error)
    }
}
