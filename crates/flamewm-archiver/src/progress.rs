//! Typed progress and cancellation shared by every long archive operation.
//!
//! `CancelToken` is checked before every entry and every read chunk. `Reporter` owns the running
//! totals and feeds the caller's sink in coarse steps (about a hundred per operation), so memory
//! stays bounded by the copy buffer and the UI is not woken once per chunk. The first step that
//! carries bytes and the completion snapshot always report, so a caller sees real progress and a
//! final total even for a job that is over within the emission interval.

use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::{ArchiveError, Result};

/// Cloneable cancel flag. Cancelling is idempotent and visible across threads.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Snapshot emitted to the caller's sink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub entries_done: usize,
    pub entries_total: usize,
}

/// Final counts returned by a completed operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub dirs: usize,
    /// Symlinks, hard links and special files that were not materialised.
    pub skipped: usize,
    pub bytes: u64,
}

/// Smallest byte step between two progress emissions.
const MIN_BYTE_STEP: u64 = 64 * 1024;
/// Emissions per operation when the totals are known (the bar moves in 1% steps).
const EMISSIONS_PER_OPERATION: u64 = 100;
/// Minimum spacing between two emissions; a fast archive is capped at ten updates a second.
const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// Running totals plus cancel check for one operation.
///
/// The sink is fed at most ~100 times per operation: every 1/100th of the totals, plus the
/// completion snapshot and the first snapshot that carries bytes. Each emission costs a channel
/// send and a UI wake, so a per-chunk sink would repaint the progress window thousands of times
/// for one large file.
pub struct Reporter<'a> {
    sink: &'a mut dyn FnMut(Progress),
    cancel: &'a CancelToken,
    state: Progress,
    /// Bytes counted since the last emission.
    pending_bytes: u64,
    /// Entries finished since the last emission.
    pending_entries: usize,
    /// Byte step between emissions; never zero.
    byte_step: u64,
    /// Entry step between emissions; never zero.
    entry_step: usize,
    /// Whether a snapshot carrying bytes has been emitted. The first one skips the interval so
    /// a caller sees real progress even when the whole job finishes in a few milliseconds.
    first_bytes_emitted: bool,
    /// When the sink last received a snapshot; the constructor's initial one counts.
    last_emit: Option<Instant>,
}

impl<'a> Reporter<'a> {
    pub fn new(
        bytes_total: u64,
        entries_total: usize,
        cancel: &'a CancelToken,
        sink: &'a mut dyn FnMut(Progress),
    ) -> Self {
        let state = Progress {
            bytes_total,
            entries_total,
            ..Progress::default()
        };
        sink(state);
        Self {
            sink,
            cancel,
            state,
            pending_bytes: 0,
            pending_entries: 0,
            byte_step: (bytes_total / EMISSIONS_PER_OPERATION).max(MIN_BYTE_STEP),
            entry_step: (entries_total / EMISSIONS_PER_OPERATION as usize).max(1),
            first_bytes_emitted: false,
            last_emit: Some(Instant::now()),
        }
    }

    pub fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(ArchiveError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Maps an I/O failure to `Cancelled` when the failure was caused by cancellation.
    pub fn fail(&self, err: io::Error) -> ArchiveError {
        if self.cancel.is_cancelled() {
            ArchiveError::Cancelled
        } else {
            ArchiveError::Io(err)
        }
    }

    pub fn add_bytes(&mut self, n: u64) {
        self.state.bytes_done += n;
        self.pending_bytes += n;
        self.maybe_emit();
    }

    pub fn finish_entry(&mut self) {
        self.state.entries_done += 1;
        self.pending_entries += 1;
        self.maybe_emit();
    }

    /// Reports the current totals when a step or the end of the operation is reached.
    fn maybe_emit(&mut self) {
        let complete = self.state.bytes_done >= self.state.bytes_total
            && self.state.entries_done >= self.state.entries_total;
        let stepped =
            self.pending_bytes >= self.byte_step || self.pending_entries >= self.entry_step;
        if !complete && !stepped {
            return;
        }
        let now = Instant::now();
        // Completion always reports. So does the first step that carries bytes: a caller learns
        // that real work has started without waiting out the interval. Every later step and
        // every entry-only step waits it out, so a fast archive still cannot wake the window
        // more than ten times a second.
        if !complete {
            let first_bytes = !self.first_bytes_emitted && self.state.bytes_done > 0;
            if !first_bytes
                && self
                    .last_emit
                    .is_some_and(|last| now.duration_since(last) < MIN_INTERVAL)
            {
                return;
            }
            if self.state.bytes_done > 0 {
                self.first_bytes_emitted = true;
            }
        }
        self.last_emit = Some(now);
        self.pending_bytes = 0;
        self.pending_entries = 0;
        (self.sink)(self.state);
    }
}

/// Read adapter that checks cancellation and reports bytes on every read.
pub struct Tracked<'r, 'a> {
    inner: &'r mut dyn Read,
    rep: &'r mut Reporter<'a>,
}

impl<'r, 'a> Tracked<'r, 'a> {
    pub fn new(inner: &'r mut dyn Read, rep: &'r mut Reporter<'a>) -> Self {
        Self { inner, rep }
    }
}

impl Read for Tracked<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.rep.check().is_err() {
            // Not `Interrupted`: `io::copy` retries that kind forever.
            return Err(io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.rep.add_bytes(n as u64);
        Ok(n)
    }
}

/// Copy buffer. Eight times the `io::copy` default, so a large entry needs one eighth of the
/// read/write syscall pairs. Shared with the tar writer's read buffer so creation prefetches the
/// same amount instead of tar's small internal chunk.
pub(crate) const COPY_BUFFER: usize = 64 * 1024;

thread_local! {
    /// Reused copy buffer. An archive of many small files used to allocate and zero a 64 KiB
    /// buffer for every entry; one scratch buffer per worker thread removes that per-entry churn.
    static COPY_SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Copies `src` into `dst` through the reporter. Returns the bytes copied.
pub fn copy_tracked(
    src: &mut dyn Read,
    dst: &mut dyn Write,
    rep: &mut Reporter<'_>,
) -> Result<u64> {
    rep.check()?;
    // The scratch is taken out of the thread-local for the copy and put back after, so a nested
    // copy on the same thread (re-entrant writer) simply finds it empty and uses a fresh buffer.
    let mut buffer = COPY_SCRATCH
        .try_with(|scratch| std::mem::take(&mut *scratch.borrow_mut()))
        .unwrap_or_default();
    if buffer.len() != COPY_BUFFER {
        buffer.resize(COPY_BUFFER, 0);
    }
    let result = copy_chunks(src, dst, rep, &mut buffer);
    let _ = COPY_SCRATCH.try_with(|scratch| *scratch.borrow_mut() = buffer);
    result
}

fn copy_chunks(
    src: &mut dyn Read,
    dst: &mut dyn Write,
    rep: &mut Reporter<'_>,
    buffer: &mut [u8],
) -> Result<u64> {
    let mut total = 0_u64;
    loop {
        // Cancel between chunks, like the `Tracked` reader does for tar entries.
        rep.check()?;
        let n = match src.read(&mut buffer[..]) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(rep.fail(err)),
        };
        dst.write_all(&buffer[..n]).map_err(|err| rep.fail(err))?;
        rep.add_bytes(n as u64);
        total += n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reporter_opens_with_a_zero_snapshot_and_suppresses_sub_step_updates() {
        let seen = std::cell::RefCell::new(Vec::new());
        {
            let cancel = CancelToken::new();
            let mut sink = |progress: Progress| seen.borrow_mut().push(progress);
            // Entries: 500 gives an entry step of 5, so one finished entry is below the step.
            let mut reporter = Reporter::new(1_000_000, 500, &cancel, &mut sink);
            assert_eq!(
                seen.borrow().len(),
                1,
                "the constructor emits one starting snapshot"
            );
            assert_eq!(
                seen.borrow()[0],
                Progress {
                    bytes_done: 0,
                    bytes_total: 1_000_000,
                    entries_done: 0,
                    entries_total: 500,
                }
            );
            reporter.add_bytes(1024);
            reporter.finish_entry();
            assert_eq!(
                seen.borrow().len(),
                1,
                "updates below both the byte and the entry step are coalesced"
            );
        }
    }

    #[test]
    fn reporter_reports_the_first_step_carrying_bytes_immediately() {
        let seen = std::cell::RefCell::new(Vec::new());
        {
            let cancel = CancelToken::new();
            let mut sink = |progress: Progress| seen.borrow_mut().push(progress);
            let mut reporter = Reporter::new(1_000_000, 1, &cancel, &mut sink);
            // The first step carrying bytes reports at once, without waiting out the emit
            // interval, so a caller learns that real work started even when the job is short.
            reporter.add_bytes(64 * 1024);
            assert_eq!(
                seen.borrow().len(),
                2,
                "the first byte step is not throttled"
            );
            assert_eq!(seen.borrow()[1].bytes_done, 64 * 1024);
            // Further steps inside the interval stay coalesced.
            reporter.add_bytes(64 * 1024);
            assert_eq!(seen.borrow().len(), 2);
        }
    }

    #[test]
    fn reporter_reports_completion_even_inside_the_minimum_interval() {
        let seen = std::cell::RefCell::new(Vec::new());
        {
            let cancel = CancelToken::new();
            let mut sink = |progress: Progress| seen.borrow_mut().push(progress);
            let mut reporter = Reporter::new(0, 0, &cancel, &mut sink);
            // Completion bypasses the step and the 100 ms interval: a finished
            // operation always reports its final totals.
            reporter.finish_entry();
            assert_eq!(seen.borrow().len(), 2);
            assert_eq!(
                seen.borrow()[1],
                Progress {
                    bytes_done: 0,
                    bytes_total: 0,
                    entries_done: 1,
                    entries_total: 0,
                }
            );
        }
    }

    #[test]
    fn cancel_is_visible_to_the_reporter_and_maps_io_failures_to_cancelled() {
        let cancel = CancelToken::new();
        let mut sink = |_progress: Progress| {};
        let reporter = Reporter::new(0, 0, &cancel, &mut sink);
        assert!(reporter.check().is_ok());
        assert!(matches!(
            reporter.fail(io::Error::other("disk")),
            ArchiveError::Io(_)
        ));
        cancel.cancel();
        assert!(reporter.check().is_err(), "cancel stops the copy loop");
        assert!(matches!(
            reporter.fail(io::Error::other("disk")),
            ArchiveError::Cancelled
        ));

        // A failure with no cancellation keeps its I/O cause.
        let fresh = CancelToken::new();
        let mut sink = |_progress: Progress| {};
        let reporter = Reporter::new(0, 0, &fresh, &mut sink);
        assert!(matches!(
            reporter.fail(io::Error::other("disk")),
            ArchiveError::Io(_)
        ));
    }
}
