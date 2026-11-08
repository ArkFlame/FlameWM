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
