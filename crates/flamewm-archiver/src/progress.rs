//! Typed progress and cancellation shared by every long archive operation.
//!
//! `CancelToken` is checked before every entry and every read chunk. `Reporter` owns the running
//! totals and feeds the caller's sink in coarse steps (about a hundred per operation), so memory
//! stays bounded by the copy buffer and the UI is not woken once per chunk.

use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Running totals plus cancel check for one operation.
///
/// The sink is fed at most ~100 times per operation: every 1/100th of the totals, plus the
/// completion snapshot. Each emission costs a channel send and a UI wake, so a per-chunk sink
/// would repaint the progress window thousands of times for one large file.
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
        if complete
            || self.pending_bytes >= self.byte_step
            || self.pending_entries >= self.entry_step
        {
            self.pending_bytes = 0;
            self.pending_entries = 0;
            (self.sink)(self.state);
        }
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
/// read/write syscall pairs.
const COPY_BUFFER: usize = 64 * 1024;

thread_local! {
    /// Reused copy buffer. An archive of many small files used to allocate and zero a 64 KiB
    /// buffer for every entry; one scratch buffer per worker thread removes that per-entry churn.
    static COPY_SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

/// Copies `src` into `dst` through the reporter. Returns the bytes copied.
pub fn copy_tracked(
    src: &mut dyn Read,
    dst: &mut dyn Write,
    rep: &mut Reporter<'_>,
) -> Result<u64> {
    rep.check()?;
    // A nested copy on the same thread (re-entrant writer) cannot share the scratch buffer, so it
    // falls back to a temporary one instead of panicking on an already-borrowed `RefCell`.
    match COPY_SCRATCH.try_with(|scratch| scratch.try_borrow_mut()) {
        Ok(Ok(mut buffer)) => copy_chunks(src, dst, rep, &mut buffer),
        _ => copy_chunks(src, dst, rep, &mut vec![0_u8; COPY_BUFFER]),
    }
}

fn copy_chunks(
    src: &mut dyn Read,
    dst: &mut dyn Write,
    rep: &mut Reporter<'_>,
    buffer: &mut Vec<u8>,
) -> Result<u64> {
    if buffer.len() < COPY_BUFFER {
        buffer.resize(COPY_BUFFER, 0);
    }
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
