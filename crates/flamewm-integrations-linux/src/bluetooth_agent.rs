//! `BlueZ` pairing agent core: `org.bluez.Agent1` handlers and `org.bluez.AgentManager1`
//! registration. No UI and no event source of its own. The caller owns the system-bus pump
//! (`flamewm_dbus_reactor::BusPump`), registers `watch()` with the reactor and calls `on_ready`.
//!
//! Precedent: `.vendor/vela-main/bluetooth/src/agent.c` registers through `RegisterAgent` and then
//! `RequestDefaultAgent`, and unregisters on teardown. Standard: the `BlueZ` agent API
//! (`org.bluez.Agent1`, capabilities `NoInputNoOutput`, `DisplayOnly`, `KeyboardDisplay`).
//! Choice: capability `NoInputNoOutput` until an in-shell prompt exists. Every request that needs
//! a decision is rejected at once, because an agent that accepts a request and never answers hangs
//! the caller.

use std::ffi::CString;
use std::time::Duration;

use dbus::arg::messageitem::MessageItem;
use dbus::strings::ErrorName;
use dbus::{Message, MessageType, Path};
use flamewm_api::{FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

use crate::bluetooth::BLUEZ_SERVICE;

pub const AGENT_MANAGER_IFACE: &str = "org.bluez.AgentManager1";
pub const AGENT_MANAGER_PATH: &str = "/org/bluez";
pub const AGENT_IFACE: &str = "org.bluez.Agent1";
/// Exported object path for the agent callbacks; `BlueZ` calls back on the unique bus name.
pub const AGENT_PATH: &str = "/org/flamewm/BluetoothAgent";
pub const AGENT_CAPABILITY: &str = "NoInputNoOutput";
pub const REJECTED_ERR: &str = "org.bluez.Error.Rejected";
/// Message text sent with `REJECTED_ERR`, so callers and tests can tell the refusal apart.
pub const REJECTED_TEXT: &str = "pairing request declined: no pairing prompt";
const REGISTER: &str = "RegisterAgent";
const REQUEST_DEFAULT: &str = "RequestDefaultAgent";
const UNREGISTER: &str = "UnregisterAgent";
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Agent methods that ask for a decision. Each one is refused with `REJECTED_ERR`.
pub const REJECTED_METHODS: [&str; 5] = [
    "RequestPinCode",
    "RequestPasskey",
    "RequestConfirmation",
    "RequestAuthorization",
    "AuthorizeService",
];
/// Agent methods that carry no decision (display notices, cancel, release). They get an empty reply.
pub const ACKNOWLEDGED_METHODS: [&str; 4] =
    ["DisplayPinCode", "DisplayPasskey", "Cancel", "Release"];

fn manager_call(member: &str, object: &'static str) -> FlameResult<Message> {
    let mut call = Message::new_method_call(
        BLUEZ_SERVICE,
        AGENT_MANAGER_PATH,
        AGENT_MANAGER_IFACE,
        member,
    )
    .map_err(|error| FlameError::invalid(format!("bluez agent message: {error}")))?;
    call.append_items(&[MessageItem::ObjectPath(Path::from(object))]);
    Ok(call)
}

/// `RegisterAgent(object_path, capability)` for this agent. Pure: no bus required.
pub fn register_message() -> FlameResult<Message> {
    let mut call = manager_call(REGISTER, AGENT_PATH)?;
    call.append_items(&[MessageItem::Str(AGENT_CAPABILITY.to_owned())]);
    Ok(call)
}

/// `RequestDefaultAgent(object_path)`. Pure: no bus required.
pub fn request_default_message() -> FlameResult<Message> {
    manager_call(REQUEST_DEFAULT, AGENT_PATH)
}

/// `UnregisterAgent(object_path)`. Pure: no bus required.
pub fn unregister_message() -> FlameResult<Message> {
    manager_call(UNREGISTER, AGENT_PATH)
}

fn error_reply(call: &Message, name: &str, text: &str) -> Message {
    let text = CString::new(text).unwrap_or_default();
    // The name is a constant that is always a valid D-Bus error name.
    let name = ErrorName::new(name).expect("constant BlueZ error name");
    call.error(&name, &text)
}

/// Routes one incoming call on `AGENT_PATH`. Requests for a decision are refused, notices are
/// acknowledged, and anything else falls through as `Unhandled` so the pump sends the standard
/// D-Bus error reply.
#[must_use]
pub fn handle_agent_call(message: &Message) -> MessageDisposition {
    if message.msg_type() != MessageType::MethodCall {
        return MessageDisposition::Unhandled;
    }
    let (Some(iface), Some(member), Some(path)) =
        (message.interface(), message.member(), message.path())
    else {
        return MessageDisposition::Unhandled;
    };
    let (iface, path) = (iface.to_string(), path.to_string());
    if iface != AGENT_IFACE || path != AGENT_PATH {
        return MessageDisposition::Unhandled;
    }
    let member = member.to_string();
    if REJECTED_METHODS.contains(&member.as_str()) {
        return MessageDisposition::Reply(error_reply(message, REJECTED_ERR, REJECTED_TEXT));
    }
    if ACKNOWLEDGED_METHODS.contains(&member.as_str()) {
        return MessageDisposition::Reply(message.method_return());
    }
    MessageDisposition::Unhandled
}

/// Registered `BlueZ` pairing agent on a system-bus pump the caller owns.
pub struct BluetoothAgent {
    pump: BusPump,
}

impl BluetoothAgent {
    /// Registers over an open bus: `RegisterAgent`, then `RequestDefaultAgent`. When the default
    /// request fails, the registration is withdrawn before the error is returned.
    ///
    /// # Errors
    /// Returns the encoding or bus error of either manager call.
    pub fn register_on(pump: BusPump) -> FlameResult<Self> {
        pump.send_with_reply(register_message()?, REPLY_TIMEOUT)?;
        if let Err(error) = pump.send_with_reply(request_default_message()?, REPLY_TIMEOUT) {
            if let Ok(call) = unregister_message() {
                let _ = pump.send_with_reply(call, REPLY_TIMEOUT);
            }
            return Err(error);
        }
        Ok(Self { pump })
    }

    /// Opens the system bus and registers the agent.
    ///
    /// # Errors
    /// Returns the bus error, or the registration error from `register_on`.
    pub fn connect_system() -> FlameResult<Self> {
        Self::register_on(BusPump::connect(BusKind::System)?)
    }

    /// Unique bus name the agent calls are sent to, once connected.
    #[must_use]
    pub fn unique_name(&self) -> Option<&str> {
        self.pump.unique_name()
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.pump.watch()
    }

    /// Dispatches incoming calls on `AGENT_PATH`. Other messages get the default handling.
    ///
    /// # Errors
    /// Returns the bus error from the pump turn.
    pub fn on_ready(&self) -> FlameResult<usize> {
        self.pump
            .on_ready_with_object_path(AGENT_PATH, handle_agent_call, |_| {
                MessageDisposition::Unhandled
            })
    }

    /// Unregisters the agent. Call only on orderly exit.
    ///
    /// # Errors
    /// Returns the encoding or bus error of the unregister call.
    pub fn shutdown(&self) -> FlameResult<()> {
        self.pump
            .send_with_reply(unregister_message()?, REPLY_TIMEOUT)
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";

    /// A call as if it arrived from the bus. Replies need a nonzero serial, which libdbus asserts.
    fn call(member: &str) -> Message {
        let mut message =
            Message::new_method_call(BLUEZ_SERVICE, AGENT_PATH, AGENT_IFACE, member).expect("call");
        message.set_serial(1);
        message
    }

    /// Error name of a reply, or `None` when the reply is a method return.
    fn error_name_of(disposition: MessageDisposition) -> Option<String> {
        let MessageDisposition::Reply(mut reply) = disposition else {
            panic!("expected a reply");
        };
        reply
            .as_result()
            .err()
            .and_then(|error| error.name().map(str::to_owned))
    }

    #[test]
    fn every_decision_request_is_rejected() {
        for member in REJECTED_METHODS {
            assert_eq!(
                error_name_of(handle_agent_call(&call(member))).as_deref(),
                Some(REJECTED_ERR),
                "{member} must be refused"
            );
        }
    }

    #[test]
    fn notices_and_lifecycle_calls_get_an_empty_reply() {
        for member in ACKNOWLEDGED_METHODS {
            assert_eq!(
                error_name_of(handle_agent_call(&call(member))),
                None,
                "{member} must be acknowledged"
            );
        }
    }

    #[test]
    fn unknown_or_foreign_calls_fall_through() {
        assert!(matches!(
            handle_agent_call(&call("Bogus")),
            MessageDisposition::Unhandled
        ));
        let other_path =
            Message::new_method_call(BLUEZ_SERVICE, DEVICE, AGENT_IFACE, "Release").expect("call");
        assert!(matches!(
            handle_agent_call(&other_path),
            MessageDisposition::Unhandled
        ));
        let other_iface =
            Message::new_method_call(BLUEZ_SERVICE, AGENT_PATH, "org.bluez.Other", "Release")
                .expect("call");
        assert!(matches!(
            handle_agent_call(&other_iface),
            MessageDisposition::Unhandled
        ));
    }

    #[test]
    fn register_message_carries_path_and_capability() {
        let message = register_message().expect("register");
        assert_eq!(message.member().expect("member").to_string(), REGISTER);
        let expected = Path::from(AGENT_PATH);
        assert!(matches!(
            message.get_items().as_slice(),
            [MessageItem::ObjectPath(path), MessageItem::Str(capability)]
                if *path == expected && capability == AGENT_CAPABILITY
        ));
    }

    #[test]
    fn default_and_unregister_name_the_agent_path() {
        for message in [
            request_default_message().expect("default"),
            unregister_message().expect("unregister"),
        ] {
            let expected = Path::from(AGENT_PATH);
            assert!(matches!(
                message.get_items().as_slice(),
                [MessageItem::ObjectPath(path)] if *path == expected
            ));
        }
    }
}

#[cfg(test)]
mod e2e_tests;
