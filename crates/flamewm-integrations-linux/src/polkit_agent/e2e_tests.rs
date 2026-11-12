//! End-to-end polkit agent test on a private `dbus-daemon`. A mock Authority thread registers the
//! agent and sends `BeginAuthentication`; the agent is pumped here; the password conversation runs
//! through `polkit_helper` against a fake helper script. Nothing touches the host bus. When
//! `dbus-daemon` is missing the test reports a skip and returns.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::messageitem::MessageItem;
use flamewm_reactor::ChildPipe;

use super::*;
use crate::polkit_helper::{HelperConversation, HelperStep, drive, spawn_helper};

const COOKIE: &str = "e2e-cookie";
const USER: &str = "alice";
const PASSWORD: &str = "hunter2";
const TIMEOUT: Duration = Duration::from_secs(10);

/// Fake helper: reads the cookie, prompts once, checks the answer and reports.
/// Argv: $1 = user. Stdin: cookie line, then the answer.
const FAKE_HELPER: &str = r#"
read cookie
printf 'PAM_PROMPT_ECHO_OFF Password: \n'
read answer
if [ "$cookie" = "e2e-cookie" ] && [ "$answer" = "hunter2" ] && [ "$1" = "alice" ]; then
  printf 'PAM_TEXT_INFO ok\nSUCCESS\n'
else
  printf 'PAM_ERROR_MSG wrong\nFAILURE\n'
fi
"#;

pub(crate) struct Daemon {
    child: Child,
    pub(crate) address: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts a private session-style `dbus-daemon` and returns its address, or `None` when the
/// binary is unavailable.
pub(crate) fn start_daemon(dir: &std::path::Path) -> Option<Daemon> {
    std::fs::create_dir_all(dir).ok()?;
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .arg(format!("--address=unix:tmpdir={}", dir.display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).ok()?;
    let address = line.trim().to_owned();
    if address.is_empty() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    Some(Daemon { child, address })
}

fn is_register(message: &Message) -> bool {
    message
        .member()
        .is_some_and(|m| matches!(m.to_string().as_str(), REGISTER))
}

/// Mock Authority: owns the Authority name, answers `RegisterAuthenticationAgent`, then sends one
/// `BeginAuthentication` to the registered agent and reports the agent's answer.
fn run_mock_authority(
    address: &str,
    ready: &mpsc::Sender<()>,
    result: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let pump = BusPump::connect_address(address).map_err(|e| e.to_string())?;
    pump.own_name(AUTHORITY_DEST).map_err(|e| e.to_string())?;
    let _ = ready.send(());
    let mut agent: Option<String> = None;
    let deadline = Instant::now() + TIMEOUT;
    while agent.is_none() {
        if Instant::now() >= deadline {
            return Err("agent never registered".into());
        }
        pump.wait_ready(Duration::from_millis(20))
            .map_err(|e| e.to_string())?;
        pump.on_ready(|message| {
            if is_register(message) {
                agent = message.sender().map(|s| s.to_string());
                return MessageDisposition::Reply(message.method_return());
            }
            MessageDisposition::Unhandled
        })
        .map_err(|e| e.to_string())?;
    }
    let agent = agent.ok_or("no sender")?;
    let answer = begin_call(&agent).and_then(|call| {
        pump.send_with_reply(call, TIMEOUT)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    let _ = result.send(answer);
    Ok(())
}

fn begin_call(agent: &str) -> Result<Message, String> {
    let identity = MessageItem::Struct(vec![
        MessageItem::Str("unix-user".to_owned()),
        MessageItem::new_dict(vec![(
            MessageItem::Str("uid".to_owned()),
            MessageItem::Variant(Box::new(MessageItem::UInt32(1000))),
        )])
        .map_err(|e| format!("{e:?}"))?,
    ]);
    let details = MessageItem::new_dict(vec![(
        MessageItem::Str("polkit.message".to_owned()),
        MessageItem::Str("Authentication is required".to_owned()),
    )])
    .map_err(|e| format!("{e:?}"))?;
    let identities = MessageItem::new_array(vec![identity]).map_err(|e| format!("{e:?}"))?;
    let mut call = Message::new_method_call(agent, AGENT_PATH, AGENT_IFACE, BEGIN)?;
    call.append_items(&[
        MessageItem::Str("org.freedesktop.udisks2.filesystem-mount".to_owned()),
        MessageItem::Str("Authenticate to mount".to_owned()),
        MessageItem::Str("drive-removable-media".to_owned()),
        details,
        MessageItem::Str(COOKIE.to_owned()),
        identities,
    ]);
    Ok(call)
}

fn wait_for_prompt(agent: &mut PolkitAgent) -> BeginRequest {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        assert!(
            Instant::now() < deadline,
            "no prompt from the mock Authority"
        );
        agent.on_ready().expect("agent pump");
        for event in agent.take_events() {
            if let PolkitEvent::Show(request) = event {
                return request;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Runs the helper conversation; returns true on `SUCCESS`.
fn converse(helper: &mut ChildPipe) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    let mut conversation = HelperConversation::default();
    loop {
        assert!(Instant::now() < deadline, "helper conversation timed out");
        match drive(helper, &mut conversation).expect("helper read") {
            HelperStep::NeedsPassword => {
                helper.write_line(PASSWORD).expect("answer prompt");
                conversation.answered();
            }
            HelperStep::Done(outcome) => return outcome == AuthOutcome::Success,
            HelperStep::Idle => thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[test]
fn prompt_round_trips_through_private_bus_and_fake_helper() {
    let dir = std::env::temp_dir().join(format!("flamewm-polkit-e2e-{}", std::process::id()));
    let Some(daemon) = start_daemon(&dir) else {
        eprintln!("skipped: dbus-daemon unavailable");
        return;
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let address = daemon.address.clone();
    let mock = thread::spawn(move || run_mock_authority(&address, &ready_tx, &result_tx));
    ready_rx
        .recv_timeout(TIMEOUT)
        .expect("mock Authority ready");

    let pump = BusPump::connect_address(&daemon.address).expect("agent bus");
    let mut agent = PolkitAgent::register_on(pump, "c1", "C").expect("register");
    let request = wait_for_prompt(&mut agent);
    assert_eq!(request.cookie, COOKIE);
    assert_eq!(
        request.action_id,
        "org.freedesktop.udisks2.filesystem-mount"
    );

    let mut command = Command::new("sh");
    command.arg("-c").arg(FAKE_HELPER).arg("sh");
    let mut helper = spawn_helper(command, USER, &request.cookie).expect("helper");
    let ok = converse(&mut helper);
    assert!(ok, "fake helper must report SUCCESS for the right password");
    helper.wait().expect("helper exit");

    let outcome = if ok {
        AuthOutcome::Success
    } else {
        AuthOutcome::Failed
    };
    agent
        .finish(&request.cookie, outcome)
        .expect("finish sends the outcome");
    let answer = result_rx
        .recv_timeout(TIMEOUT)
        .expect("BeginAuthentication reply");
    assert_eq!(
        answer,
        Ok(()),
        "BeginAuthentication must return after the outcome"
    );
    mock.join().expect("mock thread").expect("mock Authority");
}
