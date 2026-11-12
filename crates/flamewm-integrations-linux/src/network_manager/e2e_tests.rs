//! End-to-end VPN activate and deactivate on a private `dbus-daemon`. A mock `NetworkManager` owns the
//! service name, answers the VPN calls and reports each one it sees; the provider is pumped here.
//! Nothing touches the host bus. When `dbus-daemon` is missing the test reports a skip and returns.

use std::ffi::CString;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::messageitem::MessageItem;
use dbus::strings::ErrorName;
use dbus::{Message, Path};
use flamewm_api::system::{SystemAction, VpnConnectionAction, VpnConnectionOp, VpnConnectionUuid};
use flamewm_dbus_reactor::{BusPump, MessageDisposition};

use super::{NETWORK_MANAGER_SERVICE, NetworkManagerProvider};
use crate::polkit_agent::e2e_tests::start_daemon;

const ROOT_PATH: &str = "/org/freedesktop/NetworkManager";
const ROOT_INTERFACE: &str = "org.freedesktop.NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const ACTIVE_INTERFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const PROFILE_PATH: &str = "/org/freedesktop/NetworkManager/Settings/7";
const ACTIVATED_PATH: &str = "/org/freedesktop/NetworkManager/ActiveConnection/9";
const ACTIVE_OTHER: &str = "/org/freedesktop/NetworkManager/ActiveConnection/3";
const ACTIVE_VPN: &str = "/org/freedesktop/NetworkManager/ActiveConnection/4";
const VPN_UUID: &str = "6f1c9a2e-0b7d-4c3a-9e1f-2a4b5c6d7e8f";
const OTHER_UUID: &str = "11111111-2222-4333-8444-555555555555";
const UNKNOWN_UUID: &str = "00000000-0000-4000-8000-000000000000";
const TIMEOUT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(300);

/// What the mock `NetworkManager` saw, in arrival order.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Lookup(String),
    Activate {
        profile: String,
        device: String,
        specific: String,
    },
    ActiveList,
    ActiveUuid(String),
    Deactivate(String),
}

fn answer(message: &Message, seen: &mpsc::Sender<Seen>) -> MessageDisposition {
    let member = message.member().map(|m| m.to_string()).unwrap_or_default();
    let path = message.path().map(|p| p.to_string()).unwrap_or_default();
    let items = message.get_items();
    match (path.as_str(), member.as_str(), items.as_slice()) {
        (SETTINGS_PATH, "GetConnectionByUuid", [MessageItem::Str(uuid)]) if uuid == VPN_UUID => {
            let _ = seen.send(Seen::Lookup(uuid.clone()));
            reply(message, vec![object(PROFILE_PATH)])
        }
        (SETTINGS_PATH, "GetConnectionByUuid", _) => {
            fail(message, "org.freedesktop.NetworkManager.Settings.NotFound")
        }
        (
            ROOT_PATH,
            "ActivateConnection",
            [
                MessageItem::ObjectPath(profile),
                MessageItem::ObjectPath(device),
                MessageItem::ObjectPath(specific),
            ],
        ) => {
            let _ = seen.send(Seen::Activate {
                profile: profile.to_string(),
                device: device.to_string(),
                specific: specific.to_string(),
            });
            reply(message, vec![object(ACTIVATED_PATH)])
        }
        (ROOT_PATH, "DeactivateConnection", [MessageItem::ObjectPath(active)]) => {
            let _ = seen.send(Seen::Deactivate(active.to_string()));
            reply(message, vec![])
        }
        (ROOT_PATH, "Get", [MessageItem::Str(interface), MessageItem::Str(name)])
            if interface == ROOT_INTERFACE && name == "ActiveConnections" =>
        {
            let _ = seen.send(Seen::ActiveList);
            let list = MessageItem::new_array(vec![object(ACTIVE_OTHER), object(ACTIVE_VPN)])
                .expect("homogeneous object paths");
            reply(message, vec![MessageItem::Variant(Box::new(list))])
        }
        (active, "Get", [MessageItem::Str(interface), MessageItem::Str(name)])
            if interface == ACTIVE_INTERFACE && name == "Uuid" =>
        {
            let _ = seen.send(Seen::ActiveUuid(active.to_owned()));
            let uuid = if active == ACTIVE_VPN {
                VPN_UUID
            } else {
                OTHER_UUID
            };
            reply(
                message,
                vec![MessageItem::Variant(Box::new(MessageItem::Str(
                    uuid.to_owned(),
                )))],
            )
        }
        _ => MessageDisposition::Unhandled,
    }
}

fn object(path: &str) -> MessageItem {
    MessageItem::ObjectPath(Path::new(path.to_owned()).expect("object path"))
}

fn reply(message: &Message, items: Vec<MessageItem>) -> MessageDisposition {
    let mut reply = message.method_return();
    reply.append_items(&items);
    MessageDisposition::Reply(reply)
}

fn fail(message: &Message, name: &str) -> MessageDisposition {
    let name = ErrorName::new(name).expect("error name");
    let text = CString::new("mock refusal").expect("text");
    MessageDisposition::Reply(message.error(&name, &text))
}

/// Mock `NetworkManager`: owns the service name and answers until `stop` is signalled or dropped.
fn run_mock(
    address: &str,
    ready: &mpsc::Sender<()>,
    seen: &mpsc::Sender<Seen>,
    stop: &mpsc::Receiver<()>,
) -> Result<(), String> {
    let pump = BusPump::connect_address(address).map_err(|e| e.to_string())?;
    pump.own_name(NETWORK_MANAGER_SERVICE)
        .map_err(|e| e.to_string())?;
    let _ = ready.send(());
    while stop.try_recv().is_err() {
        pump.wait_ready(Duration::from_millis(10))
            .map_err(|e| e.to_string())?;
        pump.on_ready(|message| answer(message, seen))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn queue(provider: &mut NetworkManagerProvider, op: VpnConnectionOp, uuid: &str) {
    let uuid = VpnConnectionUuid::new(uuid).expect("uuid");
    let action = SystemAction::VpnConnection(VpnConnectionAction { op, uuid });
    let snapshot = provider.snapshot();
    provider
        .perform_action(action, &snapshot)
        .expect("queue VPN action");
}

/// Pumps the provider until the mock reports its next event.
fn next(provider: &mut NetworkManagerProvider, seen: &mpsc::Receiver<Seen>) -> Seen {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(event) = seen.try_recv() {
            return event;
        }
        assert!(Instant::now() < deadline, "no mock NetworkManager event");
        provider.on_ready().expect("provider pump");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Pumps the provider for `span` and returns every event the mock reported meanwhile.
fn drain(
    provider: &mut NetworkManagerProvider,
    seen: &mpsc::Receiver<Seen>,
    span: Duration,
) -> Vec<Seen> {
    let end = Instant::now() + span;
    let mut events = Vec::new();
    while Instant::now() < end {
        provider.on_ready().expect("provider pump");
        events.extend(seen.try_iter());
        thread::sleep(Duration::from_millis(5));
    }
    events
}

#[test]
fn vpn_activate_and_deactivate_run_end_to_end_on_a_private_bus() {
    let dir = std::env::temp_dir().join(format!("flamewm-nm-e2e-{}", std::process::id()));
    let Some(daemon) = start_daemon(&dir) else {
        eprintln!("skipped: dbus-daemon unavailable");
        return;
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (seen_tx, seen_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let address = daemon.address.clone();
    let mock = thread::spawn(move || run_mock(&address, &ready_tx, &seen_tx, &stop_rx));
    ready_rx
        .recv_timeout(TIMEOUT)
        .expect("mock NetworkManager ready");
    let pump = BusPump::connect_address(&daemon.address).expect("provider bus");
    let mut provider = NetworkManagerProvider::with_pump(pump);

    // Activate by UUID: settings lookup, then ActivateConnection on the profile path.
    queue(&mut provider, VpnConnectionOp::Activate, VPN_UUID);
    assert_eq!(
        next(&mut provider, &seen_rx),
        Seen::Lookup(VPN_UUID.to_owned())
    );
    assert_eq!(
        next(&mut provider, &seen_rx),
        Seen::Activate {
            profile: PROFILE_PATH.to_owned(),
            device: "/".to_owned(),
            specific: "/".to_owned(),
        }
    );

    // Deactivate by UUID: list active connections, probe each UUID, deactivate the match.
    queue(&mut provider, VpnConnectionOp::Deactivate, VPN_UUID);
    assert_eq!(next(&mut provider, &seen_rx), Seen::ActiveList);
    assert_eq!(
        next(&mut provider, &seen_rx),
        Seen::ActiveUuid(ACTIVE_OTHER.to_owned())
    );
    assert_eq!(
        next(&mut provider, &seen_rx),
        Seen::ActiveUuid(ACTIVE_VPN.to_owned())
    );
    assert_eq!(
        next(&mut provider, &seen_rx),
        Seen::Deactivate(ACTIVE_VPN.to_owned())
    );

    // Unknown UUID: the lookup fails, so no ActivateConnection is ever sent.
    queue(&mut provider, VpnConnectionOp::Activate, UNKNOWN_UUID);
    assert_eq!(drain(&mut provider, &seen_rx, QUIET), Vec::new());

    // Unknown UUID on deactivate: every active path is probed, none matches, nothing is sent.
    queue(&mut provider, VpnConnectionOp::Deactivate, UNKNOWN_UUID);
    assert_eq!(
        drain(&mut provider, &seen_rx, QUIET),
        vec![
            Seen::ActiveList,
            Seen::ActiveUuid(ACTIVE_OTHER.to_owned()),
            Seen::ActiveUuid(ACTIVE_VPN.to_owned()),
        ]
    );

    drop(provider);
    stop_tx.send(()).expect("stop mock");
    mock.join()
        .expect("mock thread")
        .expect("mock NetworkManager");
    let address = daemon.address.clone();
    drop(daemon);
    assert!(
        BusPump::connect_address(&address).is_err(),
        "private bus must be torn down"
    );
}
