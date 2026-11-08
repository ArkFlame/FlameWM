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

/// Visible "Stop recording" control: a persistent notification whose action sets `stop`.
/// Needs `notify-send --wait` (libnotify 0.7.9+). Without it, recording still ends at its duration.
pub struct StopIndicator {
    child: std::process::Child,
    reader: Option<std::thread::JoinHandle<()>>,
}

pub fn recording_indicator(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Option<StopIndicator> {
    let help = Command::new("notify-send").arg("--help").output().ok()?;
    if !String::from_utf8_lossy(&help.stdout).contains("--wait") {
        return None;
    }
    let mut child = Command::new("notify-send")
        .args([
            "--app-name=FlameWM",
            "--urgency=normal",
            "--expire-time=0",
            "--wait",
            "--action=stop=Stop recording",
        ])
        .arg("Recording screen")
        .arg("Press Stop recording to finish")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.trim() == "stop" {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
        }
    });
    Some(StopIndicator {
        child,
        reader: Some(reader),
    })
}

impl Drop for StopIndicator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
