//! Two Clipboard instances on a private Xvfb: files set by one are read by the other.
//! Skips (with a message) when no `Xvfb` binary is installed.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use flamewm_clipboard::{Clipboard, FileOp};

/// Kills only the Xvfb process this test started.
struct TestServer(Child);

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server() -> Option<(TestServer, String)> {
    for n in 96..120 {
        let display = format!(":{n}");
        if Path::new(&format!("/tmp/.X{n}-lock")).exists() {
            continue;
        }
        let child = Command::new("Xvfb")
            .args([&display, "-screen", "0", "640x480x24", "-nolisten", "tcp"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let server = TestServer(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if Clipboard::connect_display(Some(&display)).is_ok() {
                return Some((server, display));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        return None;
    }
    None
}

#[test]
fn set_files_on_one_instance_is_read_by_another() {
    let Some((_server, display)) = start_server() else {
        eprintln!("skipped: Xvfb not available or no free display");
        return;
    };
    let owner = Clipboard::connect_display(Some(&display)).expect("owner connects");
    let reader = Clipboard::connect_display(Some(&display)).expect("reader connects");

    let files = vec![
        PathBuf::from("/tmp/a b#%ü.txt"),
        PathBuf::from("/tmp/dir/plain"),
    ];
    owner.set_files(&files, FileOp::Cut).expect("set cut");
    assert_eq!(
        reader.get_files().expect("get cut"),
        Some((files.clone(), FileOp::Cut))
    );

    owner
        .set_files(&files, FileOp::Copy)
        .expect("replace with copy");
    assert_eq!(
        reader.get_files().expect("get copy"),
        Some((files, FileOp::Copy))
    );

    // Large list: exceeds one request, so the owner must serve ICCCM INCR chunks.
    let large: Vec<PathBuf> = (0..3000)
        .map(|i| PathBuf::from(format!("/tmp/large/entry-{i:05}-ünïcode name.txt")))
        .collect();
    owner.set_files(&large, FileOp::Cut).expect("set large");
    assert_eq!(
        reader.get_files().expect("get large"),
        Some((large, FileOp::Cut))
    );
}
