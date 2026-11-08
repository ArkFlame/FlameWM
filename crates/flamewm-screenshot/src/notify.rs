//! Post-capture notification through the freedesktop Notifications D-Bus API.
//! `notify-send` is the existing desktop client; no resident helper or bus dependency is added.

use std::path::Path;
use std::process::Command;

pub fn captured(saved: Option<&Path>, copied: bool) {
    let body = match (saved, copied) {
        (Some(path), true) => format!("Saved to {} and copied to clipboard", path.display()),
        (Some(path), false) => format!("Saved to {}", path.display()),
        (None, true) => "Copied to clipboard".to_owned(),
        (None, false) => return,
    };
    // Notification service activation may block for seconds. Detach the short-lived
    // client so capture/clipboard latency never depends on a notification daemon.
    if let Err(error) = Command::new("notify-send")
        .args(["--app-name=FlameWM", "--urgency=low", "--expire-time=3000"])
        .arg("Screenshot captured")
        .arg(body)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        eprintln!("FlameWM: notification unavailable: {error}");
    }
}

/// Visible "Stop recording" control: a persistent notification whose Stop action sets `stop`.
/// `gdbus` sends the Notify call with its action; `dbus-monitor` watches `ActionInvoked` for that id.
/// `notify-send` 0.8.8 drops actions before they reach the server, so it cannot carry this
/// control. Without `gdbus` recording still ends at its duration.
pub struct StopIndicator {
    monitor: std::process::Child,
    reader: Option<std::thread::JoinHandle<()>>,
    id: u32,
}

pub fn recording_indicator(
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Option<StopIndicator> {
    let mut monitor = Command::new("dbus-monitor")
        .args([
            "--session",
            "interface='org.freedesktop.Notifications',member='ActionInvoked'",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let reply = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            NOTIFY_NAME,
            "--object-path",
            NOTIFY_PATH,
        ])
        .args(["--method", "org.freedesktop.Notifications.Notify"])
        .args([
            "FlameWM",
            "uint32 0",
            "",
            "Recording screen",
            "Press Stop recording to finish",
        ])
        .args(["['stop', 'Stop recording']", "{}", "int32 0"])
        .output();
    let id = reply
        .ok()
        .and_then(|output| parse_notify_reply(&String::from_utf8_lossy(&output.stdout)));
    let Some(id) = id else {
        let _ = monitor.kill();
        let _ = monitor.wait();
        return None;
    };
    let stdout = monitor.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut parser = ActionParser::default();
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if let Some((invoked, action)) = parser.feed(&line)
                && invoked == id
                && action == "stop"
            {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
        }
    });
    Some(StopIndicator {
        monitor,
        reader: Some(reader),
        id,
    })
}

const NOTIFY_NAME: &str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";

/// Parses the `gdbus call` reply of Notify, for example `(uint32 5,)`.
pub fn parse_notify_reply(text: &str) -> Option<u32> {
    let rest = text.trim().strip_prefix("(uint32 ")?;
    rest.split(',').next()?.trim().parse().ok()
}

/// Reads `dbus-monitor` output one line at a time. An `ActionInvoked` signal prints a header
/// line followed by `uint32 <id>` and `string "<action>"` lines.
#[derive(Default)]
pub struct ActionParser {
    armed: bool,
    id: Option<u32>,
}

impl ActionParser {
    pub fn feed(&mut self, line: &str) -> Option<(u32, String)> {
        if line.starts_with("signal ") {
            self.armed = line.contains("member=ActionInvoked");
            self.id = None;
            return None;
        }
        if !self.armed {
            return None;
        }
        let text = line.trim();
        match self.id {
            None => {
                self.id = text.strip_prefix("uint32 ")?.trim().parse().ok();
                None
            }
            Some(id) => {
                let action = text
                    .strip_prefix("string ")?
                    .trim()
                    .trim_matches('"')
                    .to_owned();
                self.armed = false;
                self.id = None;
                Some((id, action))
            }
        }
    }
}

impl Drop for StopIndicator {
    fn drop(&mut self) {
        let _ = self.monitor.kill();
        let _ = self.monitor.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        // Remove the control once recording ends; best effort, the notification may already be gone.
        let _ = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                NOTIFY_NAME,
                "--object-path",
                NOTIFY_PATH,
            ])
            .args([
                "--method",
                "org.freedesktop.Notifications.CloseNotification",
            ])
            .arg(format!("uint32 {}", self.id))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod stop_control_tests {
    use super::{ActionParser, parse_notify_reply};

    #[test]
    fn notify_reply_yields_the_notification_id() {
        assert_eq!(parse_notify_reply("(uint32 5,)\n"), Some(5));
        assert_eq!(parse_notify_reply("(uint32 123456,)"), Some(123_456));
        assert_eq!(parse_notify_reply("Error: no such method"), None);
        assert_eq!(parse_notify_reply(""), None);
    }

    fn feed_all(parser: &mut ActionParser, text: &str) -> Vec<(u32, String)> {
        text.lines().filter_map(|line| parser.feed(line)).collect()
    }

    #[test]
    fn action_invoked_yields_id_and_key() {
        let signal = "signal time=1 sender=:1.2 -> destination=(null destination) serial=2 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=ActionInvoked\n   uint32 5\n   string \"stop\"\n";
        assert_eq!(
            feed_all(&mut ActionParser::default(), signal),
            vec![(5, "stop".to_owned())]
        );
    }

    #[test]
    fn other_signals_and_bodies_are_ignored() {
        let text = "signal time=1 sender=org.freedesktop.DBus -> destination=:1.1 serial=2 path=/org/freedesktop/DBus; interface=org.freedesktop.DBus; member=NameAcquired\n   string \":1.1\"\n\
signal time=2 sender=:1.2 -> destination=(null destination) serial=3 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=NotificationClosed\n   uint32 5\n   uint32 2\n";
        assert!(feed_all(&mut ActionParser::default(), text).is_empty());
    }

    #[test]
    fn a_stale_value_line_does_not_arm_the_parser() {
        let text = "   uint32 9\n   string \"stop\"\n";
        assert!(feed_all(&mut ActionParser::default(), text).is_empty());
    }
}
