//! Private Xvfb harness shared by the X11 integration tests. The server is started with
//! `-displayfd`, so Xvfb picks a free display, reclaims stale locks itself and reports the
//! number only once it listens. A startup failure panics through the caller; only a missing
//! `Xvfb` binary skips, and it says so on stderr.

use std::ffi::{CString, c_char, c_int, c_void};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut c_void;
    fn XCloseDisplay(display: *mut c_void) -> c_int;
}

/// Owns one Xvfb process and the Xlib connection that proved it ready. The connection stays open
/// until drop: when the last client leaves, the server resets (keymap and extensions are rebuilt),
/// and a client that connects during that reset fails to open the display.
pub struct TestServer {
    child: Child,
    probe: *mut c_void,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // SAFETY: `probe` came from XOpenDisplay and is closed once, before the server dies.
        if !self.probe.is_null() {
            unsafe { XCloseDisplay(self.probe) };
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts a private Xvfb and waits until Xlib (the client library the app uses) can open it.
/// `Ok(None)` means the `Xvfb` binary is absent: the caller returns without asserting.
pub fn start_server() -> Result<Option<(TestServer, String)>, String> {
    let (reader, writer) = std::io::pipe().map_err(|error| error.to_string())?;
    let child = Command::new("Xvfb")
        .args([
            "-displayfd",
            "1",
            "-nolisten",
            "tcp",
            "-screen",
            "0",
            "1024x768x24",
        ])
        .stdout(Stdio::from(writer))
        .stderr(Stdio::null())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("SKIPPED: Xvfb binary not installed; the private-X11 test did not run");
            return Ok(None);
        }
        Err(error) => return Err(format!("spawn Xvfb: {error}")),
    };
    let mut server = TestServer {
        child,
        probe: std::ptr::null_mut(),
    };
    let mut line = String::new();
    BufReader::new(reader)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let number: u32 = line
        .trim()
        .parse()
        .map_err(|_| format!("Xvfb reported no display number (got {line:?})"))?;
    let display = format!(":{number}");
    let name = CString::new(display.as_str()).map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: `name` is a valid C string; a non-null display is kept open until drop.
        let opened = unsafe { XOpenDisplay(name.as_ptr()) };
        if !opened.is_null() {
            server.probe = opened;
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("Xlib cannot open {display} within 5 s"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(Some((server, display)))
}
