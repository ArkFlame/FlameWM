//! WM-side client for the `flamewm-audio-helper` process.
//!
//! The helper owns libpulse. This client spawns it on the first audio command, keeps its stdin
//! for requests, and forwards decoded snapshot events from a reader thread. The helper exits after
//! an idle period. The reader thread reaps the child when its stdout closes, and every send reaps
//! an already-exited child before deciding whether to respawn, so no zombie outlives its helper.
//!
//! The window manager also sends one `get` at startup, so the helper is started once before the
//! first user action; that is the only eager spawn.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use flamewm_api::system::{AudioMuteAction, AudioSnapshot, AudioVolumeAction};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::audio_protocol::{AudioRequest, SnapshotDecoder, encode_request};

/// Executable name of the helper, installed next to the WM binary.
pub const HELPER_BINARY: &str = "flamewm-audio-helper";

type SnapshotCallback = Box<dyn FnMut(AudioSnapshot) + Send>;

/// One running helper. `id` lets a reader thread reap only the child it was started for.
struct Session {
    id: u64,
    child: Child,
    stdin: ChildStdin,
}

#[derive(Default)]
struct Process {
    next_id: u64,
    session: Option<Session>,
}

/// Lazily spawned audio helper; holds no libpulse state in the WM process.
pub struct AudioHelper {
    program: PathBuf,
    args: Vec<String>,
    on_snapshot: Arc<Mutex<SnapshotCallback>>,
    process: Arc<Mutex<Process>>,
}

impl AudioHelper {
    /// Create the client. The helper is not started until the first request.
    pub fn connect<F>(on_snapshot: F) -> FlameResult<Self>
    where
        F: FnMut(AudioSnapshot) + Send + 'static,
    {
        Ok(Self::new(helper_program(), Vec::new(), Box::new(on_snapshot)))
    }

    fn new(program: PathBuf, args: Vec<String>, on_snapshot: SnapshotCallback) -> Self {
        Self {
            program,
            args,
            on_snapshot: Arc::new(Mutex::new(on_snapshot)),
            process: Arc::new(Mutex::new(Process::default())),
        }
    }

    /// Ask the helper for its current snapshot, starting it if needed.
    pub fn request_snapshot(&self) -> FlameResult<()> {
        self.send(&AudioRequest::Get)
    }

    pub fn set_volume(&self, action: AudioVolumeAction) -> FlameResult<()> {
        self.send(&AudioRequest::Volume(action))
    }

    pub fn set_mute(&self, action: AudioMuteAction) -> FlameResult<()> {
        self.send(&AudioRequest::Mute(action))
    }

    fn send(&self, request: &AudioRequest) -> FlameResult<()> {
        let line = encode_request(request);
        let mut process = lock(&self.process);
        for _attempt in 0..2 {
            reap_exited(&mut process);
            if process.session.is_none() {
                self.spawn(&mut process)?;
            }
            let Some(session) = process.session.as_mut() else {
                break;
            };
            if writeln!(session.stdin, "{line}")
                .and_then(|()| session.stdin.flush())
                .is_ok()
            {
                return Ok(());
            }
            if let Some(session) = process.session.take() {
                retire(session.child);
            }
        }
        Err(FlameError::new(
            ErrorCode::Unavailable,
            "audio helper is not accepting requests",
        ))
    }

    fn spawn(&self, process: &mut Process) -> FlameResult<()> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                FlameError::new(
                    ErrorCode::IoFailure,
                    format!("failed to start {HELPER_BINARY}: {error}"),
                )
            })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            retire(child);
            return Err(FlameError::new(
                ErrorCode::IoFailure,
                format!("{HELPER_BINARY} pipes are missing"),
            ));
        };
        process.next_id = process.next_id.wrapping_add(1);
        let id = process.next_id;
        let callback = Arc::clone(&self.on_snapshot);
        let shared = Arc::clone(&self.process);
        // The reader blocks on the process lock held here until the session is installed.
        let reader = thread::Builder::new()
            .name("flamewm-audio-reader".to_owned())
            .spawn(move || read_events(stdout, &callback, &shared, id));
        if let Err(error) = reader {
            retire(child);
            return Err(FlameError::new(ErrorCode::IoFailure, error.to_string()));
        }
        process.session = Some(Session { id, child, stdin });
        Ok(())
    }

    #[cfg(test)]
    fn child_pid(&self) -> Option<u32> {
        lock(&self.process).session.as_ref().map(|session| session.child.id())
    }
}

impl Drop for AudioHelper {
    fn drop(&mut self) {
        if let Some(session) = lock(&self.process).session.take() {
            retire(session.child);
        }
    }
}

fn read_events(
    stdout: ChildStdout,
    callback: &Mutex<SnapshotCallback>,
    process: &Mutex<Process>,
    id: u64,
) {
    let mut decoder = SnapshotDecoder::default();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else {
            break;
        };
        match decoder.push_line(&line) {
            Ok(Some(snapshot)) => {
                if let Ok(mut emit) = callback.lock() {
                    emit(snapshot);
                }
            }
            Ok(None) => {}
            Err(_) => decoder = SnapshotDecoder::default(),
        }
    }
    // stdout closed: the helper exited (idle timeout or crash). Reap it now instead of at the
    // next request. A newer session has a different id and is left alone.
    let mut process = lock(process);
    if process.session.as_ref().is_some_and(|session| session.id == id)
        && let Some(session) = process.session.take()
    {
        retire(session.child);
    }
}

/// Reap the helper if it has already exited. Returns its status when it was collected.
fn reap_exited(process: &mut Process) -> Option<ExitStatus> {
    let session = process.session.as_mut()?;
    match session.child.try_wait() {
        Ok(None) => None,
        Ok(Some(status)) => {
            process.session = None;
            Some(status)
        }
        Err(_) => {
            if let Some(session) = process.session.take() {
                retire(session.child);
            }
            None
        }
    }
}

/// Stop and collect a helper that is no longer usable.
fn retire(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn helper_program() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(HELPER_BINARY)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(HELPER_BINARY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use flamewm_api::system::{AudioEndpointKind, AudioTarget};

    /// Fake helper: answers the first request with one snapshot block, then exits like an idle
    /// helper would. Runs over real stdin/stdout pipes; never touches PulseAudio.
    const ONE_SHOT_HELPER: &str = "read _line; printf 'snapshot available 1 1 50 0 x\\nend\\n'; exit 0";

    fn recorder() -> (Arc<Mutex<Vec<AudioSnapshot>>>, AudioHelper) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let helper = AudioHelper::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_owned(), ONE_SHOT_HELPER.to_owned()],
            Box::new(move |snapshot| lock(&sink).push(snapshot)),
        );
        (seen, helper)
    }

    fn volume_action() -> AudioVolumeAction {
        AudioVolumeAction {
            target: AudioTarget::Endpoint {
                id: 1,
                kind: AudioEndpointKind::Sink,
            },
            percent: 50,
            generation: 1,
            server_generation: 1,
        }
    }

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for helper event");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn exited_helper_is_reaped_without_waiting_for_next_request() {
        let (seen, helper) = recorder();
        helper.request_snapshot().expect("first spawn");
        let pid = helper.child_pid().expect("running child");
        wait_for(|| helper.child_pid().is_none());
        assert_eq!(lock(&seen).len(), 1);
        assert!(!Path::new(&format!("/proc/{pid}")).exists(), "exited helper left a zombie");
    }

    #[test]
    fn respawns_after_helper_exit_and_applies_next_request() {
        let (seen, helper) = recorder();
        helper.request_snapshot().expect("first spawn");
        let first = helper.child_pid().expect("running child");
        wait_for(|| helper.child_pid().is_none());
        helper.set_volume(volume_action()).expect("respawn for volume");
        let second = helper.child_pid().expect("respawned child");
        assert_ne!(first, second);
        wait_for(|| lock(&seen).len() == 2);
    }

    #[test]
    fn send_reaps_an_already_exited_child() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn fake child");
        let stdin = child.stdin.take().expect("stdin");
        let pid = child.id();
        let mut process = Process {
            next_id: 1,
            session: Some(Session { id: 1, child, stdin }),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = reap_exited(&mut process) {
                break status;
            }
            assert!(Instant::now() < deadline, "child never exited");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.code(), Some(3));
        assert!(process.session.is_none());
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
}
