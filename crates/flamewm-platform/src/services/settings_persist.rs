//! Trailing-edge debounced writer for the settings file.
//!
//! Live settings state is committed in memory by `SettingsService`; this writer only decides when
//! that text reaches disk. A burst of `schedule` calls (a slider drag) collapses into one atomic
//! write of the newest text, issued once the burst has been quiet for [`SETTINGS_IDLE_DEBOUNCE`]
//! or has run for [`SETTINGS_MAX_DEFER`]. The writer thread blocks on its channel while nothing is
//! pending, so an idle compositor arms no timer and wakes for nothing.
//!
//! Every write goes through this one thread in submission order, so a synchronous `write_now`
//! always supersedes any older pending text and a stale debounced write can never land last.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use flamewm_api::{ErrorCode, FlameError, FlameResult};

/// Quiet period after the last scheduled change before the pending text is written.
pub const SETTINGS_IDLE_DEBOUNCE: Duration = Duration::from_millis(300);
/// Upper bound on how long a continuous drag may defer the first write.
pub const SETTINGS_MAX_DEFER: Duration = Duration::from_secs(2);

type Sink = Box<dyn FnMut(&str) -> FlameResult<()> + Send>;

enum Command {
    /// Newest live text; written after the debounce window unless superseded.
    Schedule(String),
    /// Write this text now (superseding pending text) and reply with the outcome.
    WriteNow(String, Sender<FlameResult<()>>),
    /// Write pending text now and reply with the outcome of that write or of an earlier
    /// background failure that has not been reported yet.
    Flush(Sender<FlameResult<()>>),
}

/// Owns the settings-writer thread. Dropping it flushes pending text and joins the thread.
pub struct SettingsWriter {
    tx: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for SettingsWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsWriter").finish_non_exhaustive()
    }
}

impl SettingsWriter {
    /// Starts a writer that persists to `path` with the production debounce windows.
    #[must_use]
    pub fn spawn(path: std::path::PathBuf) -> Self {
        Self::with_sink(
            SETTINGS_IDLE_DEBOUNCE,
            SETTINGS_MAX_DEFER,
            Box::new(move |text| {
                crate::storage::write_atomic_text(&path, text)
                    .map_err(|error| FlameError::new(ErrorCode::IoFailure, error.to_string()))
            }),
        )
    }

    /// Starts a writer with explicit windows and sink. Tests use an in-memory sink.
    #[must_use]
    pub fn with_sink(idle: Duration, max_defer: Duration, sink: Sink) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("flamewm-settings-writer".to_owned())
            .spawn(move || run(&rx, idle, max_defer, sink))
            .expect("settings writer thread must start");
        Self {
            tx: Some(tx),
            thread: Some(thread),
        }
    }

    /// Queues `text` for a debounced write. Never blocks on disk.
    pub fn schedule(&self, text: String) {
        self.send(Command::Schedule(text));
    }

    /// Writes `text` before returning. Any pending older text is discarded.
    pub fn write_now(&self, text: String) -> FlameResult<()> {
        self.request(|reply| Command::WriteNow(text, reply))
    }

    /// Writes any pending text before returning.
    pub fn flush(&self) -> FlameResult<()> {
        self.request(Command::Flush)
    }

    fn send(&self, command: Command) {
        if let Some(tx) = &self.tx {
            // A dead writer thread has already flushed or lost its text; the next request reports it.
            let _ = tx.send(command);
        }
    }

    fn request(&self, build: impl FnOnce(Sender<FlameResult<()>>) -> Command) -> FlameResult<()> {
        let Some(tx) = &self.tx else {
            return Ok(());
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.send(build(reply_tx)).is_err() {
            return Err(FlameError::new(
                ErrorCode::IoFailure,
                "settings writer thread is not running",
            ));
        }
        reply_rx.recv().unwrap_or_else(|_| {
            Err(FlameError::new(
                ErrorCode::IoFailure,
                "settings writer thread exited before replying",
            ))
        })
    }
}

impl Drop for SettingsWriter {
    fn drop(&mut self) {
        // Disconnecting the channel makes the thread flush pending text, then exit.
        drop(self.tx.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(rx: &Receiver<Command>, idle: Duration, max_defer: Duration, mut sink: Sink) {
    let mut pending: Option<String> = None;
    let mut first_pending = Instant::now();
    let mut last_scheduled = first_pending;
    // Background failure not yet reported to a caller; surfaced by the next flush or write.
    let mut background_error: Option<String> = None;

    loop {
        let command = if pending.is_some() {
            let deadline = (last_scheduled + idle).min(first_pending + max_defer);
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(text) = pending.take()
                        && let Err(error) = sink(&text)
                    {
                        background_error = Some(error.message);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if let Some(text) = pending.take()
                        && let Err(error) = sink(&text)
                    {
                        // No caller is left to receive this error once the channel is gone.
                        eprintln!(
                            "flamewm: settings write on shutdown failed: {}",
                            error.message
                        );
                    }
                    return;
                }
            }
        } else {
            match rx.recv() {
                Ok(command) => command,
                Err(_) => return,
            }
        };

        match command {
            Command::Schedule(text) => {
                let now = Instant::now();
                if pending.is_none() {
                    first_pending = now;
                }
                last_scheduled = now;
                pending = Some(text);
            }
            Command::WriteNow(text, reply) => {
                pending = None;
                let result = sink(&text);
                background_error = None;
                let _ = reply.send(result);
            }
            Command::Flush(reply) => {
                let result = match pending.take() {
                    Some(text) => sink(&text),
                    None => Ok(()),
                };
                let result = match (result, background_error.take()) {
                    (Err(error), _) => Err(error),
                    (Ok(()), Some(message)) => Err(FlameError::new(ErrorCode::IoFailure, message)),
                    (Ok(()), None) => Ok(()),
                };
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    type Log = Arc<Mutex<Vec<String>>>;

    fn recording_sink(log: &Log) -> Sink {
        let log = Arc::clone(log);
        Box::new(move |text| {
            log.lock().expect("log").push(text.to_owned());
            Ok(())
        })
    }

    fn failing_sink() -> Sink {
        Box::new(|_| Err(FlameError::new(ErrorCode::IoFailure, "disk full")))
    }

    #[test]
    fn burst_of_schedules_coalesces_into_one_write_of_the_newest_text() {
        let log: Log = Arc::default();
        let writer = SettingsWriter::with_sink(
            Duration::from_millis(40),
            Duration::from_secs(5),
            recording_sink(&log),
        );
        for step in 0..20 {
            writer.schedule(format!("v{step}"));
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(*log.lock().expect("log"), vec!["v19".to_owned()]);
    }

    #[test]
    fn continuous_scheduling_is_bounded_by_max_defer() {
        let log: Log = Arc::default();
        let writer = SettingsWriter::with_sink(
            Duration::from_secs(5),
            Duration::from_millis(60),
            recording_sink(&log),
        );
        writer.schedule("first".to_owned());
        std::thread::sleep(Duration::from_millis(30));
        writer.schedule("second".to_owned());
        std::thread::sleep(Duration::from_millis(100));
        // Idle is 5s, so only the max-defer bound can have written; it writes the newest text.
        assert_eq!(*log.lock().expect("log"), vec!["second".to_owned()]);
    }

    #[test]
    fn flush_writes_pending_text_immediately_and_is_idempotent() {
        let log: Log = Arc::default();
        let writer = SettingsWriter::with_sink(
            Duration::from_secs(5),
            Duration::from_secs(5),
            recording_sink(&log),
        );
        writer.schedule("live".to_owned());
        writer.flush().expect("flush");
        writer.flush().expect("second flush has nothing to write");
        assert_eq!(*log.lock().expect("log"), vec!["live".to_owned()]);
    }

    #[test]
    fn write_now_supersedes_older_pending_text_so_it_never_lands_last() {
        let log: Log = Arc::default();
        let writer = SettingsWriter::with_sink(
            Duration::from_millis(30),
            Duration::from_secs(5),
            recording_sink(&log),
        );
        writer.schedule("stale".to_owned());
        writer.write_now("sync".to_owned()).expect("write_now");
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(*log.lock().expect("log"), vec!["sync".to_owned()]);
    }

    #[test]
    fn drop_flushes_pending_text_before_joining() {
        let log: Log = Arc::default();
        let writer = SettingsWriter::with_sink(
            Duration::from_secs(60),
            Duration::from_secs(60),
            recording_sink(&log),
        );
        writer.schedule("on-exit".to_owned());
        drop(writer);
        assert_eq!(*log.lock().expect("log"), vec!["on-exit".to_owned()]);
    }

    #[test]
    fn write_now_reports_sink_failure_to_the_caller() {
        let writer = SettingsWriter::with_sink(
            Duration::from_secs(5),
            Duration::from_secs(5),
            failing_sink(),
        );
        let error = writer.write_now("x".to_owned()).expect_err("sink failure");
        assert_eq!(error.code, ErrorCode::IoFailure);
    }

    #[test]
    fn background_failure_is_reported_by_the_next_flush_once() {
        let writer = SettingsWriter::with_sink(
            Duration::from_millis(20),
            Duration::from_secs(5),
            failing_sink(),
        );
        writer.schedule("lost".to_owned());
        std::thread::sleep(Duration::from_millis(100));
        assert!(writer.flush().is_err(), "background failure must surface");
        assert!(writer.flush().is_ok(), "failure is reported once");
    }

    /// Scratch directory under the system temp dir, removed on drop. Tests never touch the real config dir.
    struct ScratchDir(std::path::PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("flamewm-persist-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A path whose parent is a regular file, so no directory can be created for it. Root cannot bypass this,
    /// unlike a 0o500 directory.
    fn unwritable_path(scratch: &ScratchDir) -> std::path::PathBuf {
        let blocker = scratch.0.join("not-a-dir");
        std::fs::write(&blocker, "blocker").expect("blocker file");
        blocker.join("settings.conf")
    }

    #[test]
    fn write_now_under_unwritable_config_dir_reports_io_failure_and_writes_nothing() {
        let scratch = ScratchDir::new("write-now-blocked");
        let path = unwritable_path(&scratch);
        let writer = SettingsWriter::spawn(path.clone());

        let error = writer
            .write_now("[ui]\n".to_owned())
            .expect_err("unwritable config dir must fail the save");
        assert_eq!(error.code, ErrorCode::IoFailure);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(scratch.0.join("not-a-dir")).expect("blocker"),
            "blocker"
        );
        // The writer thread survives the failure and keeps reporting it on later requests.
        let again = writer
            .write_now("again".to_owned())
            .expect_err("still unwritable");
        assert_eq!(again.code, ErrorCode::IoFailure);
    }

    #[test]
    fn scheduled_save_under_unwritable_config_dir_surfaces_on_flush() {
        let scratch = ScratchDir::new("scheduled-blocked");
        let path = unwritable_path(&scratch);
        let writer = SettingsWriter::spawn(path);

        writer.schedule("live".to_owned());
        let error = writer
            .flush()
            .expect_err("debounced save failure must surface on flush");
        assert_eq!(error.code, ErrorCode::IoFailure);
        assert!(writer.flush().is_ok(), "reported once, then clear");
    }

    #[test]
    fn failed_rename_onto_directory_leaves_no_temp_file_behind() {
        let scratch = ScratchDir::new("rename-onto-dir");
        let path = scratch.0.join("settings.conf");
        std::fs::create_dir(&path).expect("directory at target path");
        let writer = SettingsWriter::spawn(path.clone());

        let error = writer
            .write_now("v".to_owned())
            .expect_err("rename onto a directory must fail");
        assert_eq!(error.code, ErrorCode::IoFailure);
        assert!(path.is_dir(), "target left untouched");
        let leftovers = std::fs::read_dir(&scratch.0)
            .expect("scratch listing")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("settings.conf.") && name.ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "temp file must be removed on failure");
    }

    #[test]
    fn drop_with_pending_save_to_unwritable_dir_does_not_panic() {
        let scratch = ScratchDir::new("drop-blocked");
        let path = unwritable_path(&scratch);
        let writer = SettingsWriter::spawn(path.clone());
        writer.schedule("on-exit".to_owned());
        drop(writer);
        assert!(!path.exists());
    }
}
