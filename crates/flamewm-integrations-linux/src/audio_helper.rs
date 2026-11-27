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

use flamewm_api::system::{AudioMuteAction, AudioSnapshot, AudioVolumeAction, ServiceAvailability};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::audio_protocol::{AudioRequest, SnapshotDecoder, encode_request};

/// Executable name of the helper, installed next to the WM binary.
pub const HELPER_BINARY: &str = "flamewm-audio-helper";

/// A helper that cannot start retries on a doubling schedule, never a fixed short tick.
const RETRY_FLOOR: std::time::Duration = std::time::Duration::from_millis(300);
const RETRY_CAP: std::time::Duration = std::time::Duration::from_secs(8);

/// Arm the bounded retry window after a failed spawn or a crashed helper, so an absent backend
/// is not respawned in a tight loop on every subsequent request.
fn schedule_retry(process: &mut Process) {
    process.failures = process.failures.saturating_add(1);
    let shift = u32::from(process.failures.min(7));
    let delay = RETRY_FLOOR.saturating_mul(1_u32 << shift).min(RETRY_CAP);
    process.retry_at = Some(std::time::Instant::now() + delay);
}

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
    /// Generations of the last published snapshot, so a synthesized Unavailable keeps its place.
    last_generation: u64,
    last_server_generation: u64,
    /// Earliest instant a failed spawn may be retried; None while no backoff is pending.
    retry_at: Option<std::time::Instant>,
    failures: u8,
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
        loop {
            let retry_at = lock(&self.process).retry_at;
            let line = match retry_at {
                Some(at) => match receiver
                    .recv_timeout(at.saturating_duration_since(std::time::Instant::now()))
                {
                    Ok(line) => line,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        encode_request(&AudioRequest::Get)
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                },
                None => match receiver.recv() {
                    Ok(line) => line,
                    Err(_) => break,
                },
            };
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
        if process
            .retry_at
            .is_some_and(|at| std::time::Instant::now() < at)
        {
            return Err(FlameError::new(
                ErrorCode::Unavailable,
                "audio helper is backing off",
            ));
        }
        process.retry_at = None;
        for _attempt in 0..2 {
            reap_exited(&mut process);
            if process.session.is_none() && self.spawn(&mut process).is_err() {
                break;
            }
            let Some(session) = process.session.as_mut() else {
                break;
            };
            if writeln!(session.stdin, "{line}")
                .and_then(|()| session.stdin.flush())
                .is_ok()
            {
                process.failures = 0;
                return Ok(());
            }
            if let Some(session) = process.session.take() {
                retire(session.child);
            }
        }
        schedule_retry(&mut process);
        drop(process);
        publish_unavailable(&self.on_snapshot, &self.process);
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
            // A pending backoff wake holds the writer in recv_timeout. Closing the queue makes its
            // next wake exit, so joining here would only stall shutdown until that one deadline.
            if lock(&self.core.process).retry_at.is_none() {
                let _ = writer.join();
            }
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
                {
                    let mut state = lock(process);
                    state.last_generation = snapshot.generation;
                    state.last_server_generation = snapshot.server_generation;
                }
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
    let status = {
        let mut process = lock(process);
        let status = if process
            .session
            .as_ref()
            .is_some_and(|session| session.id == id)
        {
            process
                .session
                .take()
                .and_then(|session| retire(session.child))
        } else {
            None
        };
        // A backend that died mid-session must not be respawned at once on the next request.
        if status.is_some_and(|status| !status.success()) {
            schedule_retry(&mut process);
        }
        status
    };
    // A backend that died mid-session must hide the controls; an idle exit is a normal success.
    if status.is_some_and(|status| !status.success()) {
        publish_unavailable(callback, process);
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

/// Stop and collect a helper that is no longer usable. Returns its status when collected.
fn retire(mut child: Child) -> Option<ExitStatus> {
    let _ = child.kill();
    child.wait().ok()
}

/// Publish Unavailable with the last seen generations so the host accepts the state change
/// and repeated failures dedupe instead of republishing on every retry.
fn publish_unavailable(callback: &Mutex<SnapshotCallback>, process: &Mutex<Process>) {
    let (generation, server_generation) = {
        let state = lock(process);
        (state.last_generation, state.last_server_generation)
    };
    let mut emit = lock(callback);
    emit(AudioSnapshot {
        availability: ServiceAvailability::Unavailable,
        generation,
        server_generation,
        ..AudioSnapshot::default()
    });
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

    // REGRESSION: a crashed helper must arm the retry window; without it an absent backend is
    // respawned on every following request with no delay.
    #[test]
    fn crashed_helper_arms_bounded_retry() {
        let helper = AudioHelper::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_owned(), "read _line; exit 3".to_owned()],
            Box::new(|_| {}),
        )
        .expect("writer thread starts");
        helper.request_snapshot().expect("queue first request");
        wait_for(|| lock(&helper.core.process).retry_at.is_some());
        let retry_at = lock(&helper.core.process).retry_at.expect("retry armed");
        assert!(
            retry_at.saturating_duration_since(std::time::Instant::now()) <= RETRY_CAP,
            "retry window exceeded cap"
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
            ..Process::default()
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

    // REGRESSION: an absent helper used to leave the snapshot at Unknown, so the shell kept the
    // audio controls visible forever. It must publish Unavailable and schedule a backed-off retry.
    #[test]
    fn missing_helper_hides_audio_and_retries_with_backoff() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let helper = AudioHelper::new(
            PathBuf::from("/no/such/flamewm-audio-helper"),
            Vec::new(),
            Box::new(move |snapshot| lock(&sink).push(snapshot)),
        )
        .expect("writer thread starts");
        helper.request_snapshot().expect("queue request");
        wait_for(|| !lock(&seen).is_empty());
        assert_eq!(
            lock(&seen)[0].availability,
            ServiceAvailability::Unavailable
        );
        let (failures, retry_at) = {
            let process = lock(&helper.core.process);
            (process.failures, process.retry_at)
        };
        assert_eq!(failures, 1);
        assert!(retry_at.is_some());
    }
}
