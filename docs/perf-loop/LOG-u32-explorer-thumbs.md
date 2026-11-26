# u32-explorer-thumbs

Batch five over Explorer row thumbnails (decode off-thread, downscale on decode, LRU byte cap,
visible-row priority, drop on scroll-away). Review of the landed passes first, then the next
hotspots by composition share.

## Review of the landed thumbnail work (bugs/regressions fixed)

- Pass start dropped every queued job (`queue.retain(|_| false)`) even though `retain` exists to keep
  the jobs the next pass still shows. Every one-row scroll therefore cloned the key (path
  allocation) of each still-visible row, removed it from `pending`, and re-pushed it (second clone
  plus a second hash-set insert) in the same pass. `Thumbnails::start_pass_keeping` now keeps the jobs
  of rows that remain projected in view; only rows that scrolled away lose their job and pending
  mark. `start_pass` stays as the drop-all form used by the bench and the queue tests.
- `project_rows` computed the per-slot `in_view` twice conceptually (once implicitly when deciding
  what to request, once per row); the full pass now builds one `visible` mask, uses it for the keep
  predicate and for each row's visibility. Window position to slot mapping is explicit, because a
  listing at its first row is offset by one (`first` saturates at zero).
- Review of `Thumbnails::lookup`/`begin`/`complete` and `thumb_bound` rebinding found no stale-state
  or double-decode path: an in-queue job always carries its pending mark, a completed key always
  leaves it, and the upload is skipped while the bound id matches. No change needed there.

## New optimizations

- Header-first reads (`thumbs.rs`): `read_source` used to read a whole source (up to the 25 MiB cap)
  and only then probe the container for its canvas size. `thumbnail_from_file` now opens the file,
  reads a 64 KiB prefix, probes it, and returns before reading the rest when the header already
  proves the canvas is over `MAX_SOURCE_PIXELS` (or when the prefix is the whole file). A 25 MiB,
  25-megapixel photo no longer allocates and reads 25 MiB just to be refused. `thumbnail_from_bytes`
  keeps its public behaviour on top of the shared `decode_canvas`.
- One open per decode (`thumbs.rs`): the metadata check and the read now share one opened handle
  (`open_source` + `read_prefix`/`read_rest`) instead of `fs::metadata` followed by `fs::read`
  (which re-stats internally), and the prefix buffer is reused for the remainder. The pre-open
  `is_file` check stays, so a FIFO or device in a listing can still never block the decode worker.
- Wake batching (`workers.rs`): each decoded thumbnail wrote one wake byte and therefore ran one full
  UI projection. `thumb_worker` now announces at most once per 40 ms while more jobs are queued and
  always announces the last job of a burst, so a burst of small icons costs a few projections
  instead of one per (usually sub-millisecond) decode. Slow photo decodes are unaffected (their
  spacing already exceeds the interval), so thumbnails still pop in one by one.
- Lazy icon probing (`appicon.rs`): `resolve_icon` used to build the entire candidate list (up to 512
  themed paths, each `join` + `format!`) and only then stat them in order, even though a normal hit
  is the first `hicolor/48x48` entry. It now probes the same order one allocation at a time, so a
  `.desktop` thumbnail costs a handful of paths instead of ~1000 allocations plus a 512-entry vector.

## Expected composition shift

- Explorer decode worker: drops a whole-file read and the 25 MiB-class allocation for oversize
  sources, and removes one path resolution per decode.
- UI thread: full projections while a thumbnail burst lands fall from one per decode to at most
  25/s; each scroll step no longer clones/re-hashes the key of every visible image row.
- Explorer worker icon resolution: ~1000 fewer allocations per `.desktop` row before the first stat.
- Visuals unchanged: same cache, same LRU byte cap, same decode downsample, same type-icon fallback.

## Checks

- No cargo/git in this worker (shared checkout): changes are compile-reviewed only; the orchestrator
  runs `cargo test -p flamewm-explorer` and the workspace check.
- Added `pass_start_keeps_queued_jobs_that_stay_visible` to `thumbs/tests/queue.rs`.
