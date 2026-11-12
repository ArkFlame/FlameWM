//! End-to-end `BlueZ` pairing agent on a private `dbus-daemon`. A mock `org.bluez` owns the service
//! name, records `RegisterAgent`, `RequestDefaultAgent` and `UnregisterAgent`, and calls every
//! `org.bluez.Agent1` method on the registered agent. The agent is pumped here and each queued
//! prompt is answered through `BluetoothAgent::decide`. Nothing touches the host bus. When
//! `dbus-daemon` is missing the test reports a skip and returns.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::messageitem::MessageItem;
use dbus::{Message, Path};
use flamewm_dbus_reactor::{BusPump, MessageDisposition};

use super::{
    ACKNOWLEDGED_METHODS, AGENT_IFACE, AGENT_MANAGER_IFACE, AGENT_MANAGER_PATH, AGENT_PATH,
    BluetoothAgent, CANCELED_TEXT, Decision, REJECTED_TEXT,
};
use crate::bluetooth::BLUEZ_SERVICE;
use crate::polkit_agent::e2e_tests::start_daemon;

const DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
const TIMEOUT: Duration = Duration::from_secs(10);
/// Member sent to the agent that no handler owns; the agent must not treat it as a refusal.
const UNKNOWN: &str = "Bogus";

/// Outcome of one probe: the reply's arguments as text, or the error text.
type Outcome = Result<String, String>;

/// What the mock `org.bluez` saw, in arrival order.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Register {
        sender: String,
        path: String,
        capability: String,
    },
    RequestDefault(String),
    Unregister(String),
}

fn answer(message: &Message, seen: &mpsc::Sender<Seen>) -> MessageDisposition {
    let path = message.path().map(|p| p.to_string()).unwrap_or_default();
    let interface = message
        .interface()
        .map(|i| i.to_string())
        .unwrap_or_default();
    if path == DEVICE && interface == "org.freedesktop.DBus.Properties" {
        // Device1 GetAll: Alias is what the card shows.
        let alias = MessageItem::Variant(Box::new(MessageItem::Str("Test Keyboard".to_owned())));
        let props = MessageItem::new_dict(vec![(MessageItem::Str("Alias".to_owned()), alias)])
            .expect("a{sv}");
        let mut reply = message.method_return();
        reply.append_items(&[props]);
        return MessageDisposition::Reply(reply);
    }
    if path != AGENT_MANAGER_PATH || interface != AGENT_MANAGER_IFACE {
        return MessageDisposition::Unhandled;
    }
    let member = message.member().map(|m| m.to_string()).unwrap_or_default();
    let items = message.get_items();
    match (member.as_str(), items.as_slice()) {
        ("RegisterAgent", [MessageItem::ObjectPath(path), MessageItem::Str(capability)]) => {
            let sender = message.sender().map(|s| s.to_string()).unwrap_or_default();
            let _ = seen.send(Seen::Register {
                sender,
                path: path.to_string(),
                capability: capability.clone(),
            });
            MessageDisposition::Reply(message.method_return())
        }
        ("RequestDefaultAgent", [MessageItem::ObjectPath(path)]) => {
            let _ = seen.send(Seen::RequestDefault(path.to_string()));
            MessageDisposition::Reply(message.method_return())
        }
        ("UnregisterAgent", [MessageItem::ObjectPath(path)]) => {
            let _ = seen.send(Seen::Unregister(path.to_string()));
            MessageDisposition::Reply(message.method_return())
        }
        _ => MessageDisposition::Unhandled,
    }
}

/// Arguments `BlueZ` sends with each `Agent1` method.
fn probe_items(member: &str) -> Vec<MessageItem> {
    let device = MessageItem::ObjectPath(Path::new(DEVICE).expect("device path"));
    match member {
        "RequestPinCode" | "RequestPasskey" | "RequestAuthorization" => vec![device],
        "RequestConfirmation" => vec![device, MessageItem::UInt32(123_456)],
        "AuthorizeService" | "DisplayPinCode" => {
            vec![
                device,
                MessageItem::Str("0000110b-0000-1000-8000-00805f9b34fb".to_owned()),
            ]
        }
        "DisplayPasskey" => vec![device, MessageItem::UInt32(123_456), MessageItem::UInt16(0)],
        _ => vec![],
    }
}

/// Mock `org.bluez`: owns the service name, answers manager calls, and performs one probe call on
/// the agent for each `(destination, member)` it receives on `probes`.
fn run_mock(
    address: &str,
    ready: &mpsc::Sender<()>,
    seen: &mpsc::Sender<Seen>,
    probes: &mpsc::Receiver<(String, &'static str)>,
    results: &mpsc::Sender<(&'static str, Outcome)>,
    stop: &mpsc::Receiver<()>,
) -> Result<(), String> {
    let pump = BusPump::connect_address(address).map_err(|e| e.to_string())?;
    pump.own_name(BLUEZ_SERVICE).map_err(|e| e.to_string())?;
    let _ = ready.send(());
    while stop.try_recv().is_err() {
        pump.wait_ready(Duration::from_millis(10))
            .map_err(|e| e.to_string())?;
        pump.on_ready(|message| answer(message, seen))
            .map_err(|e| e.to_string())?;
        while let Ok((dest, member)) = probes.try_recv() {
            let outcome = probe(&pump, &dest, member);
            let _ = results.send((member, outcome));
        }
    }
    Ok(())
}

fn probe(pump: &BusPump, dest: &str, member: &'static str) -> Outcome {
    let mut call = Message::new_method_call(dest, AGENT_PATH, AGENT_IFACE, member)?;
    call.append_items(&probe_items(member));
    pump.send_with_reply(call, TIMEOUT)
        .map(|reply| format!("{:?}", reply.get_items()))
        .map_err(|e| e.to_string())
}

/// Sends one probe and pumps the agent until the mock reports the outcome. A queued prompt is
/// answered with `answer` when one is given; `None` leaves it for the timeout.
fn probe_agent(
    agent: &mut BluetoothAgent,
    probes: &mpsc::Sender<(String, &'static str)>,
    results: &mpsc::Receiver<(&'static str, Outcome)>,
    dest: &str,
    member: &'static str,
    answer: Option<Decision>,
) -> Outcome {
    probes.send((dest.to_owned(), member)).expect("probe mock");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok((reported, outcome)) = results.try_recv() {
            assert_eq!(reported, member, "probe outcome order");
            return outcome;
        }
        assert!(Instant::now() < deadline, "no probe outcome for {member}");
        agent.on_ready().expect("agent pump");
        if let Some(decision) = &answer {
            let ids: Vec<u64> = agent.prompts().map(|(id, _)| id).collect();
            for id in ids {
                agent.decide(id, decision.clone()).expect("decide prompt");
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn next_seen(seen: &mpsc::Receiver<Seen>) -> Seen {
    seen.recv_timeout(TIMEOUT).expect("mock org.bluez event")
}

#[test]
fn agent_registers_and_answers_every_agent1_method_on_a_private_bus() {
    let dir = std::env::temp_dir().join(format!("flamewm-bt-e2e-{}", std::process::id()));
    let Some(daemon) = start_daemon(&dir) else {
        eprintln!("skipped: dbus-daemon unavailable");
        return;
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (seen_tx, seen_rx) = mpsc::channel();
    let (probe_tx, probe_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let address = daemon.address.clone();
    let mock = thread::spawn(move || {
        run_mock(
            &address, &ready_tx, &seen_tx, &probe_rx, &result_tx, &stop_rx,
        )
    });
    ready_rx
        .recv_timeout(TIMEOUT)
        .expect("mock org.bluez ready");

    let pump = BusPump::connect_address(&daemon.address).expect("agent bus");
    let mut agent = BluetoothAgent::register_on(pump).expect("register and default");
    let sender = agent.unique_name().expect("agent unique name").to_owned();
    assert_eq!(
        next_seen(&seen_rx),
        Seen::Register {
            sender: sender.clone(),
            path: AGENT_PATH.to_owned(),
            capability: "KeyboardDisplay".to_owned(),
        }
    );
    assert_eq!(
        next_seen(&seen_rx),
        Seen::RequestDefault(AGENT_PATH.to_owned())
    );

    let ask = |agent: &mut BluetoothAgent, member: &'static str, answer: Option<Decision>| {
        probe_agent(agent, &probe_tx, &result_rx, &sender, member, answer)
    };
    // Each decision request gets the reply its answer maps to.
    let confirm = ask(
        &mut agent,
        "RequestConfirmation",
        Some(Decision::Accept(None)),
    );
    assert!(confirm.is_ok(), "confirm accepted: {confirm:?}");
    let authorize = ask(&mut agent, "RequestAuthorization", Some(Decision::Reject));
    assert!(authorize.unwrap_err().contains(REJECTED_TEXT));
    let service = ask(&mut agent, "AuthorizeService", Some(Decision::Cancel));
    assert!(service.unwrap_err().contains(CANCELED_TEXT));
    let pin = ask(
        &mut agent,
        "RequestPinCode",
        Some(Decision::Accept(Some("0000".to_owned()))),
    );
    assert!(pin.expect("pin reply").contains("\"0000\""));
    let passkey = ask(
        &mut agent,
        "RequestPasskey",
        Some(Decision::Accept(Some("123456".to_owned()))),
    );
    assert!(passkey.expect("passkey reply").contains("UInt32(123456)"));
    // Without an answer the prompt waits, then expires to Rejected instead of hanging.
    agent.set_prompt_timeout(Duration::from_millis(200));
    let timed_out = ask(&mut agent, "RequestPinCode", None);
    assert!(timed_out.unwrap_err().contains(REJECTED_TEXT));
    assert_eq!(agent.prompts().count(), 0);

    for member in ACKNOWLEDGED_METHODS {
        assert!(
            ask(&mut agent, member, None).is_ok(),
            "{member} must get an empty reply"
        );
    }
    // The Device1 Alias arrives as a reply on a later turn; the card label follows it.
    let deadline = Instant::now() + TIMEOUT;
    while agent.device_label(DEVICE) != "Test Keyboard" {
        assert!(Instant::now() < deadline, "Device1 Alias not resolved");
        agent.on_ready().expect("agent pump");
        thread::sleep(Duration::from_millis(5));
    }
    let unknown = ask(&mut agent, UNKNOWN, None);
    let error = unknown.expect_err("unknown members get a D-Bus error");
    assert!(
        !error.contains(REJECTED_TEXT),
        "unknown member refused: {error}"
    );

    agent.shutdown().expect("unregister");
    assert_eq!(next_seen(&seen_rx), Seen::Unregister(AGENT_PATH.to_owned()));

    drop(agent);
    stop_tx.send(()).expect("stop mock");
    mock.join().expect("mock thread").expect("mock org.bluez");
}
