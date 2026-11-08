//! Shared job progress snapshots for any long file operation (copy, move,
//! archive extract). Workers own a `Reporter` and feed it byte and item counts
//! through a [`jobs::TransferSink`]; consumers receive `Snapshot`s throttled to
//! at most one per `MIN_INTERVAL`, with terminal states always delivered.
//! Cancellation uses the job's `CancellationToken` (see `jobs`): the worker
//! polls it per chunk, so the UI only ever forwards the intent.
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::jobs::{CancellationToken, JobId, JobState, TransferSink};

/// Minimum spacing between non-forced snapshots (10 per second).
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// One entry a job is working on, kept short for the progress window.
const MAX_LABEL_CHARS: usize = 64;

/// Point-in-time view of one job. Byte totals drive the percent when known;
/// jobs that cannot know their size ahead (archive extraction) fall back to
/// item counts and then to an indeterminate 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub job: JobId,
    pub state: JobState,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub items_done: u64,
    pub items_total: u64,
    pub current: Option<String>,
}

impl Snapshot {
    /// 0..=100. A completed job always reports 100.
    #[must_use]
    pub fn percent(&self) -> u8 {
        if self.state == JobState::Completed {
            return 100;
        }
        let (done, total) = if self.bytes_total > 0 {
            (self.bytes_done, self.bytes_total)
        } else {
            (self.items_done, self.items_total)
        };
        if total == 0 {
            0
        } else {
            (done.min(total) * 100 / total) as u8
        }
    }

    /// True when the denominator is known, so the bar can show a real share.
    #[must_use]
    pub fn is_determinate(&self) -> bool {
        self.bytes_total > 0 || self.items_total > 0 || self.state == JobState::Completed
    }

    /// One line for the status strip and the progress window: percent, then the
    /// current entry, or the state while there is no entry yet.
    #[must_use]
    pub fn label(&self) -> String {
        let percent = self.percent();
        let job = match self.state {
            JobState::Queued => "Waiting",
            JobState::Counting => "Counting",
            JobState::Running => "Working",
            JobState::Cancelling => "Cancelling",
            JobState::Completed => "Done",
            JobState::Cancelled => "Cancelled",
            JobState::PartiallyFailed => "Partially done",
            JobState::Failed => "Failed",
        };
        match &self.current {
            Some(name) if self.state == JobState::Running || self.state == JobState::Cancelling => {
                format!("{percent}% · {name}")
            }
            _ => format!("{percent}% · {job}"),
        }
    }

    /// Byte counters as "1.2 MiB of 8.0 MiB" when the total is known.
    #[must_use]
    pub fn bytes_text(&self) -> String {
        if self.bytes_total > 0 {
            format!(
                "{} of {}",
                flamewm_api::format::human_size(self.bytes_done),
                flamewm_api::format::human_size(self.bytes_total)
            )
        } else if self.bytes_done > 0 {
            format!(
                "{} transferred",
                flamewm_api::format::human_size(self.bytes_done)
            )
        } else {
            String::new()
        }
    }

    /// Cancelling flag shared with the worker thread.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(
            self.state,
            JobState::Completed
                | JobState::PartiallyFailed
                | JobState::Cancelled
                | JobState::Failed
        )
    }
}

/// Short display name for a path (never the full path: it can be long).
fn name_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let mut text: String = name.chars().take(MAX_LABEL_CHARS).collect();
    if name.chars().count() > MAX_LABEL_CHARS {
        text.push('…');
    }
    Some(text)
}

/// Worker-side accumulator. Counters only grow; `emit` is the throttle gate and
/// `out` is where accepted snapshots go (the worker's wake channel).
pub struct Reporter {
    cancel: CancellationToken,
    snapshot: Snapshot,
    last_sent: Option<Instant>,
    out: Option<Box<dyn FnMut(Snapshot) + Send>>,
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The sink is a boxed closure: report whether one is attached, not its value.
        f.debug_struct("Reporter")
            .field("snapshot", &self.snapshot)
            .field("connected", &self.out.is_some())
            .finish_non_exhaustive()
    }
}

impl Reporter {
    #[must_use]
    pub fn new(job: JobId, cancel: CancellationToken) -> Self {
        let snapshot = Snapshot {
            job,
            state: JobState::Queued,
            bytes_done: 0,
            bytes_total: 0,
            items_done: 0,
            items_total: 0,
            current: None,
        };
        Reporter {
            cancel,
            snapshot,
            last_sent: None,
            out: None,
        }
    }

    /// A reporter that forwards every accepted snapshot to `out`, throttled to
    /// `MIN_INTERVAL`. This is the only place the throttle is applied, so the
    /// worker never has to track time itself.
    #[must_use]
    pub fn connected(
        job: JobId,
        cancel: CancellationToken,
        out: impl FnMut(Snapshot) + Send + 'static,
    ) -> Self {
        Reporter {
            out: Some(Box::new(out)),
            ..Reporter::new(job, cancel)
        }
    }

    /// Cancel handle shared with the worker; `cancel()` stops the job cooperatively.
    #[must_use]
    pub fn cancel_handle(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn set_state(&mut self, state: JobState) {
        self.snapshot.state = state;
    }

    #[must_use]
    pub fn state(&self) -> JobState {
        self.snapshot.state
    }

    /// Announces a state change immediately, bypassing the throttle.
    pub fn announce(&mut self, state: JobState) -> Snapshot {
        self.set_state(state);
        let snapshot = self.force();
        if let Some(out) = self.out.as_mut() {
            out(snapshot.clone());
        }
        snapshot
    }

    pub fn set_totals(&mut self, bytes: u64, items: u64) {
        self.snapshot.bytes_total = bytes;
        self.snapshot.items_total = items;
    }

    pub fn set_current(&mut self, path: &Path) {
        self.snapshot.current = name_of(path);
    }

    pub fn add_bytes(&mut self, bytes: u64) {
        self.snapshot.bytes_done = self.snapshot.bytes_done.saturating_add(bytes);
    }

    pub fn finish_item(&mut self) {
        self.snapshot.items_done = self.snapshot.items_done.saturating_add(1);
    }

    /// Returns a snapshot when at least `MIN_INTERVAL` passed since the last one.
    /// `now` is passed in so callers and tests control the clock.
    pub fn emit(&mut self, now: Instant) -> Option<Snapshot> {
        if self
            .last_sent
            .is_some_and(|last| now.duration_since(last) < MIN_INTERVAL)
        {
            return None;
        }
        self.last_sent = Some(now);
        Some(self.snapshot.clone())
    }

    /// Throttled publish of the current counters to the connected sink.
    fn publish(&mut self, now: Instant) {
        if let Some(snapshot) = self.emit(now)
            && let Some(out) = self.out.as_mut()
        {
            out(snapshot);
        }
    }

    /// Snapshot regardless of the throttle: used when the state changes.
    pub fn force(&mut self) -> Snapshot {
        self.last_sent = Some(Instant::now());
        self.snapshot.clone()
    }

    /// Records a terminal state and always returns (and publishes) a snapshot. A
    /// completed job reports full byte and item counts.
    pub fn finish(&mut self, state: JobState) -> Snapshot {
        self.snapshot.state = state;
        if state == JobState::Completed {
            self.snapshot.bytes_done = self.snapshot.bytes_total;
            self.snapshot.items_done = self.snapshot.items_total;
        }
        let snapshot = self.force();
        if let Some(out) = self.out.as_mut() {
            out(snapshot.clone());
        }
        snapshot
    }
}

/// The reporter is the worker's sink: bytes, current entry and finished items.
impl TransferSink for Reporter {
    fn bytes(&mut self, bytes: u64) {
        self.add_bytes(bytes);
        self.publish(Instant::now());
    }
    fn entry(&mut self, source: &Path) {
        self.set_current(source);
        self.publish(Instant::now());
    }
    fn item_done(&mut self) {
        self.finish_item();
        self.publish(Instant::now());
    }
}

/// Failure category for UI and activity text. Typed so callers never match on messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Cancelled,
    PermissionDenied,
    DiskFull,
    Other,
}

/// Maps an I/O error from a job to a `FailureKind`. Cancellation is reported
/// as `Interrupted` by `jobs`.
#[must_use]
pub fn classify(error: &io::Error) -> FailureKind {
    match error.kind() {
        io::ErrorKind::Interrupted => FailureKind::Cancelled,
        io::ErrorKind::PermissionDenied => FailureKind::PermissionDenied,
        io::ErrorKind::StorageFull => FailureKind::DiskFull,
        _ => match error.raw_os_error() {
            Some(code)
                if code == rustix::io::Errno::NOSPC.raw_os_error()
                    || code == rustix::io::Errno::DQUOT.raw_os_error() =>
            {
                FailureKind::DiskFull
            }
            Some(code)
                if code == rustix::io::Errno::ACCESS.raw_os_error()
                    || code == rustix::io::Errno::PERM.raw_os_error() =>
            {
                FailureKind::PermissionDenied
            }
            _ => FailureKind::Other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobId, copy_publish};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "flamewm-progress-{tag}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn percent_prefers_bytes_then_items_and_completed_is_full() {
        let mut reporter = Reporter::new(JobId(1), CancellationToken::new());
        assert_eq!(reporter.force().percent(), 0, "no denominator yet");
        reporter.set_totals(1000, 4);
        reporter.add_bytes(250);
        assert_eq!(reporter.force().percent(), 25);
        assert!(reporter.force().is_determinate());
        // Bytes win while known, even when items are ahead.
        reporter.set_state(JobState::Running);
        reporter.finish_item();
        reporter.finish_item();
        assert_eq!(reporter.force().percent(), 25);
        // Without a byte total the item counts drive the percent.
        reporter.set_totals(0, 4);
        assert_eq!(reporter.force().percent(), 50);
        let done = reporter.finish(JobState::Completed);
        assert_eq!(done.percent(), 100);
        assert!(done.is_finished());
    }

    #[test]
    fn label_names_the_current_entry_and_falls_back_to_the_state() {
        let mut reporter = Reporter::new(JobId(3), CancellationToken::new());
        reporter.set_totals(2048, 1);
        reporter.set_state(JobState::Running);
        reporter.add_bytes(819);
        assert_eq!(reporter.force().label(), "39% · Working");
        reporter.set_current(Path::new("/tmp/some folder/report.pdf"));
        assert_eq!(reporter.force().label(), "39% · report.pdf");
        let done = reporter.finish(JobState::Completed);
        assert_eq!(done.label(), "100% · Done");
        assert_eq!(done.bytes_text(), "2.0 KiB of 2.0 KiB");
    }

    #[test]
    fn emit_is_throttled_to_the_minimum_interval_and_force_bypasses_it() {
        let mut reporter = Reporter::new(JobId(1), CancellationToken::new());
        let t0 = Instant::now();
        assert!(reporter.emit(t0).is_some());
        assert!(reporter.emit(t0 + Duration::from_millis(50)).is_none());
        assert!(reporter.emit(t0 + MIN_INTERVAL).is_some());
        let forced = reporter.force();
        assert_eq!(forced.state, JobState::Queued);
    }

    #[test]
    fn reporter_counts_what_the_copy_sink_reports() {
        let dir = Scratch::new("sink");
        let src = dir.0.join("data");
        fs::write(&src, vec![3u8; 1024 * 1024]).unwrap();
        let dest = dir.0.join("copy");
        fs::create_dir(&dest).unwrap();
        let mut reporter = Reporter::new(JobId(4), CancellationToken::new());
        reporter.set_totals(1024 * 1024, 1);
        reporter.set_state(JobState::Running);
        // The batch path reports the entry name and the finished item through the sink.
        let report = crate::jobs::transfer_batch_with(
            std::slice::from_ref(&src),
            &dest,
            false,
            crate::jobs::Conflict::KeepBoth,
            &reporter.cancel_handle(),
            &mut reporter,
        );
        assert_eq!((report.completed, report.failed), (1, 0), "{report:?}");
        let done = reporter.finish(JobState::Completed);
        assert_eq!(done.percent(), 100);
        assert_eq!(done.current.as_deref(), Some("data"));
        assert_eq!(done.items_done, 1);
        assert_eq!(done.bytes_done, 1024 * 1024);
        assert_eq!(fs::read(dest.join("data")).unwrap().len(), 1024 * 1024);
    }

    #[test]
    fn copy_progress_is_monotonic_and_ends_at_100_percent() {
        let dir = Scratch::new("monotonic");
        let src = dir.0.join("data");
        fs::write(&src, vec![3u8; 1024 * 1024]).unwrap();
        let dest = dir.0.join("copy");
        let mut reporter = Reporter::new(JobId(2), CancellationToken::new());
        reporter.set_totals(1024 * 1024, 1);
        reporter.set_state(JobState::Running);
        reporter.set_current(&src);
        let mut percents = Vec::new();
        copy_publish(&src, &dest, false, &reporter.cancel_handle(), &mut |n| {
            reporter.add_bytes(n);
            if let Some(snapshot) = reporter.emit(Instant::now()) {
                percents.push(snapshot.percent());
            }
        })
        .unwrap();
        reporter.finish_item();
        let done = reporter.finish(JobState::Completed);
        percents.push(done.percent());
        assert!(
            percents.windows(2).all(|pair| pair[0] <= pair[1]),
            "{percents:?}"
        );
        assert_eq!(*percents.last().unwrap(), 100);
        assert_eq!(done.current.as_deref(), Some("data"));
    }

    #[test]
    fn classify_maps_permission_disk_full_and_cancel() {
        assert_eq!(
            classify(&io::Error::from(io::ErrorKind::PermissionDenied)),
            FailureKind::PermissionDenied
        );
        assert_eq!(
            classify(&io::Error::from(io::ErrorKind::StorageFull)),
            FailureKind::DiskFull
        );
        assert_eq!(
            classify(&io::Error::from_raw_os_error(28)),
            FailureKind::DiskFull
        );
        assert_eq!(
            classify(&io::Error::from_raw_os_error(122)),
            FailureKind::DiskFull
        );
        assert_eq!(
            classify(&io::Error::from(io::ErrorKind::Interrupted)),
            FailureKind::Cancelled
        );
        assert_eq!(
            classify(&io::Error::from(io::ErrorKind::NotFound)),
            FailureKind::Other
        );
    }

    #[test]
    fn unreadable_source_reports_permission_denied_without_partial_output() {
        let dir = Scratch::new("permission");
        let src = dir.0.join("locked");
        fs::write(&src, b"secret").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();
        let dest = dir.0.join("copy");
        let error = copy_publish(&src, &dest, false, &CancellationToken::new(), &mut |_| {})
            .expect_err("unreadable source must fail");
        assert_eq!(classify(&error), FailureKind::PermissionDenied);
        assert!(!dest.exists());
    }

    fn staging_left(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".flamewm-part-")
            })
            .count()
    }

    #[test]
    fn cancel_mid_copy_leaves_no_partial_destination() {
        let dir = Scratch::new("cancel");
        let src = dir.0.join("big");
        fs::write(&src, vec![7u8; 3 * 256 * 1024]).unwrap();
        let dest = dir.0.join("copy");
        let cancel = CancellationToken::new();
        let mut seen = 0_u64;
        let error = copy_publish(&src, &dest, false, &cancel, &mut |n| {
            seen += n;
            cancel.cancel();
        })
        .expect_err("cancelled copy must fail");
        assert_eq!(classify(&error), FailureKind::Cancelled);
        assert_eq!(seen, 256 * 1024, "cancel is checked per chunk");
        assert!(!dest.exists());
        assert_eq!(staging_left(&dir.0), 0);
        assert_eq!(fs::metadata(&src).unwrap().len(), 3 * 256 * 1024);
    }

    #[test]
    fn cancel_latency_is_bounded_by_one_chunk() {
        // A 64 MB source with an immediate cancel: the worker must stop after the
        // chunk in flight, so residual work is bounded by CHUNK, not by the file.
        let dir = Scratch::new("latency");
        let src = dir.0.join("huge");
        fs::write(&src, vec![9u8; 64 * 1024 * 1024]).unwrap();
        let dest = dir.0.join("copy");
        let cancel = CancellationToken::new();
        let started = Instant::now();
        let mut chunks = 0_u64;
        let error = copy_publish(&src, &dest, false, &cancel, &mut |_| {
            chunks += 1;
            cancel.cancel();
        })
        .expect_err("cancelled copy must fail");
        let elapsed = started.elapsed();
        assert_eq!(classify(&error), FailureKind::Cancelled);
        assert_eq!(chunks, 1, "no work after the cancel was observed");
        assert!(elapsed < Duration::from_secs(2), "cancel took {elapsed:?}");
        assert!(!dest.exists());
        assert_eq!(staging_left(&dir.0), 0);
    }

    #[test]
    fn cross_device_move_keeps_source_on_cancel_and_on_failure() {
        let dir = Scratch::new("move");
        let src = dir.0.join("item");
        fs::write(&src, vec![1u8; 512 * 1024]).unwrap();
        let dest = dir.0.join("moved");

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let error = crate::jobs::cross_device_move(&src, &dest, false, &cancelled, &mut |_| {})
            .expect_err("pre-cancelled move must fail");
        assert_eq!(classify(&error), FailureKind::Cancelled);
        assert!(src.exists() && !dest.exists());

        fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();
        let error = crate::jobs::cross_device_move(
            &src,
            &dest,
            false,
            &CancellationToken::new(),
            &mut |_| {},
        )
        .expect_err("unreadable source must fail");
        assert_eq!(classify(&error), FailureKind::PermissionDenied);
        fs::set_permissions(&src, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(src.exists() && !dest.exists());
        assert_eq!(staging_left(&dir.0), 0);
    }
}
