# u13 — Task-manager 1 Hz `/proc` sampler cut (hotlist rank 5)

**Status: UNBUILT.** The edits were not compiled, tested, committed or pushed: the worker contract
forbids `git`, `cargo`, builds and tests. The orchestrator must run
`cargo test -p flamewm-task-manager` after the batch lands, then flip this marker to verified and
record the scoped sample in `docs/perf-loop/LOG.md`.

Owner evidence: `crates/flamewm-task-manager/src/model.rs` owns procfs sampling,
`crates/flamewm-task-manager/src/sample_gate.rs` owns the visibility park, and
`crates/flamewm-task-manager/src/lib.rs` owns the sampler worker and its 1 s cadence. No file
outside those three was touched. No new dependency, no unsafe code, no new thread.

Measured procfs fact behind the change: `/proc/1/stat` mtime stayed `1795792037` across five
reads while `utime` advanced; `btime 1795180109` plus `starttime 61192362 / 100` reproduces that
mtime. Procfs therefore stamps the stat inode at task creation: mtime plus the parsed `starttime`
is an *identity* token (reuse / new process), not a per-tick change token. CPU ticks change without
moving mtime, so hot and due cold pids must still read `stat`; the skipped work is the per-pid
static re-read that used to run on every refresh boundary for an unchanged process.

## Findings (row by row)

| # | File:line | Cost | Change | Risk |
| --- | --- | --- | --- | --- |
| 1 | `crates/flamewm-task-manager/src/model.rs:140`, `:383` | New state: one `Option<(u64, u64)>` per cached pid (~16 B x 581) | Added `Cached::stat_mtime` and `stat_mtime()`; the mtime is captured from the reused stat path only on first sight or at a static-refresh boundary | `fs::metadata` fails for a vanished pid; the entry then looks changed and takes the refresh path, which already tolerates unreadable procfs |
| 2 | `crates/flamewm-task-manager/src/model.rs:648-681` | Before: every refresh boundary opened, read and closed `/proc/<pid>/status` **and** `/proc/<pid>/cmdline` for an unchanged pid (~2 file reads x ~39 boundaries/s at 581 processes). After: one `stat` syscall at first sight or the 20-tick deadline (never per tick); both file reads are skipped when mtime, starttime and `comm` match | Identity-gated static refresh: `status` only on reuse / first sight / hard deadline; `cmdline` on rename or hard deadline; unchanged pids pay no file read | UID or argv that changes without a new inode, a rename or the 20-tick deadline can lag. The deadline (`STATIC_REFRESH_TICKS * 4`) keeps the lag bounded; correctness under `setuid`/same-comm `exec` needs a live check, which is UNBUILT here |
| 3 | `crates/flamewm-task-manager/src/model.rs:626` | `stat` is the only per-pid content read on a due tick | Already present: RSS and VSIZE come from stat fields 24/23, so no second `status` read for memory; the single `read_proc` reuses `scratch` | None; preserved, not reworked |
| 4 | `crates/flamewm-task-manager/src/model.rs:55-62`, `:371-382` | Per-pid `String` / `Vec` allocation on a 1 Hz walk | Already present: `scratch`, `path`, `text` and `command_text` are reused across samples; `proc_path` writes into `path` with `write!` and no `format!` | None; preserved. The new mtime probe reuses the same `path`, so it adds no allocation |
| 5 | `crates/flamewm-task-manager/src/model.rs:82-89`, `:616-625` | Full 581-pid walk per second | Already present: 64-pid hot set every tick, rotating `SLOW_TICKS = 4` for the rest, zero-syscall replay on off ticks. Kept because CPU ticks change without an mtime change | Cold pids remain up to four ticks stale; unchanged from the prior design |
| 6 | `crates/flamewm-task-manager/src/lib.rs:1182-1186` | One spurious full `/proc` walk when a hide lands between gate check and sample | Re-read the mirrored `worker_hidden` flag immediately before `collect()`; a hidden window `continue`s back to the timerless `wait_visible` park | A stale `true` flag delays a sample by one wake; the uncover message still releases the gate and samples now |
| 7 | `crates/flamewm-task-manager/src/lib.rs:1197`, `:1216-1224`, `:1314-1324` | Sampling while minimized or a burst of samples from queued visibility messages | Existing gate and cadence kept and verified by inspection: the window parks with no timer while `obscured`, a hide/uncover interrupts the 1 s `recv_timeout`, and a stale show message does not resample. No extra timer or poll was added | Minimized coverage depends on `UiControllerEvent::Visibility { obscured }`; render-x11 emits only `VisibilityNotify`, so a WM that unmaps without that signal is outside this unit's ownership and needs a separate route |
| 8 | `crates/flamewm-task-manager/src/model.rs:247-326` | Per-read heap from field-splitting `stat` | Already present: one byte pass, no `Vec<&str>` of fifty fields; reused `comm` bytes bump an `Arc` instead of building strings | None; preserved |

## Verification owed by the orchestrator

- `cargo test -p flamewm-task-manager` (the parser and gate tests live in `model.rs` and
  `sample_gate.rs`).
- One-second scoped sample with the task manager visible and hidden, comparing syscalls/s and the
  table pixels before/after; record in `docs/perf-loop/LOG.md` and re-rank row 5 only if the cut is
  measured and the pixels match.

PARTIAL: the rank-5 cut is implemented as an identity-gated static refresh plus a pre-collect
hidden check, and the four already-landed mechanisms are documented with their ownership and
risks. The requested "skip unchanged pids" is bounded to the per-pid *static* reads because procfs
mtime cannot signal CPU-tick changes (measured above); skipping the `stat` read itself would freeze
CPU and memory values and break the no-visual-change criterion. No build, test or pixel comparison
was run under this contract, so the result is UNBUILT and the visual risk in row 2 remains open.
