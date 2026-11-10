//! `BlueZ` device-control facade on the system bus (`org.bluez`).
//!
//! Typed write operations: `Pair` and the `Trusted` property on `org.bluez.Device1`, and
//! `RemoveDevice` on `org.bluez.Adapter1`. Requests go out through the reactor-owned `BusPump`;
//! this module adds no event source and no polling. Message builders are pure and unit-tested
//! without a bus. Reply tracking is in `on_ready`; the device list and adapter `Powered` state
//! come from one `GetManagedObjects` call plus `PropertiesChanged` and `InterfacesAdded` /
//! `InterfacesRemoved` signals. Agent prompts and the quick-controls row are not implemented
//! (see `docs/backlog/bluetooth.md`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use dbus::arg::messageitem::MessageItem;
use dbus::strings::Path;
use dbus::{Message, MessageType};
use flamewm_api::ports::FdEvents;
use flamewm_api::{FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

pub use flamewm_api::system::BluetoothDeviceOp as DeviceOp;

pub const BLUEZ_SERVICE: &str = "org.bluez";
pub const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
pub const DEVICE_INTERFACE: &str = "org.bluez.Device1";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const SET: &str = "Set";
const TRUSTED: &str = "Trusted";
pub const DEFAULT_ADAPTER: &str = "hci0";
/// Match rules for the list. Sender filtering is done by the bus, not by this module.
const MATCH_PROPERTIES: &str = "type='signal',sender='org.bluez',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged',path_namespace='/org/bluez'";
const MATCH_OBJECTS: &str =
    "type='signal',sender='org.bluez',interface='org.freedesktop.DBus.ObjectManager'";

/// Validates an adapter name (`hciN`) so it cannot inject path segments.
/// Address of a device object path (`.../dev_AA_BB_CC_DD_EE_FF`), or empty when absent.
fn address_from_path(path: &str) -> String {
    path.rsplit("/dev_")
        .next()
        .filter(|segment| segment.len() == 17 && segment.contains('_'))
        .map(|segment| segment.replace('_', ":"))
        .unwrap_or_default()
}

fn is_adapter_name(adapter: &str) -> bool {
    adapter
        .strip_prefix("hci")
        .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
}

/// Validates a `AA:BB:CC:DD:EE:FF` address and returns its upper-case `BlueZ` path form.
fn address_segment(address: &str) -> Option<String> {
    let bytes = address.as_bytes();
    if bytes.len() != 17 {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = if index % 3 == 2 {
            *byte == b':'
        } else {
            byte.is_ascii_hexdigit()
        };
        if !ok {
            return None;
        }
    }
    Some(address.to_ascii_uppercase().replace(':', "_"))
}

/// Builds `/org/bluez/<adapter>/dev_<AA_BB_CC_DD_EE_FF>`. Malformed input is rejected.
pub fn device_object_path(adapter: &str, address: &str) -> FlameResult<String> {
    if !is_adapter_name(adapter) {
        return Err(FlameError::invalid("bluetooth adapter name"));
    }
    let segment =
        address_segment(address).ok_or_else(|| FlameError::invalid("bluetooth device address"))?;
    Ok(format!("/org/bluez/{adapter}/dev_{segment}"))
}

fn adapter_object(adapter: &str) -> String {
    format!("/org/bluez/{adapter}")
}

fn method_call(path: &str, interface: &str, member: &str) -> FlameResult<Message> {
    Message::new_method_call(BLUEZ_SERVICE, path, interface, member)
        .map_err(|error| FlameError::invalid(format!("bluez message: {error}")))
}

/// Serialises one device operation into a method call. Pure: no bus required.
pub fn device_op_message(op: DeviceOp, adapter: &str, address: &str) -> FlameResult<Message> {
    let device = device_object_path(adapter, address)?;
    match op {
        DeviceOp::Pair => method_call(&device, DEVICE_INTERFACE, "Pair"),
        DeviceOp::SetTrusted(trusted) => {
            let mut message = method_call(&device, PROPERTIES_INTERFACE, SET)?;
            message.append_items(&[
                MessageItem::Str(DEVICE_INTERFACE.to_owned()),
                MessageItem::Str(TRUSTED.to_owned()),
                MessageItem::Variant(Box::new(MessageItem::Bool(trusted))),
            ]);
            Ok(message)
        }
        DeviceOp::Remove => {
            let adapter_path = adapter_object(adapter);
            let mut message = method_call(&adapter_path, ADAPTER_INTERFACE, "RemoveDevice")?;
            let object = Path::new(device)
                .map_err(|error| FlameError::invalid(format!("bluez object path: {error}")))?;
            message.append_items(&[MessageItem::ObjectPath(object)]);
            Ok(message)
        }
    }
}

/// `GetManagedObjects` on the `BlueZ` object manager root. Pure: no bus required.
pub fn managed_objects_message() -> FlameResult<Message> {
    method_call("/", OBJECT_MANAGER_INTERFACE, "GetManagedObjects")
}

/// `org.freedesktop.DBus.AddMatch` with one signal rule. Pure: no bus required.
pub fn add_match_message(rule: &str) -> FlameResult<Message> {
    let mut message = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_SERVICE, "AddMatch")
        .map_err(|error| FlameError::invalid(format!("dbus message: {error}")))?;
    message.append_items(&[MessageItem::Str(rule.to_owned())]);
    Ok(message)
}

pub use flamewm_api::system::{BluetoothDevice as Device, BluetoothSnapshot};

type Dict = [(MessageItem, MessageItem)];

#[derive(Debug, Clone, Default)]
struct DeviceEntry {
    alias: Option<String>,
    name: Option<String>,
    paired: bool,
    trusted: bool,
    connected: bool,
}

impl DeviceEntry {
    fn apply(&mut self, key: &str, value: &MessageItem) {
        match (key, value) {
            ("Alias", MessageItem::Str(text)) => self.alias = Some(text.clone()),
            ("Name", MessageItem::Str(text)) => self.name = Some(text.clone()),
            ("Paired", MessageItem::Bool(on)) => self.paired = *on,
            ("Trusted", MessageItem::Bool(on)) => self.trusted = *on,
            ("Connected", MessageItem::Bool(on)) => self.connected = *on,
            _ => {}
        }
    }
}

/// Facade-side mirror of the adapter and its devices. Pure: fed by parsed messages only.
#[derive(Debug, Clone)]
struct BluetoothState {
    adapter_path: String,
    available: bool,
    powered: bool,
    devices: BTreeMap<String, DeviceEntry>,
    /// Serial of the in-flight `GetManagedObjects`, if any.
    pending: Option<u32>,
}

/// Unwraps a `v` value. Non-variant items are returned unchanged.
fn unwrap_variant(item: &MessageItem) -> &MessageItem {
    match item {
        MessageItem::Variant(inner) => inner,
        other => other,
    }
}

fn dict_slice(item: &MessageItem) -> Option<&Dict> {
    match item {
        MessageItem::Dict(dict) => {
            let slice: &Dict = dict;
            Some(slice)
        }
        _ => None,
    }
}

/// Property name / unwrapped value pairs of an `a{sv}` dict. Non-string keys are skipped.
fn named_values(dict: &Dict) -> Vec<(&str, &MessageItem)> {
    dict.iter()
        .filter_map(|(key, value)| match key {
            MessageItem::Str(name) => Some((name.as_str(), unwrap_variant(value))),
            _ => None,
        })
        .collect()
}

impl BluetoothState {
    fn new(adapter_path: String) -> Self {
        Self {
            adapter_path,
            available: false,
            powered: false,
            devices: BTreeMap::new(),
            pending: None,
        }
    }

    fn is_device(&self, path: &str) -> bool {
        path.strip_prefix(self.adapter_path.as_str())
            .is_some_and(|rest| rest.starts_with("/dev_"))
    }

    fn apply_interface(&mut self, path: &str, interface: &str, changed: &Dict) {
        if interface == ADAPTER_INTERFACE && path == self.adapter_path {
            self.available = true;
            for (key, value) in named_values(changed) {
                if let ("Powered", MessageItem::Bool(on)) = (key, value) {
                    self.powered = *on;
                }
            }
        } else if interface == DEVICE_INTERFACE && self.is_device(path) {
            let entry = self.devices.entry(path.to_owned()).or_default();
            for (key, value) in named_values(changed) {
                entry.apply(key, value);
            }
        }
    }

    fn apply_interfaces(&mut self, path: &str, interfaces: &Dict) {
        for (interface, props) in interfaces {
            if let (MessageItem::Str(interface), Some(props)) = (interface, dict_slice(props)) {
                self.apply_interface(path, interface, props);
            }
        }
    }

    fn remove_interfaces(&mut self, path: &str, removed: &[MessageItem]) {
        for item in removed {
            if let MessageItem::Str(interface) = item {
                if interface == DEVICE_INTERFACE {
                    self.devices.remove(path);
                } else if interface == ADAPTER_INTERFACE && path == self.adapter_path {
                    self.available = false;
                    self.powered = false;
                }
            }
        }
    }

    /// Applies `PropertiesChanged`, `InterfacesAdded` or `InterfacesRemoved`. Returns `false`
    /// for any other signal, which is left for the default handler.
    fn apply_signal(&mut self, message: &Message) -> bool {
        let items = message.get_items();
        let object = message.path();
        let object = object.as_deref();
        let interface = message.interface();
        let member = message.member();
        match (interface.as_deref(), member.as_deref()) {
            (Some(PROPERTIES_INTERFACE), Some("PropertiesChanged")) => {
                if let (
                    Some(path),
                    Some(MessageItem::Str(iface)),
                    Some(MessageItem::Dict(changed)),
                ) = (object, items.first(), items.get(1))
                {
                    self.apply_interface(path, iface, changed);
                    return true;
                }
                false
            }
            (Some(OBJECT_MANAGER_INTERFACE), Some("InterfacesAdded")) => match items.as_slice() {
                [MessageItem::ObjectPath(path), MessageItem::Dict(interfaces)] => {
                    self.apply_interfaces(path, interfaces);
                    true
                }
                _ => false,
            },
            (Some(OBJECT_MANAGER_INTERFACE), Some("InterfacesRemoved")) => match items.as_slice() {
                [MessageItem::ObjectPath(path), MessageItem::Array(removed)] => {
                    let removed: &[MessageItem] = removed;
                    self.remove_interfaces(path, removed);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Builds the state from a `GetManagedObjects` body (`a{oa{sa{sv}}}`). Other objects are
    /// ignored; devices are kept only when they sit under `adapter_path`.
    fn from_managed_objects(adapter_path: &str, body: &[MessageItem]) -> FlameResult<Self> {
        let malformed = || FlameError::invalid("bluez managed objects body");
        let [MessageItem::Dict(objects)] = body else {
            return Err(malformed());
        };
        let mut state = Self::new(adapter_path.to_owned());
        for (path, interfaces) in objects.iter() {
            let (MessageItem::ObjectPath(path), Some(interfaces)) = (path, dict_slice(interfaces))
            else {
                return Err(malformed());
            };
            state.apply_interfaces(path, interfaces);
        }
        Ok(state)
    }

    /// Consumes the reply to the pending `GetManagedObjects` (replacing the state) and applies
    /// signals. Returns `false` for messages this state does not own.
    fn absorb(&mut self, message: &Message) -> FlameResult<bool> {
        if message.msg_type() == MessageType::Signal {
            return Ok(self.apply_signal(message));
        }
        if self.pending.is_none() || message.get_reply_serial() != self.pending {
            return Ok(false);
        }
        self.pending = None;
        if message.msg_type() == MessageType::MethodReturn {
            *self = Self::from_managed_objects(&self.adapter_path, &message.get_items())?;
        }
        Ok(true)
    }

    fn snapshot(&self) -> BluetoothSnapshot {
        BluetoothSnapshot {
            available: self.available,
            powered: self.powered,
            devices: self
                .devices
                .iter()
                .map(|(path, entry)| Device {
                    path: path.clone(),
                    address: address_from_path(path),
                    name: entry
                        .alias
                        .clone()
                        .or_else(|| entry.name.clone())
                        .unwrap_or_default(),
                    paired: entry.paired,
                    trusted: entry.trusted,
                    connected: entry.connected,
                })
                .collect(),
        }
    }
}

/// Parses a `GetManagedObjects` reply for `adapter` (`hciN`) into a typed snapshot. Pure.
pub fn parse_managed_objects(reply: &Message, adapter: &str) -> FlameResult<BluetoothSnapshot> {
    if reply.msg_type() != MessageType::MethodReturn {
        return Err(FlameError::invalid("bluez managed objects reply"));
    }
    let state = BluetoothState::from_managed_objects(&adapter_object(adapter), &reply.get_items())?;
    Ok(state.snapshot())
}

/// Failure line for an error reply, `None` for anything else. Pure, so the consumer is testable.
#[must_use]
pub fn reply_log_line(message: &Message) -> Option<String> {
    if message.msg_type() != MessageType::Error {
        return None;
    }
    let detail = message.get1::<&str>().unwrap_or("no detail");
    Some(format!("bluetooth request failed: {detail}"))
}

fn is_reply(message: &Message) -> bool {
    matches!(
        message.msg_type(),
        MessageType::MethodReturn | MessageType::Error
    )
}

/// Fail-soft device-control facade. An unreachable system bus yields `unavailable` per request.
pub struct BluetoothDeviceControl {
    pump: Option<BusPump>,
    state: RefCell<BluetoothState>,
    dormant: UnixStream,
    _peer: UnixStream,
}

impl BluetoothDeviceControl {
    pub fn connect() -> FlameResult<Self> {
        let (peer, dormant) = UnixStream::pair()
            .map_err(|io| FlameError::unavailable(format!("socketpair failed: {io}")))?;
        let control = Self {
            pump: BusPump::connect(BusKind::System).ok(),
            state: RefCell::new(BluetoothState::new(adapter_object(DEFAULT_ADAPTER))),
            dormant,
            _peer: peer,
        };
        if let Err(error) = control.subscribe() {
            eprintln!("flamewm-integrations-linux: bluetooth subscribe failed: {error}");
        }
        Ok(control)
    }

    /// Adds the signal rules and requests the object list. The replies arrive through `on_ready`.
    fn subscribe(&self) -> FlameResult<()> {
        let Some(pump) = self.pump.as_ref() else {
            return Ok(());
        };
        pump.send(add_match_message(MATCH_PROPERTIES)?)?;
        pump.send(add_match_message(MATCH_OBJECTS)?)?;
        let serial = pump.send(managed_objects_message()?)?;
        self.state.borrow_mut().pending = Some(serial);
        Ok(())
    }

    /// Bus readiness; a dormant socket stands in while the system bus is absent.
    #[must_use]
    pub fn watch(&self) -> BusWatch {
        match &self.pump {
            Some(pump) => pump.watch(),
            None => BusWatch {
                fd: self.dormant.as_raw_fd(),
                events: FdEvents::READABLE,
            },
        }
    }

    /// Latest adapter and device state. Empty and unpowered until the first reply arrives.
    #[must_use]
    pub fn snapshot(&self) -> BluetoothSnapshot {
        self.state.borrow().snapshot()
    }

    /// Consumes every queued reply in one bounded turn so none stays queued on the connection.
    /// Error replies are logged once each; the `GetManagedObjects` reply and signals update the
    /// snapshot; other successful replies are dropped as expected.
    pub fn on_ready(&self) -> FlameResult<usize> {
        let Some(pump) = self.pump.as_ref() else {
            return Ok(0);
        };
        pump.on_ready(|message| {
            if let Some(line) = reply_log_line(message) {
                eprintln!("flamewm-integrations-linux: {line}");
            }
            let consumed = match self.state.borrow_mut().absorb(message) {
                Ok(consumed) => consumed,
                Err(error) => {
                    eprintln!("flamewm-integrations-linux: bluetooth state: {error}");
                    true
                }
            };
            if consumed || is_reply(message) {
                MessageDisposition::Handled
            } else {
                MessageDisposition::Unhandled
            }
        })
    }

    /// Sends one operation and returns its serial. The reply is consumed by `on_ready`.
    pub fn request(&self, op: DeviceOp, adapter: &str, address: &str) -> FlameResult<u32> {
        let pump = self
            .pump
            .as_ref()
            .ok_or_else(|| FlameError::unavailable("system bus unavailable"))?;
        pump.send(device_op_message(op, adapter, address)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbus::MessageType;
    use dbus::arg::messageitem::MessageItemArray;
    use dbus::strings::{ErrorName, Signature};

    const MAC: &str = "aa:bb:cc:dd:ee:ff";
    const ADAPTER: &str = "/org/bluez/hci0";
    const DEV_KEYBOARD: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01";
    const DEV_SPEAKER: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_02";
    const DEV_NEW: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_03";

    fn strs(text: &str) -> MessageItem {
        MessageItem::Str(text.to_owned())
    }

    fn props(pairs: Vec<(&str, MessageItem)>) -> MessageItem {
        let items = pairs
            .into_iter()
            .map(|(key, value)| (strs(key), MessageItem::Variant(Box::new(value))))
            .collect();
        MessageItem::new_dict(items).expect("a{sv}")
    }

    fn object(path: &str, interfaces: Vec<(&str, MessageItem)>) -> (MessageItem, MessageItem) {
        let ifaces = interfaces
            .into_iter()
            .map(|(name, values)| (strs(name), values))
            .collect();
        (
            MessageItem::ObjectPath(Path::new(path.to_owned()).expect("path")),
            MessageItem::new_dict(ifaces).expect("a{sa{sv}}"),
        )
    }

    /// Adapter powered, two devices on hci0, one device on hci1 and one GATT child object.
    fn fixture() -> MessageItem {
        MessageItem::new_dict(vec![
            object(
                ADAPTER,
                vec![(
                    ADAPTER_INTERFACE,
                    props(vec![("Powered", MessageItem::Bool(true))]),
                )],
            ),
            object(
                DEV_KEYBOARD,
                vec![(
                    DEVICE_INTERFACE,
                    props(vec![
                        ("Alias", strs("Keyboard")),
                        ("Name", strs("K")),
                        ("Paired", MessageItem::Bool(true)),
                        ("Trusted", MessageItem::Bool(true)),
                        ("Connected", MessageItem::Bool(false)),
                    ]),
                )],
            ),
            object(
                DEV_SPEAKER,
                vec![(
                    DEVICE_INTERFACE,
                    props(vec![
                        ("Name", strs("Speaker")),
                        ("Paired", MessageItem::Bool(false)),
                        ("Trusted", MessageItem::Bool(false)),
                        ("Connected", MessageItem::Bool(false)),
                    ]),
                )],
            ),
            object(
                "/org/bluez/hci1/dev_AA_BB_CC_DD_EE_09",
                vec![(
                    DEVICE_INTERFACE,
                    props(vec![("Alias", strs("Other adapter"))]),
                )],
            ),
            object(
                "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01/service0001",
                vec![("org.bluez.GattService1", props(vec![("UUID", strs("x"))]))],
            ),
        ])
        .expect("a{oa{sa{sv}}}")
    }

    fn reply_with(body: MessageItem, serial: u32) -> Message {
        let mut call = managed_objects_message().expect("call");
        call.set_serial(serial);
        let mut reply = Message::new_method_return(&call).expect("return");
        reply.append_items(&[body]);
        reply
    }

    fn fixture_state() -> BluetoothState {
        BluetoothState::from_managed_objects(ADAPTER, &[fixture()]).expect("state")
    }

    fn changed_signal(path: &str, interface: &str, changed: MessageItem) -> Message {
        let mut signal =
            Message::new_signal(path, PROPERTIES_INTERFACE, "PropertiesChanged").expect("signal");
        let invalidated = MessageItem::Array(
            MessageItemArray::new(Vec::new(), Signature::new("as").expect("as")).expect("array"),
        );
        signal.append_items(&[strs(interface), changed, invalidated]);
        signal
    }

    #[test]
    fn device_path_normalises_address_and_adapter() {
        assert_eq!(
            device_object_path(DEFAULT_ADAPTER, MAC).expect("path"),
            "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
        );
    }

    #[test]
    fn malformed_adapter_or_address_is_rejected() {
        for adapter in ["", "hci", "eth0", "hci0/../x", "../hci0"] {
            assert!(device_object_path(adapter, MAC).is_err(), "{adapter}");
        }
        for address in [
            "",
            "AA:BB:CC:DD:EE",
            "AA:BB:CC:DD:EE:GG",
            "AA_BB_CC_DD_EE_FF",
            "AA:BB:CC:DD:EE:FF/..",
            "AA:BB:CC:DD:EE:FF:00",
        ] {
            assert!(
                device_object_path(DEFAULT_ADAPTER, address).is_err(),
                "{address}"
            );
        }
    }

    #[test]
    fn pair_is_a_device_method_call_without_arguments() {
        let message = device_op_message(DeviceOp::Pair, DEFAULT_ADAPTER, MAC).expect("pair");
        assert_eq!(
            message.path().as_deref(),
            Some("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF")
        );
        assert_eq!(message.interface().as_deref(), Some(DEVICE_INTERFACE));
        assert_eq!(message.member().as_deref(), Some("Pair"));
        assert_eq!(message.msg_type(), MessageType::MethodCall);
        assert!(message.get_items().is_empty());
    }

    #[test]
    fn trusted_is_set_through_properties_with_bool_variant() {
        let message =
            device_op_message(DeviceOp::SetTrusted(true), DEFAULT_ADAPTER, MAC).expect("trust");
        assert_eq!(message.interface().as_deref(), Some(PROPERTIES_INTERFACE));
        assert_eq!(message.member().as_deref(), Some(SET));
        let items = message.get_items();
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[0], MessageItem::Str(s) if s == DEVICE_INTERFACE));
        assert!(matches!(&items[1], MessageItem::Str(s) if s == TRUSTED));
        assert!(matches!(
            &items[2],
            MessageItem::Variant(inner) if matches!(**inner, MessageItem::Bool(true))
        ));
    }

    #[test]
    fn remove_targets_adapter_with_device_object_path() {
        let message = device_op_message(DeviceOp::Remove, DEFAULT_ADAPTER, MAC).expect("remove");
        assert_eq!(message.path().as_deref(), Some("/org/bluez/hci0"));
        assert_eq!(message.interface().as_deref(), Some(ADAPTER_INTERFACE));
        assert_eq!(message.member().as_deref(), Some("RemoveDevice"));
        assert!(matches!(
            message.get_items().as_slice(),
            [MessageItem::ObjectPath(p)] if &**p == "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"
        ));
    }

    #[test]
    fn list_requests_are_pure_method_calls() {
        let list = managed_objects_message().expect("list");
        assert_eq!(list.path().as_deref(), Some("/"));
        assert_eq!(list.interface().as_deref(), Some(OBJECT_MANAGER_INTERFACE));
        assert_eq!(list.member().as_deref(), Some("GetManagedObjects"));
        assert_eq!(list.msg_type(), MessageType::MethodCall);

        let rule = add_match_message(MATCH_OBJECTS).expect("match");
        assert_eq!(rule.path().as_deref(), Some(DBUS_PATH));
        assert_eq!(rule.interface().as_deref(), Some(DBUS_SERVICE));
        assert_eq!(rule.member().as_deref(), Some("AddMatch"));
        assert!(matches!(
            rule.get_items().as_slice(),
            [MessageItem::Str(s)] if s == MATCH_OBJECTS
        ));
    }

    #[test]
    fn managed_objects_reply_yields_powered_and_scoped_devices() {
        let snapshot =
            parse_managed_objects(&reply_with(fixture(), 1), DEFAULT_ADAPTER).expect("parse");
        assert!(snapshot.powered);
        assert_eq!(
            snapshot.devices,
            vec![
                Device {
                    path: DEV_KEYBOARD.to_owned(),
                    address: "AA:BB:CC:DD:EE:01".to_owned(),
                    name: "Keyboard".to_owned(),
                    paired: true,
                    trusted: true,
                    connected: false,
                },
                Device {
                    path: DEV_SPEAKER.to_owned(),
                    address: "AA:BB:CC:DD:EE:02".to_owned(),
                    name: "Speaker".to_owned(),
                    paired: false,
                    trusted: false,
                    connected: false,
                },
            ]
        );
    }

    #[test]
    fn malformed_managed_objects_body_is_rejected() {
        let reply = reply_with(strs("not objects"), 1);
        assert!(parse_managed_objects(&reply, DEFAULT_ADAPTER).is_err());
        let mut state = BluetoothState::new(ADAPTER.to_owned());
        state.pending = Some(4);
        let reply = reply_with(strs("not objects"), 4);
        assert!(state.absorb(&reply).is_err());
        assert_eq!(state.pending, None);
        assert!(state.devices.is_empty());
    }

    #[test]
    fn properties_changed_updates_device_flags_and_adapter_power() {
        let mut state = fixture_state();
        let connected = changed_signal(
            DEV_KEYBOARD,
            DEVICE_INTERFACE,
            props(vec![("Connected", MessageItem::Bool(true))]),
        );
        assert!(state.apply_signal(&connected));
        let powered_off = changed_signal(
            ADAPTER,
            ADAPTER_INTERFACE,
            props(vec![("Powered", MessageItem::Bool(false))]),
        );
        assert!(state.apply_signal(&powered_off));
        let snapshot = state.snapshot();
        assert!(!snapshot.powered);
        assert!(snapshot.devices[0].connected);
        assert!(!snapshot.devices[1].connected);
    }

    #[test]
    fn interfaces_added_and_removed_track_devices() {
        let mut state = fixture_state();
        let mut added =
            Message::new_signal("/", OBJECT_MANAGER_INTERFACE, "InterfacesAdded").expect("added");
        let (path, ifaces) = object(
            DEV_NEW,
            vec![(DEVICE_INTERFACE, props(vec![("Alias", strs("Mouse"))]))],
        );
        added.append_items(&[path, ifaces]);
        assert!(state.apply_signal(&added));
        assert_eq!(state.snapshot().devices.len(), 3);
        assert_eq!(state.snapshot().devices[2].name, "Mouse");

        let mut removed = Message::new_signal("/", OBJECT_MANAGER_INTERFACE, "InterfacesRemoved")
            .expect("removed");
        removed.append_items(&[
            MessageItem::ObjectPath(Path::new(DEV_NEW.to_owned()).expect("path")),
            MessageItem::new_array(vec![strs(DEVICE_INTERFACE)]).expect("array"),
        ]);
        assert!(state.apply_signal(&removed));
        assert_eq!(state.snapshot().devices.len(), 2);
    }

    #[test]
    fn unrelated_signal_is_not_consumed() {
        let mut state = fixture_state();
        let signal = Message::new_signal("/", "org.example.Other", "Ping").expect("signal");
        assert!(!state.apply_signal(&signal));
    }

    #[test]
    fn only_the_pending_serial_replaces_the_list() {
        let mut state = BluetoothState::new(ADAPTER.to_owned());
        state.pending = Some(2);
        assert!(!state.absorb(&reply_with(fixture(), 1)).expect("stale"));
        assert!(state.devices.is_empty());
        assert!(state.absorb(&reply_with(fixture(), 2)).expect("fresh"));
        assert_eq!(state.pending, None);
        assert_eq!(state.snapshot().devices.len(), 2);
        assert!(state.snapshot().powered);
    }

    #[test]
    fn error_reply_to_pending_list_clears_pending_without_state_change() {
        let mut state = BluetoothState::new(ADAPTER.to_owned());
        let mut call = managed_objects_message().expect("call");
        call.set_serial(3);
        state.pending = Some(3);
        let name = ErrorName::new("org.freedesktop.DBus.Error.ServiceUnknown").expect("name");
        let err = call.error(&name, c"Service unknown");
        assert!(state.absorb(&err).expect("absorb"));
        assert_eq!(state.pending, None);
        assert!(state.devices.is_empty());
    }

    #[test]
    fn absent_bus_rejects_requests_without_panicking() {
        let (peer, dormant) = UnixStream::pair().expect("pair");
        let control = BluetoothDeviceControl {
            pump: None,
            state: RefCell::new(BluetoothState::new(ADAPTER.to_owned())),
            dormant,
            _peer: peer,
        };
        assert!(
            control
                .request(DeviceOp::Pair, DEFAULT_ADAPTER, MAC)
                .is_err()
        );
        assert_eq!(control.on_ready().expect("drain"), 0);
        assert_eq!(control.snapshot(), BluetoothSnapshot::default());
    }

    #[test]
    fn replies_are_consumed_and_only_error_replies_are_logged() {
        let mut call = device_op_message(DeviceOp::Pair, DEFAULT_ADAPTER, MAC).expect("pair");
        call.set_serial(1);
        let ok = Message::new_method_return(&call).expect("return");
        assert!(is_reply(&ok));
        assert_eq!(reply_log_line(&ok), None);
        let name = ErrorName::new("org.bluez.Error.AlreadyExists").expect("name");
        let err = call.error(&name, c"Already Exists");
        assert!(is_reply(&err));
        assert_eq!(
            reply_log_line(&err).as_deref(),
            Some("bluetooth request failed: Already Exists")
        );
        assert!(!is_reply(&call));
    }
}
