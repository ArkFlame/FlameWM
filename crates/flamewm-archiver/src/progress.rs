//! Typed progress and cancellation shared by every long archive operation.
//!
//! `CancelToken` is checked before every entry and every read chunk. `Reporter` owns the running
//! totals and calls the caller's sink after each chunk, so memory stays bounded by the copy buffer.

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

/// Running totals plus cancel check for one operation.
pub struct Reporter<'a> {
    sink: &'a mut dyn FnMut(Progress),
    cancel: &'a CancelToken,
    state: Progress,
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
        (self.sink)(self.state);
    }

    pub fn finish_entry(&mut self) {
        self.state.entries_done += 1;
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

/// Copies `src` into `dst` through the reporter. Returns the bytes copied.
pub fn copy_tracked(
    src: &mut dyn Read,
    dst: &mut dyn Write,
    rep: &mut Reporter<'_>,
) -> Result<u64> {
    rep.check()?;
    let mut tracked = Tracked::new(src, rep);
    match io::copy(&mut tracked, dst) {
        Ok(n) => Ok(n),
        Err(err) => Err(tracked.rep.fail(err)),
    }
}
