//! Shared job progress snapshots for any long file operation (copy, move,
//! archive extract/create). Workers own a `Reporter` and feed it byte and item
//! counts; consumers receive `Snapshot`s throttled to at most one per
//! `MIN_INTERVAL` unless a state change is forced with `Reporter::finish`.
//! Cancellation uses the job's `CancellationToken` (see `jobs`).
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::jobs::{CancellationToken, JobId, JobState};

/// Minimum spacing between non-forced snapshots (10 per second).
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// Point-in-time view of one job. Byte totals drive the percent when known.
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
    /// 0..=100. Completed jobs always report 100.
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
}

/// Worker-side accumulator. Counters only grow; `emit` is the throttle gate.
#[derive(Debug)]
pub struct Reporter {
    cancel: CancellationToken,
    snapshot: Snapshot,
    last_sent: Option<Instant>,
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

    pub fn set_totals(&mut self, bytes: u64, items: u64) {
        self.snapshot.bytes_total = bytes;
        self.snapshot.items_total = items;
    }

    pub fn set_current(&mut self, path: &Path) {
        self.snapshot.current = path.file_name().map(|n| n.to_string_lossy().into_owned());
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

    /// Records a terminal state and always returns a snapshot. A completed job
    /// reports full byte and item counts.
    pub fn finish(&mut self, state: JobState) -> Snapshot {
        self.snapshot.state = state;
        if state == JobState::Completed {
            self.snapshot.bytes_done = self.snapshot.bytes_total;
            self.snapshot.items_done = self.snapshot.items_total;
        }
        self.snapshot.clone()
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
    fn emit_is_throttled_to_the_minimum_interval() {
        let mut reporter = Reporter::new(JobId(1), CancellationToken::new());
        let t0 = Instant::now();
        assert!(reporter.emit(t0).is_some());
        assert!(reporter.emit(t0 + Duration::from_millis(50)).is_none());
        assert!(reporter.emit(t0 + MIN_INTERVAL).is_some());
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
