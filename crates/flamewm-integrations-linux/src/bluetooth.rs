//! `BlueZ` device-control facade on the system bus (`org.bluez`).
//!
//! Typed write operations: `Pair` and the `Trusted` property on `org.bluez.Device1`, and
//! `RemoveDevice` on `org.bluez.Adapter1`. Requests go out through the reactor-owned `BusPump`;
//! this module adds no event source and no polling. Message builders are pure and unit-tested
//! without a bus. Reply tracking, agent prompts and the quick-controls row are not implemented
//! (see `docs/backlog/bluetooth.md`).

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
const SET: &str = "Set";
const TRUSTED: &str = "Trusted";
pub const DEFAULT_ADAPTER: &str = "hci0";

/// Validates an adapter name (`hciN`) so it cannot inject path segments.
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
            let adapter_path = format!("/org/bluez/{adapter}");
            let mut message = method_call(&adapter_path, ADAPTER_INTERFACE, "RemoveDevice")?;
            let object = Path::new(device)
                .map_err(|error| FlameError::invalid(format!("bluez object path: {error}")))?;
            message.append_items(&[MessageItem::ObjectPath(object)]);
            Ok(message)
        }
    }
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
    dormant: UnixStream,
    _peer: UnixStream,
}

impl BluetoothDeviceControl {
    pub fn connect() -> FlameResult<Self> {
        let (peer, dormant) = UnixStream::pair()
            .map_err(|io| FlameError::unavailable(format!("socketpair failed: {io}")))?;
        Ok(Self {
            pump: BusPump::connect(BusKind::System).ok(),
            dormant,
            _peer: peer,
        })
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

    /// Consumes every queued reply in one bounded turn so none stays queued on the connection.
    /// Error replies are logged once each; successful replies are dropped as expected.
    pub fn on_ready(&self) -> FlameResult<usize> {
        let Some(pump) = self.pump.as_ref() else {
            return Ok(0);
        };
        pump.on_ready(|message| {
            if let Some(line) = reply_log_line(message) {
                eprintln!("flamewm-integrations-linux: {line}");
            }
            if is_reply(message) {
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

    const MAC: &str = "aa:bb:cc:dd:ee:ff";

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
    fn absent_bus_rejects_requests_without_panicking() {
        let (peer, dormant) = UnixStream::pair().expect("pair");
        let control = BluetoothDeviceControl {
            pump: None,
            dormant,
            _peer: peer,
        };
        assert!(
            control
                .request(DeviceOp::Pair, DEFAULT_ADAPTER, MAC)
                .is_err()
        );
        assert_eq!(control.on_ready().expect("drain"), 0);
    }

    #[test]
    fn replies_are_consumed_and_only_error_replies_are_logged() {
        use dbus::strings::ErrorName;
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
