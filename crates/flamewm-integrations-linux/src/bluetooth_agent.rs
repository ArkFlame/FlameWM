//! `BlueZ` pairing agent core: `org.bluez.Agent1` handlers and `org.bluez.AgentManager1`
//! registration. No UI and no event source of its own. The caller owns the system-bus pump
//! (`flamewm_dbus_reactor::BusPump`), registers `watch()` with the reactor and calls `on_ready`.
//!
//! Precedent: `.vendor/vela-main/bluetooth/src/agent.c` registers through `RegisterAgent` and then
//! `RequestDefaultAgent`, and unregisters on teardown. Standard: the `BlueZ` agent API
//! (`org.bluez.Agent1`, capabilities `NoInputNoOutput`, `DisplayOnly`, `KeyboardDisplay`).
//! Choice: a decision request is queued as a `PromptKind` and its reply is deferred until a
//! `Decision` arrives from the prompt UI, or until `PROMPT_TIMEOUT` expires it to `Rejected`
//! (D-bt fail-fast: a pairing never hangs). Capability `KeyboardDisplay` is announced because the
//! queue can answer PIN, passkey and confirmation prompts. Registration stays behind
//! `BLUETOOTH_AGENT_CAN_ANSWER`, which is false until a prompt card drives `decide`.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::time::{Duration, Instant};

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
pub const AGENT_CAPABILITY: &str = "KeyboardDisplay";
pub const REJECTED_ERR: &str = "org.bluez.Error.Rejected";
pub const REJECTED_TEXT: &str = "pairing request declined";
pub const CANCELED_ERR: &str = "org.bluez.Error.Canceled";
pub const CANCELED_TEXT: &str = "pairing request cancelled";
/// Longest a prompt may stay unanswered before it is rejected (D-bt fail-fast).
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(30);
/// Prompts held at once; a call beyond this is rejected at once.
pub const MAX_PENDING: usize = 8;
/// `BlueZ` PIN codes are 1 to 16 characters; passkeys are 0 to 999999.
const MAX_PIN_LEN: usize = 16;
const MAX_PASSKEY: u32 = 999_999;
/// Registration gate, mirroring `POLKIT_AGENT_CAN_ANSWER`. Stays false until a prompt card answers
/// through `BluetoothAgent::decide`; the shell does not register the agent while it is false.
pub const BLUETOOTH_AGENT_CAN_ANSWER: bool = false;
const REGISTER: &str = "RegisterAgent";
const REQUEST_DEFAULT: &str = "RequestDefaultAgent";
const UNREGISTER: &str = "UnregisterAgent";
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Agent methods that ask for a decision. Each one is queued until `decide` or the timeout.
pub const PROMPT_METHODS: [&str; 5] = [
    "RequestPinCode",
    "RequestPasskey",
    "RequestConfirmation",
    "RequestAuthorization",
    "AuthorizeService",
];
/// Agent methods that carry no decision (display notices, cancel, release). They get an empty reply.
pub const ACKNOWLEDGED_METHODS: [&str; 4] =
    ["DisplayPinCode", "DisplayPasskey", "Cancel", "Release"];

/// A decision the agent is waiting for, shown to the user by the prompt UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// `RequestConfirmation`: confirm that `passkey` is shown on the device.
    Confirm { device: String, passkey: u32 },
    /// `RequestAuthorization`: allow the device to pair.
    Authorize { device: String },
    /// `AuthorizeService`: allow the device to use the service with this UUID.
    AuthorizeService { device: String, uuid: String },
    /// `RequestPinCode`: the user types the PIN.
    PinCode { device: String },
    /// `RequestPasskey`: the user types the passkey.
    Passkey { device: String },
}

/// The answer chosen by the prompt UI. `Accept` carries the typed value for PIN and passkey
/// prompts, and `None` for confirmations and authorizations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Accept(Option<String>),
    Reject,
    Cancel,
}

/// One queued call with its three prepared replies, built from the call while it is borrowed.
struct Pending {
    kind: PromptKind,
    accept: Message,
    reject: Message,
    cancel: Message,
    since: Instant,
}

/// Decision requests waiting for a UI answer. UI-agnostic: a prompt card lists `prompts`, answers
/// with `decide`, and the owner sends `take_outbox`. Replies are queued in the order decided.
#[derive(Default)]
pub struct PromptQueue {
    next_id: u64,
    pending: BTreeMap<u64, Pending>,
    outbox: Vec<Message>,
}

impl PromptQueue {
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.pending.len() >= MAX_PENDING
    }

    /// Holds one decision request. The reply is sent only after `decide` or `expire`.
    pub fn enqueue(&mut self, call: &Message, kind: PromptKind, now: Instant) -> u64 {
        self.next_id += 1;
        let pending = Pending {
            kind,
            accept: call.method_return(),
            reject: error_reply(call, REJECTED_ERR, REJECTED_TEXT),
            cancel: error_reply(call, CANCELED_ERR, CANCELED_TEXT),
            since: now,
        };
        self.pending.insert(self.next_id, pending);
        self.next_id
    }

    /// Pending prompts, oldest first, for the UI to show.
    pub fn prompts(&self) -> impl Iterator<Item = (u64, &PromptKind)> + '_ {
        self.pending
            .iter()
            .map(|(id, pending)| (*id, &pending.kind))
    }

    /// Answers prompt `id`. Returns `false` for unknown or already answered ids.
    pub fn decide(&mut self, id: u64, decision: Decision) -> bool {
        let Some(pending) = self.pending.remove(&id) else {
            return false;
        };
        self.outbox.push(finish(pending, decision));
        true
    }

    /// Rejects every prompt pending for at least `max_age`. Returns how many expired.
    pub fn expire(&mut self, now: Instant, max_age: Duration) -> usize {
        let stale: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| now.saturating_duration_since(p.since) >= max_age)
            .map(|(id, _)| *id)
            .collect();
        for id in &stale {
            self.decide(*id, Decision::Reject);
        }
        stale.len()
    }

    /// Rejects every pending prompt, for orderly shutdown.
    pub fn reject_all(&mut self) {
        let ids: Vec<u64> = self.pending.keys().copied().collect();
        for id in ids {
            self.decide(id, Decision::Reject);
        }
    }

    /// Takes the replies to send on the bus, in the order they were decided.
    pub fn take_outbox(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.outbox)
    }
}

/// Picks the reply for a decided prompt. Invalid typed values (empty or over-long PIN, passkey that
/// is not a number up to 999999) are refused, as `BlueZ` expects a value or an error.
fn finish(pending: Pending, decision: Decision) -> Message {
    let Pending {
        kind,
        mut accept,
        reject,
        cancel,
        ..
    } = pending;
    match decision {
        Decision::Reject => reject,
        Decision::Cancel => cancel,
        Decision::Accept(value) => match (&kind, value) {
            (PromptKind::PinCode { .. }, Some(pin)) if (1..=MAX_PIN_LEN).contains(&pin.len()) => {
                accept.append_items(&[MessageItem::Str(pin)]);
                accept
            }
            (PromptKind::Passkey { .. }, Some(text)) => match text.parse::<u32>() {
                Ok(passkey) if passkey <= MAX_PASSKEY => {
                    accept.append_items(&[MessageItem::UInt32(passkey)]);
                    accept
                }
                _ => reject,
            },
            (PromptKind::PinCode { .. } | PromptKind::Passkey { .. }, _) => reject,
            _ => accept,
        },
    }
}

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

/// The decision request `BlueZ` sent with `member`, or `None` when the arguments do not match.
fn prompt_kind(message: &Message, member: &str) -> Option<PromptKind> {
    let items = message.get_items();
    let device = |path: &Path| path.to_string();
    match (member, items.as_slice()) {
        ("RequestConfirmation", [MessageItem::ObjectPath(path), MessageItem::UInt32(passkey)]) => {
            Some(PromptKind::Confirm {
                device: device(path),
                passkey: *passkey,
            })
        }
        ("RequestAuthorization", [MessageItem::ObjectPath(path)]) => Some(PromptKind::Authorize {
            device: device(path),
        }),
        ("AuthorizeService", [MessageItem::ObjectPath(path), MessageItem::Str(uuid)]) => {
            Some(PromptKind::AuthorizeService {
                device: device(path),
                uuid: uuid.clone(),
            })
        }
        ("RequestPinCode", [MessageItem::ObjectPath(path)]) => Some(PromptKind::PinCode {
            device: device(path),
        }),
        ("RequestPasskey", [MessageItem::ObjectPath(path)]) => Some(PromptKind::Passkey {
            device: device(path),
        }),
        _ => None,
    }
}

/// Routes one incoming call on `AGENT_PATH`. Decision requests are queued and answered later
/// (`Handled` now), notices are acknowledged, and anything else falls through as `Unhandled` so the
/// pump sends the standard D-Bus error reply.
#[must_use]
pub fn handle_agent_call(
    message: &Message,
    queue: &mut PromptQueue,
    now: Instant,
) -> MessageDisposition {
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
    if let Some(kind) = prompt_kind(message, &member) {
        if queue.is_full() {
            let refused = error_reply(message, REJECTED_ERR, REJECTED_TEXT);
            return MessageDisposition::Reply(refused);
        }
        queue.enqueue(message, kind, now);
        return MessageDisposition::Handled;
    }
    if ACKNOWLEDGED_METHODS.contains(&member.as_str()) {
        return MessageDisposition::Reply(message.method_return());
    }
    MessageDisposition::Unhandled
}

/// Registered `BlueZ` pairing agent on a system-bus pump the caller owns.
pub struct BluetoothAgent {
    pump: BusPump,
    prompts: PromptQueue,
    prompt_timeout: Duration,
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
        Ok(Self {
            pump,
            prompts: PromptQueue::default(),
            prompt_timeout: PROMPT_TIMEOUT,
        })
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

    /// Changes how long an unanswered prompt waits before it is rejected.
    pub fn set_prompt_timeout(&mut self, timeout: Duration) {
        self.prompt_timeout = timeout;
    }

    /// Pending decision requests for the prompt UI.
    pub fn prompts(&self) -> impl Iterator<Item = (u64, &PromptKind)> + '_ {
        self.prompts.prompts()
    }

    /// Answers prompt `id` from the UI and sends its reply. Returns `false` for unknown ids.
    ///
    /// # Errors
    /// Returns the bus error when the reply cannot be sent.
    pub fn decide(&mut self, id: u64, decision: Decision) -> FlameResult<bool> {
        let decided = self.prompts.decide(id, decision);
        self.flush()?;
        Ok(decided)
    }

    /// Dispatches incoming calls on `AGENT_PATH`, rejects prompts past their timeout, and sends the
    /// replies that are ready. Other messages get the default handling.
    ///
    /// # Errors
    /// Returns the bus error from the pump turn.
    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let now = Instant::now();
        let prompts = &mut self.prompts;
        let visited = self.pump.on_ready_with_object_path(
            AGENT_PATH,
            |message| handle_agent_call(message, prompts, now),
            |_| MessageDisposition::Unhandled,
        )?;
        self.prompts.expire(Instant::now(), self.prompt_timeout);
        self.flush()?;
        Ok(visited)
    }

    /// Rejects pending prompts and unregisters the agent. Call only on orderly exit.
    ///
    /// # Errors
    /// Returns the encoding or bus error of the unregister call.
    pub fn shutdown(&mut self) -> FlameResult<()> {
        self.prompts.reject_all();
        self.flush()?;
        self.pump
            .send_with_reply(unregister_message()?, REPLY_TIMEOUT)
            .map(|_| ())
    }

    fn flush(&mut self) -> FlameResult<()> {
        for reply in self.prompts.take_outbox() {
            self.pump.send(reply)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
    const UUID: &str = "0000110b-0000-1000-8000-00805f9b34fb";

    /// A call as if it arrived from the bus. Replies need a nonzero serial, which libdbus asserts.
    fn call_with(member: &str, items: &[MessageItem]) -> Message {
        let mut message =
            Message::new_method_call(BLUEZ_SERVICE, AGENT_PATH, AGENT_IFACE, member).expect("call");
        message.append_items(items);
        message.set_serial(1);
        message
    }

    fn call(member: &str) -> Message {
        call_with(member, &[])
    }

    fn device() -> MessageItem {
        MessageItem::ObjectPath(Path::new(DEVICE).expect("device path"))
    }

    /// The five decision calls, with the arguments `BlueZ` sends.
    fn prompt_calls() -> Vec<Message> {
        vec![
            call_with(
                "RequestConfirmation",
                &[device(), MessageItem::UInt32(123_456)],
            ),
            call_with("RequestAuthorization", &[device()]),
            call_with(
                "AuthorizeService",
                &[device(), MessageItem::Str(UUID.into())],
            ),
            call_with("RequestPinCode", &[device()]),
            call_with("RequestPasskey", &[device()]),
        ]
    }

    fn error_name(reply: &mut Message) -> Option<String> {
        reply
            .as_result()
            .err()
            .and_then(|error| error.name().map(str::to_owned))
    }

    /// Queues one decision call, answers it, and returns the reply.
    fn answer(message: &Message, decision: Decision) -> Message {
        let mut queue = PromptQueue::default();
        let disposition = handle_agent_call(message, &mut queue, Instant::now());
        assert!(matches!(disposition, MessageDisposition::Handled));
        let (id, _) = queue.prompts().next().expect("queued prompt");
        assert!(queue.decide(id, decision));
        queue.take_outbox().pop().expect("reply")
    }

    #[test]
    fn decision_requests_wait_for_an_answer() {
        let mut queue = PromptQueue::default();
        for message in prompt_calls() {
            let disposition = handle_agent_call(&message, &mut queue, Instant::now());
            assert!(matches!(disposition, MessageDisposition::Handled));
        }
        assert_eq!(queue.prompts().count(), 5);
        assert!(queue.take_outbox().is_empty(), "no reply before a decision");
    }

    #[test]
    fn decisions_map_to_bluez_replies() {
        let [confirm, authorize, service, pin, passkey] = prompt_calls().try_into().expect("five");
        assert_eq!(
            error_name(&mut answer(&confirm, Decision::Accept(None))),
            None
        );
        assert_eq!(
            error_name(&mut answer(&authorize, Decision::Reject)).as_deref(),
            Some(REJECTED_ERR)
        );
        assert_eq!(
            error_name(&mut answer(&service, Decision::Cancel)).as_deref(),
            Some(CANCELED_ERR)
        );
        let mut pin_reply = answer(&pin, Decision::Accept(Some("0000".to_owned())));
        assert_eq!(error_name(&mut pin_reply), None);
        assert_eq!(pin_reply.get_items(), [MessageItem::Str("0000".to_owned())]);
        assert_eq!(
            error_name(&mut answer(&pin, Decision::Accept(None))).as_deref(),
            Some(REJECTED_ERR)
        );
        let mut passkey_reply = answer(&passkey, Decision::Accept(Some("123456".to_owned())));
        assert_eq!(passkey_reply.get_items(), [MessageItem::UInt32(123_456)]);
        assert_eq!(error_name(&mut passkey_reply), None);
        for invalid in ["abc", "1000000"] {
            let mut refused = answer(&passkey, Decision::Accept(Some(invalid.to_owned())));
            assert_eq!(
                error_name(&mut refused).as_deref(),
                Some(REJECTED_ERR),
                "passkey {invalid}"
            );
        }
    }

    #[test]
    fn unanswered_prompt_expires_to_rejected() {
        let mut queue = PromptQueue::default();
        let now = Instant::now();
        let _ = handle_agent_call(&prompt_calls()[1], &mut queue, now);
        assert_eq!(
            queue.expire(now, PROMPT_TIMEOUT - Duration::from_secs(1)),
            0
        );
        assert_eq!(queue.expire(now + PROMPT_TIMEOUT, PROMPT_TIMEOUT), 1);
        let mut reply = queue.take_outbox().pop().expect("expired reply");
        assert_eq!(error_name(&mut reply).as_deref(), Some(REJECTED_ERR));
        assert_eq!(queue.prompts().count(), 0);
    }

    #[test]
    fn full_queue_and_shutdown_reject_at_once() {
        let mut queue = PromptQueue::default();
        for _ in 0..MAX_PENDING {
            let _ = handle_agent_call(
                &call_with("RequestPinCode", &[device()]),
                &mut queue,
                Instant::now(),
            );
        }
        let refused = handle_agent_call(
            &call_with("RequestPinCode", &[device()]),
            &mut queue,
            Instant::now(),
        );
        let MessageDisposition::Reply(mut reply) = refused else {
            panic!("full queue must reply");
        };
        assert_eq!(error_name(&mut reply).as_deref(), Some(REJECTED_ERR));
        queue.reject_all();
        assert_eq!(queue.take_outbox().len(), MAX_PENDING);
        assert_eq!(queue.prompts().count(), 0);
    }

    #[test]
    fn notices_and_lifecycle_calls_get_an_empty_reply() {
        for member in ACKNOWLEDGED_METHODS {
            let mut queue = PromptQueue::default();
            let MessageDisposition::Reply(mut reply) =
                handle_agent_call(&call(member), &mut queue, Instant::now())
            else {
                panic!("{member} must be acknowledged");
            };
            assert_eq!(error_name(&mut reply), None, "{member}");
        }
    }

    #[test]
    fn unknown_or_foreign_calls_fall_through() {
        let mut queue = PromptQueue::default();
        let now = Instant::now();
        assert!(matches!(
            handle_agent_call(&call("Bogus"), &mut queue, now),
            MessageDisposition::Unhandled
        ));
        let other_path =
            Message::new_method_call(BLUEZ_SERVICE, DEVICE, AGENT_IFACE, "Release").expect("call");
        assert!(matches!(
            handle_agent_call(&other_path, &mut queue, now),
            MessageDisposition::Unhandled
        ));
        let other_iface =
            Message::new_method_call(BLUEZ_SERVICE, AGENT_PATH, "org.bluez.Other", "Release")
                .expect("call");
        assert!(matches!(
            handle_agent_call(&other_iface, &mut queue, now),
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
