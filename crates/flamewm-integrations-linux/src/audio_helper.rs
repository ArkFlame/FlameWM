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
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

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

/// Helper process state shared by the writer thread and the reader threads it starts.
struct Core {
    program: PathBuf,
    args: Vec<String>,
    on_snapshot: Arc<Mutex<SnapshotCallback>>,
    process: Arc<Mutex<Process>>,
}

/// Lazily spawned audio helper; holds no libpulse state in the WM process.
///
/// Requests are queued to one writer thread, so a caller (the WM control thread) never blocks on
/// spawning the helper or on a full helper stdin pipe. FIFO order is preserved, so requests reach
/// the helper in the order they were made.
pub struct AudioHelper {
    core: Arc<Core>,
    requests: Option<Sender<String>>,
    writer: Option<JoinHandle<()>>,
}

impl AudioHelper {
    /// Create the client. The helper is not started until the first request.
    pub fn connect<F>(on_snapshot: F) -> FlameResult<Self>
    where
        F: FnMut(AudioSnapshot) + Send + 'static,
    {
        Self::new(helper_program(), Vec::new(), Box::new(on_snapshot))
    }

    fn new(
        program: PathBuf,
        args: Vec<String>,
        on_snapshot: SnapshotCallback,
    ) -> FlameResult<Self> {
        let core = Arc::new(Core {
            program,
            args,
            on_snapshot: Arc::new(Mutex::new(on_snapshot)),
            process: Arc::new(Mutex::new(Process::default())),
        });
        let (sender, receiver) = mpsc::channel::<String>();
        let writer_core = Arc::clone(&core);
        let writer = thread::Builder::new()
            .name("flamewm-audio-writer".to_owned())
            .spawn(move || writer_core.run_writer(&receiver))
            .map_err(|error| FlameError::new(ErrorCode::IoFailure, error.to_string()))?;
        Ok(Self {
            core,
            requests: Some(sender),
            writer: Some(writer),
        })
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

    /// Queue one request for the writer thread. Never blocks on the helper; fails only when the
    /// writer has stopped.
    fn send(&self, request: &AudioRequest) -> FlameResult<()> {
        self.requests
            .as_ref()
            .ok_or_else(writer_stopped)?
            .send(encode_request(request))
            .map_err(|_| writer_stopped())
    }

    #[cfg(test)]
    fn child_pid(&self) -> Option<u32> {
        lock(&self.core.process)
            .session
            .as_ref()
            .map(|session| session.child.id())
    }
}

fn writer_stopped() -> FlameError {
    FlameError::new(
        ErrorCode::Unavailable,
        "audio helper writer thread is not running",
    )
}

impl Core {
    /// Writer thread body: delivers queued lines in order until the queue is closed.
    fn run_writer(&self, receiver: &Receiver<String>) {
        for line in receiver {
            if let Err(error) = self.deliver(&line) {
                eprintln!("flamewm-integrations-linux: audio request dropped: {error}");
            }
        }
        if let Some(session) = lock(&self.process).session.take() {
            retire(session.child);
        }
    }

    /// Write one encoded request, spawning or respawning the helper as needed.
    fn deliver(&self, line: &str) -> FlameResult<()> {
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
}

impl Drop for AudioHelper {
    fn drop(&mut self) {
        // Kill the live helper first so a writer blocked on its stdin wakes with an error, then
        // close the queue and join the writer.
        if let Some(session) = lock(&self.core.process).session.take() {
            retire(session.child);
        }
        self.requests.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
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
    if process
        .session
        .as_ref()
        .is_some_and(|session| session.id == id)
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
    /// helper would. Runs over real stdin/stdout pipes; never touches `PulseAudio`.
    /// The short sleep keeps the child alive long enough for a poll to observe its pid.
    const ONE_SHOT_HELPER: &str =
        "read _line; printf 'snapshot available 1 1 50 0 x\\nend\\n'; sleep 0.2; exit 0";

    fn recorder() -> (Arc<Mutex<Vec<AudioSnapshot>>>, AudioHelper) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let helper = AudioHelper::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_owned(), ONE_SHOT_HELPER.to_owned()],
            Box::new(move |snapshot| lock(&sink).push(snapshot)),
        )
        .expect("writer thread starts");
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
            assert!(
                Instant::now() < deadline,
                "timed out waiting for helper event"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits for the writer thread to spawn a child and returns its pid once the child exits.
    fn wait_for_spawn_and_exit(helper: &AudioHelper) -> u32 {
        let mut pid = None;
        wait_for(|| {
            pid = pid.or_else(|| helper.child_pid());
            pid.is_some() && helper.child_pid().is_none()
        });
        pid.expect("spawned child pid")
    }

    #[test]
    fn exited_helper_is_reaped_without_waiting_for_next_request() {
        let (seen, helper) = recorder();
        helper.request_snapshot().expect("queue first request");
        let pid = wait_for_spawn_and_exit(&helper);
        assert_eq!(lock(&seen).len(), 1);
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "exited helper left a zombie"
        );
    }

    #[test]
    fn respawns_after_helper_exit_and_applies_next_request() {
        let (seen, helper) = recorder();
        helper.request_snapshot().expect("queue first request");
        let first = wait_for_spawn_and_exit(&helper);
        helper
            .set_volume(volume_action())
            .expect("queue respawn for volume");
        wait_for(|| helper.child_pid().is_some());
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
            session: Some(Session {
                id: 1,
                child,
                stdin,
            }),
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

    // REGRESSION: a helper that never reads stdin must not block the caller once its pipe is full
    // (the raise-maximum freeze). Requests queue to the writer thread, so every send returns at once.
    // MUTATION: writing synchronously on the caller thread makes this test hang past its budget.
    #[test]
    fn requests_to_a_helper_that_never_reads_do_not_block_the_caller() {
        let helper = AudioHelper::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_owned(), "sleep 30".to_owned()],
            Box::new(|_| {}),
        )
        .expect("writer thread starts");
        let started = Instant::now();
        for _ in 0..2000 {
            helper.set_volume(volume_action()).expect("request queued");
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "queueing 2000 requests blocked the caller for {:?}",
            started.elapsed()
        );
    }

    // REGRESSION: a queued request after the writer has stopped reports Unavailable instead of
    // being silently dropped.
    #[test]
    fn send_after_writer_stop_reports_unavailable() {
        let mut helper = AudioHelper::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_owned(), "exit 0".to_owned()],
            Box::new(|_| {}),
        )
        .expect("writer thread starts");
        helper.requests.take();
        let error = helper
            .request_snapshot()
            .expect_err("stopped writer must refuse requests");
        assert_eq!(error.code, ErrorCode::Unavailable);
    }
}
