# Unit u55-taskmgr-clone — log

Area: task manager (`crates/flamewm-task-manager/src/**`). Owned paths only; no `cargo`, no `git`
(shared checkout — the orchestrator builds, tests and commits).

## Review of the earlier task-manager perf slices

`git log -p` was not available in this shared checkout, so the review compared the current tree
against the recorded intent in `LOG-u03-taskmgr.md`, `LOG-u26-taskmgr-ui.md`,
`LOG-u33-taskmgr2.md` and `LOG-u44-taskmgr3.md`. Checked invariants:

- hot-set replay (u26 bug): `cached.last` is stored for **every** pid, including hot ones, so a
  pid leaving the hot set can never replay a sample from before it became hot. Still correct after
  this unit's changes.
- CPU delta: `known = cached.seen != 0 && cached.start == task.start`, span is
  `total - cached.read_total`; a PID reused with a new start time never inherits the old counter.
  Unchanged.
- rotating schedule: `slow_due(pid, tick) = tick % SLOW_TICKS == pid % SLOW_TICKS`, so every cold
  pid is due exactly once per `SLOW_TICKS` ticks while hot pids are read every tick. Unchanged.
- search folding (`contains_folded`, `pid_contains`) and the cached `hits` order: behaviour
  unchanged, incl. the ASCII fast path and the non-ASCII fallback.
- graph ping-pong rasters: `Arc::make_mut` still writes only the buffer the document does not hold.

No functional regression found; no dead code found in the owned paths (every helper is reachable
from `run`, `collect` or a test; the previous unit already removed the u33 leftovers).

## Changes (ranked by composition share)

1. **Hot-set probe: `HashSet<u32>` -> sorted `Vec<u32>` + merge cursor.** The sampler asked
   `hot.contains(&pid)` for every pid of the process table (hundreds of SipHash probes per
   sample). `hot_pids_into` now stores the pids ascending and `collect` walks both ascending lists
   with one forward index, passing the resulting `bool` into `sample_task`. Expected shift: the
   per-sample hot test costs O(pids + hot) integer compares and hashes nothing.

2. **One cache probe per due task.** A due read used `tasks.get(&pid)` for the name-reuse `Arc`,
   then `tasks.entry(pid)` for the counters/static fields (plus the hot probe). It now takes the
   entry once and reuses `&cached.name` from it. Expected shift: two map lookups per due task per
   tick (~150/s while visible) become one.

3. **Bounded `/proc/<pid>/status` read.** The sampler only needs `Uid:` (line ~9 of the file) but
   read the whole status file, making the kernel format VM, signal and cgroup counters for every
   stale refresh (~80/s). `read_proc_prefix` stops at `STATUS_PREFIX = 2048` bytes, which is an
   order of magnitude past `Uid:`. `stat` and `cmdline` still read to EOF (`read_proc` forwards
   `usize::MAX`). Expected shift: less kernel seq_file work per refresh, identical UID.

4. **Reused text buffer for the whole-file procfs reads.** `/proc/stat` (a few KiB with 32 cores),
   `/proc/meminfo`, `/proc/loadavg`, `/proc/uptime` and `/proc/net/dev` went through
   `fs::read_to_string`, i.e. five fresh `String`s per sample, and a failed read allocated
   `unwrap_or_default()` again. `read_text` reuses one `Sampler::text` buffer (capacity kept), and
   `load_cpu` / `memory` / `uptime_text` now take the text. Expected shift: ~5 allocations (and
   several KiB of allocator churn) per sample removed. This closes the first item of the u33
   "next candidates" list.

5. **Name sort without per-process `String`s.** `ensure_hits` lowercased every process name into a
   `Vec<String>` before sorting by name (one heap allocation per process, per rebuild, and a
   rebuild happens after every sample). `folded_cmp` orders on ASCII-lowercased bytes with no
   allocation and falls back to the exact former `to_lowercase()` comparison when either side is
   non-ASCII (both paths agree for ASCII pairs, so the order stays a total order). The folded
   query is also skipped entirely for an empty search box. Expected shift: removes the last
   per-row allocation from the model->view projection.

6. **Hover repaint: reused id buffer.** The pointer-motion path `format!`ed `row-{slot}` for the
   two rows that changed highlight; it now writes into one reused `String`.

Composition note: sampling still dominates the task manager's own CPU (procfs reads), so items 1,
3 and 4 are the ones that move the needle; 2, 5 and 6 remove per-second allocation from the
sampler and the projection.

## Tests

- `hot_set_is_the_highest_cpu_pids_and_bounded` updated to the `Vec` hot set and extended with an
  ascending-order assertion.
- New `name_order_folds_case_without_per_process_strings` covers the folded comparator (ASCII
  equality/ordering, empty string, non-ASCII equality).
- Existing schedule, replay and parser tests are unchanged and still apply (`SLOW_TICKS`,
  `parse_stat_with_name`, `assign_depths`, `parse_uid`, `parse_net_dev`).

No `cargo`/`git` executed (shared checkout); the changes are compile-reviewed by reading.
Commit: left to the orchestrator batch build.
