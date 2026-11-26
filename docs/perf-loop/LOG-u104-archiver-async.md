# u104-archiver-async

Area: archiver and explorer copy/move jobs (progress cadence, cancellation, UI-thread I/O,
chunk sizing).

- `crates/flamewm-explorer/src/jobs.rs`: the copy loop used one fixed 256 KiB `const CHUNK` for
  every source and destination. It now asks the block device behind the copy: `rotational` from
  `/sys/dev/block/<maj>:<min>/queue/rotational` (partitions resolve through the symlink up to the
  disk that owns the queue), cached once per `dev_t` in a `OnceLock<Mutex<HashMap<u64, usize>>>`.
  A spinning disk gets 1 MiB requests, flash keeps 256 KiB, and an unknown or virtual device
  (tmpfs, overlay, network mount) gets 128 KiB. The chunk is the larger of the source and the
  destination parent's size, so a spinning destination still gets large writes and a spinning
  source still gets large reads. The per-copy buffer stays lazy: a directory-only copy allocates
  nothing, and a tree of many small files allocates once per top-level copy.
- `crates/flamewm-explorer/src/jobs.rs`: `copy_tree_with` reused `from.join(name)` plus
  `to.join(name)` per child, two fresh `PathBuf`s per entry. One `PathBuf` per side is now pushed
  and popped while it recurses, so a 100k-entry tree drops ~200k small allocations. Cancellation
  is still checked before every entry and between every read and write chunk.
- `crates/flamewm-archiver/src/progress.rs`: `Reporter` emitted on every byte/entry step. A fast
  archive (small totals or a page-cached source) could cross many 64 KiB steps inside one frame
  interval and wake the window with each one. Emissions now pass a 100 ms gate (`MIN_INTERVAL`,
  the same 10 per second as `flamewm-explorer::progress`); completion still bypasses the gate, and
  pending counters are kept so the next accepted emission reports the real totals. The bar still
  moves in 1% steps, just never more than ten times a second.
- `crates/flamewm-explorer/src/app/transfers.rs`: the conflict probe before the overwrite dialog
  stat'ed every source on the UI thread (`directory.join(name)` + `symlink_metadata`), which blocks
  on a slow or network destination. When the destination is the folder already on screen, the rows
  themselves answer the question, so a multi-item paste now costs zero per-source stats; hidden
  names (absent from the listing) still stat, and any other destination keeps the old path. The
  worker still re-checks every target before writing, so the dialog is only a prompt.

Expected composition shift: file copy syscalls per byte drop on rotating media (1 MiB vs 256 KiB
chunks) and small-file trees lose the per-child path allocations; archive and copy progress wake
the UI at most 10 times a second instead of once per step; the pre-dialog conflict probe no longer
runs a stat per selected item on the UI thread when pasting into the listed folder.

Not changed: per-file `sync_all()` on the staging file is kept because the staging-then-rename
contract is the crash-safety guarantee; removing it would weaken reliability for a copy-time win.

No `cargo`/`git` was run in this shared checkout. Compile correctness verified by reading and by
`rustfmt --check` parsing the edited files.
