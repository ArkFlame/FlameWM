//! Freedesktop notification checks on a private `dbus-daemon`. The provider and a client share one
//! thread: each call pumps both connections until its reply or signal arrives, so no bus call
//! blocks the provider. The daemon is private; the session bus is never touched.

use std::time::{Duration, Instant};

use dbus::arg::messageitem::MessageItem;
use dbus::{Message, MessageType};
use flamewm_dbus_reactor::{BusPump, MessageDisposition};

use super::*;
use crate::polkit_agent::e2e_tests::start_daemon;

const WAIT: Duration = Duration::from_secs(5);

/// Owned copy of a received message: `dbus::Message` is not `Clone`.
struct Captured {
    kind: MessageType,
    member: Option<String>,
    items: Vec<MessageItem>,
}

impl Captured {
    fn signal(&self, member: &str) -> bool {
        self.kind == MessageType::Signal && self.member.as_deref() == Some(member)
    }
}

struct Client {
    pump: BusPump,
    inbox: Vec<Captured>,
}

impl Client {
    fn pump_once(&mut self, provider: &mut NotificationProvider) -> FlameResult<()> {
        provider.on_ready()?;
        let inbox = &mut self.inbox;
        self.pump.on_ready(|message| {
            inbox.push(Captured {
                kind: message.msg_type(),
                member: message.member().map(|name| name.to_string()),
                items: message.get_items(),
            });
            MessageDisposition::Unhandled
        })?;
        Ok(())
    }

    fn take(&mut self, matches: impl Fn(&Captured) -> bool) -> Option<Captured> {
        let index = self.inbox.iter().position(matches)?;
        Some(self.inbox.remove(index))
    }

    /// Sends `call` and returns its reply (method return or error).
    fn call(
        &mut self,
        provider: &mut NotificationProvider,
        call: Message,
    ) -> FlameResult<Captured> {
        self.pump.send(call)?;
        let deadline = Instant::now() + WAIT;
        loop {
            self.pump_once(provider)?;
            let reply =
                |m: &Captured| matches!(m.kind, MessageType::MethodReturn | MessageType::Error);
            if let Some(reply) = self.take(reply) {
                return Ok(reply);
            }
            assert!(Instant::now() < deadline, "D-Bus reply timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Pumps until the next `member` signal arrives and returns its arguments.
    fn signal(&mut self, provider: &mut NotificationProvider, member: &str) -> Vec<MessageItem> {
        let deadline = Instant::now() + WAIT;
        loop {
            self.pump_once(provider).expect("pump");
            if let Some(signal) = self.take(|m| m.signal(member)) {
                return signal.items;
            }
            assert!(Instant::now() < deadline, "{member} signal timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Pumps both sides for `duration` and asserts that no `member` signal arrived.
    fn assert_no_signal(
        &mut self,
        provider: &mut NotificationProvider,
        member: &str,
        duration: Duration,
    ) {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            self.pump_once(provider).expect("pump");
            let found = self.take(|m| m.signal(member));
            assert!(found.is_none(), "unexpected {member} signal");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn method(
    service: &str,
    path: &str,
    interface: &str,
    member: &str,
    items: &[MessageItem],
) -> Message {
    let mut call = Message::new_method_call(service, path, interface, member).expect("method call");
    call.append_items(items);
    call
}

fn notification_method(member: &str, items: &[MessageItem]) -> Message {
    method(
        NOTIFICATIONS_SERVICE,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS_INTERFACE,
        member,
        items,
    )
}

/// `Notify` with the given `expire_timeout`, `replaces_id`, flat action pairs and hint entries.
/// Returns the reply id.
fn notify(
    client: &mut Client,
    provider: &mut NotificationProvider,
    replaces_id: u32,
    timeout: i32,
    hints: Vec<(&str, MessageItem)>,
    actions: &[&str],
    summary: &str,
) -> u32 {
    let hints = MessageItem::from_dict(
        hints
            .into_iter()
            .map(|(key, value)| Ok::<_, ()>((key.to_owned(), value))),
    )
    .expect("hints");
    // `new_array` cannot encode an empty typed array, so "no actions" sends a `default` pair that
    // none of the close-reason assertions depend on.
    let actions = if actions.is_empty() {
        &["default", "Default"][..]
    } else {
        actions
    };
    let actions = MessageItem::new_array(
        actions
            .iter()
            .map(|item| MessageItem::Str((*item).to_owned()))
            .collect(),
    )
    .expect("actions");
    let reply = client
        .call(
            provider,
            notification_method(
                "Notify",
                &[
                    MessageItem::Str("test-app".to_owned()),
                    MessageItem::UInt32(replaces_id),
                    MessageItem::Str(String::new()),
                    MessageItem::Str(summary.to_owned()),
                    MessageItem::Str(String::new()),
                    actions,
                    hints,
                    MessageItem::Int32(timeout),
                ],
            ),
        )
        .expect("notify reply");
    assert_eq!(reply.kind, MessageType::MethodReturn, "Notify must succeed");
    let [MessageItem::UInt32(id)] = reply.items.as_slice() else {
        panic!("Notify must return one uint32 id");
    };
    *id
}

fn urgency(level: u8) -> Vec<(&'static str, MessageItem)> {
    vec![("urgency", MessageItem::Byte(level))]
}

fn closed(id: u32, reason: u32) -> Vec<MessageItem> {
    vec![MessageItem::UInt32(id), MessageItem::UInt32(reason)]
}

fn strings(item: &MessageItem) -> Vec<String> {
    match item {
        MessageItem::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                MessageItem::Str(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn notification_daemon_follows_spec_on_private_bus() {
    let dir = std::env::temp_dir().join(format!("flamewm-notify-e2e-{}", std::process::id()));
    let Some(daemon) = start_daemon(&dir) else {
        eprintln!("skipped: dbus-daemon unavailable");
        return;
    };
    let mut provider = NotificationProvider::connect_address(&daemon.address).expect("provider");
    assert!(
        provider.start().expect("start"),
        "private bus name must be free"
    );
    let mut client = Client {
        pump: BusPump::connect_address(&daemon.address).expect("client"),
        inbox: Vec::new(),
    };
    let add_match = method(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "AddMatch",
        &[MessageItem::Str(
            "type='signal',interface='org.freedesktop.Notifications'".to_owned(),
        )],
    );
    client.call(&mut provider, add_match).expect("AddMatch");

    // (1) Capabilities advertise only what is implemented.
    let caps = client
        .call(&mut provider, notification_method("GetCapabilities", &[]))
        .expect("capabilities");
    assert_eq!(strings(&caps.items[0]), ["actions", "body"]);

    // (3) Expiry: normal expires; critical and timeout 0 never do.
    let normal = notify(
        &mut client,
        &mut provider,
        0,
        50,
        urgency(1),
        &["open", "Open"],
        "expires",
    );
    let critical = notify(
        &mut client,
        &mut provider,
        0,
        50,
        urgency(2),
        &[],
        "critical",
    );
    let forever = notify(&mut client, &mut provider, 0, 0, urgency(1), &[], "forever");
    let far_future = Instant::now() + Duration::from_secs(60);
    assert_eq!(
        provider.expire(far_future).expect("expire"),
        1,
        "only the normal toast expires"
    );
    assert_eq!(
        client.signal(&mut provider, "NotificationClosed"),
        closed(normal, 1)
    );
    assert!(provider.active().iter().any(|view| view.id == critical));
    assert!(provider.active().iter().any(|view| view.id == forever));

    // (4) CloseNotification emits reason 3.
    client
        .call(
            &mut provider,
            notification_method("CloseNotification", &[MessageItem::UInt32(forever)]),
        )
        .expect("close");
    assert_eq!(
        client.signal(&mut provider, "NotificationClosed"),
        closed(forever, 3)
    );

    // (4) ActionInvoked carries the key, then the toast closes with reason 2; an unadvertised key
    // is refused without any signal.
    let acted = notify(
        &mut client,
        &mut provider,
        0,
        0,
        urgency(1),
        &["open", "Open"],
        "act",
    );
    assert!(!provider.invoke_action(acted, "missing").expect("invoke"));
    assert!(provider.invoke_action(acted, "open").expect("invoke"));
    assert_eq!(
        client.signal(&mut provider, "ActionInvoked"),
        vec![
            MessageItem::UInt32(acted),
            MessageItem::Str("open".to_owned())
        ]
    );
    assert_eq!(
        client.signal(&mut provider, "NotificationClosed"),
        closed(acted, 2)
    );

    // (4) A resident notification stays on screen after its action is invoked.
    let resident_hints = vec![
        ("urgency", MessageItem::Byte(1)),
        ("resident", MessageItem::Bool(true)),
    ];
    let resident = notify(
        &mut client,
        &mut provider,
        0,
        0,
        resident_hints,
        &["open", "Open"],
        "resident",
    );
    assert!(provider.invoke_action(resident, "open").expect("invoke"));
    assert_eq!(
        client.signal(&mut provider, "ActionInvoked")[0],
        MessageItem::UInt32(resident)
    );
    client.assert_no_signal(
        &mut provider,
        "NotificationClosed",
        Duration::from_millis(50),
    );
    assert!(provider.active().iter().any(|view| view.id == resident));
    provider.dismiss(resident).expect("dismiss");
    assert_eq!(
        client.signal(&mut provider, "NotificationClosed"),
        closed(resident, 2)
    );

    // (2) replaces_id keeps the id and restarts the timeout from the replacement.
    let original = notify(&mut client, &mut provider, 0, 1000, urgency(1), &[], "one");
    std::thread::sleep(Duration::from_millis(600));
    let replaced = notify(
        &mut client,
        &mut provider,
        original,
        1000,
        urgency(1),
        &[],
        "two",
    );
    assert_eq!(replaced, original, "replacement keeps the id");
    let mid = Instant::now() + Duration::from_millis(800);
    assert_eq!(
        provider.expire(mid).expect("expire"),
        0,
        "replacement must reset the timeout"
    );
    assert_eq!(
        provider
            .expire(Instant::now() + Duration::from_secs(2))
            .expect("expire"),
        1,
        "replacement expires after its own timeout"
    );
    assert_eq!(
        client.signal(&mut provider, "NotificationClosed"),
        closed(original, 1)
    );

    // (6) Do Not Disturb: normal is suppressed, critical still shows.
    provider.set_dnd(true);
    let quiet = notify(&mut client, &mut provider, 0, 0, urgency(1), &[], "quiet");
    let loud = notify(&mut client, &mut provider, 0, 0, urgency(2), &[], "loud");
    assert!(quiet != 0 && loud != 0);
    let active: Vec<u32> = provider.active().iter().map(|view| view.id).collect();
    assert!(!active.contains(&quiet), "DND suppresses normal toasts");
    assert!(
        active.contains(&loud),
        "critical toasts still show under DND"
    );
    provider.set_dnd(false);
}
