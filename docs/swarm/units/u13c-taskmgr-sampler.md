# u13c — Task-manager / system-monitor 1 Hz sampler cost audit

**Status: UNBUILT.** No `git`, `cargo`, Python, build, test or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Every cost
below is a code-path budget read from current source, not a fresh runtime sample.

## Owned files (listed before any edit)

- `crates/flamewm-task-manager/src/model.rs` — read in full (`1-1116`) and re-read at the edit site
  (`98-115`) immediately before editing, as the brief requires; it already carried edits from earlier
  units.
- `crates/flamewm-task-manager/src/lib.rs` — read only for the worker/gate and display-sort audit;
  edited once for the sort guard below.
- `docs/swarm/units/u13c-taskmgr-sampler.md` — this audit record.

`sample_gate.rs` was read as evidence only and was not edited. No other file was touched.

## Audit readback — the four requested properties

| Property | Evidence in current source | Disposition |
| --- | --- | --- |
| Stops sampling while hidden / minimized | `lib.rs:1166-1194` owns one sampler thread on an on-demand worker; `wait_visible` (`sample_gate.rs:36-48`) parks on a condvar with **no timer** while obscured and resumes at once on uncover. `lib.rs:1182-1186` re-reads the mirrored `hidden` flag after the gate and skips the walk on the hide race. `lib.rs:1213-1225` interrupts the 1 s channel wait on `VisibilityChanged` and parks instead of sampling. `lib.rs:1319-1325` drives the gate from the visibility event, including minimize (obscured). The worker sleeps on `recv_timeout`, never a 100 ms polling timer. | Already satisfied. No cut: changing the gate or cadence would change visible update timing. |
| Reads only needed `/proc` files per process | `model.rs:86-89,617-626` — hot pids are re-read every tick, cold pids replay `cached.last` and pay no syscall; a cold pid is due once per 4 ticks. `model.rs:627` reads only `/proc/<pid>/stat` on a due tick (RSS from field 24 at `model.rs:308`, replacing `VmRSS`). `model.rs:650-681` reads the bounded `status` prefix (`STATUS_PREFIX = 2048`, `model.rs:349`) **only** on first sight, PID reuse or the 20-tick hard deadline, using one mtime probe instead of two content reads. `model.rs:686-706` reads `cmdline` only when the cached static fields are stale, and rebuilds the `Arc<str>` only when the sanitized text changed. | Already satisfied: no full `cmdline`/`status` read per pid per tick remains. No cut. |
| Reuses `String` and `Vec` buffers across ticks | `model.rs:49-79` declares `scratch: Vec<u8>`, `path: String`, `text: String`, `command_text: String`, `pids`, `cores`, `parents`, `ranked`, `hot`, all kept across samples. `model.rs:172-184,351-380,406-414` clear-and-fill those buffers instead of allocating; `model.rs:100-113,507-603` clear and reuse the ranking and cache buffers; `model.rs:309-315` reuses the cached name `Arc`. | Already satisfied. No cut: the buffers are already single-allocation per process table. |
| Skips sorting when order is unchanged | `lib.rs:236-239,306-311` already suppress the whole display rebuild (`hits_dirty`) on repaints for hover/click/scroll. Remaining gap: a new snapshot marks `hits_dirty` every tick, and both `lib.rs` display sort and `model.rs:100-113` hot-set ascending sort ran unconditionally even when the incoming pid order already satisfied the comparator. | **Two cuts applied** below; both are exact no-ops on already-ordered input. |

## Rows — one per change (file:line, cost, change, risk)

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-task-manager/src/model.rs:109-116` | Before: `hot.sort_unstable()` ran on every sample even when the process list arrived in ascending pid order and fit within `HOT_COUNT` (`<=64` tasks). That is an O(h log h) pass whose result is identical to the input. | **EDIT (UNBUILT):** guard the sort with `if !hot.windows(2).all(|pair| pair[0] < pair[1])`. The ascending invariant the forward cursor in `collect()` relies on is still enforced: any out-of-order `hot` is sorted exactly as before. With `> HOT_COUNT` processes the `select_nth_unstable_by` partition usually breaks the order early, so the check short-circuits after one comparison and the sort still runs. | Low. Sorting an already-ascending slice is the identity permutation, so the membership set and the forward-cursor behaviour are unchanged. `windows(2)` is empty for `len < 2`, so the guard skips the sort for 0/1 task as before. No field, allocation or new dependency. UNBUILT: no compile or test was run. |
| `crates/flamewm-task-manager/src/lib.rs:321-351` | Before: `hits_dirty` is set on every 1 Hz snapshot, so `ensure_hits` re-ran `sort_unstable_by` over every matching row even when the sampler's pid-ordered process list already satisfied the selected sort (`Sort::Pid` ascending, plus any snapshot whose keys are already in order). That is O(n log n) comparator calls per tick. | **EDIT (UNBUILT):** lift the comparator into a local `compare` closure and guard the sort with `if !self.hits.windows(2).all(|pair| compare(&pair[0], &pair[1]).is_le())`. The comparator body is byte-for-byte the former one (same columns, same `descending` flip, same pid tiebreak), so any list that is not already ordered is sorted exactly as before. `all()` short-circuits at the first inversion, so the common descending-CPU case pays one extra comparison, not a full scan. | Low. Sorting a slice that is already non-decreasing under the same total-order comparator cannot change it, so display order, row identity and selection are identical. `windows(2)` is empty for 0/1 hit, preserving the former no-sort case. The closure is `Fn`, so the borrow in `all()` ends before `sort_unstable_by(compare)` moves it; `hits` and `processes` are disjoint fields. UNBUILT: no compile or test was run. |

## Verification

- No `git`, `cargo`, Python, build, test or sleep-loop command was run, per the brief. Both edited
  files are **UNBUILT**; the orchestrator's `cargo test -p flamewm-task-manager` is still owed.
- Source readback only: `sample_gate.rs:36-48` parks without a timer while `visible == false`, and
  `lib.rs:1177-1186,1206-1225` re-checks the mirrored `hidden` flag and interrupts the deadline wait,
  so an obscured/minimized window pays no `/proc` walk.
- Source readback only: per-process reads are gated at `model.rs:617-626,650-681,686-706`; no
  unconditioned `status` or `cmdline` read remains on a task's due tick.
- The two guards preserve each sort's original comparator and can only skip work when the input is
  already ordered under that same comparator; no visible row, graph or counter value is recomputed
  differently.

PARTIAL — all four requested properties were audited: hidden/minimized sampling stop, minimal
per-process `/proc` reads, cross-tick buffer reuse and non-snapshot sort suppression were already
present in current source and were left untouched. The one remaining gap — unconditional sorts on a
pid-ordered snapshot — was closed with two safe, visually identical cuts (one in each edited file),
which is within the "at most 3" budget. Nothing was compiled, tested or committed here, so the slice
is UNBUILT and the compile/borrow and behavioural checks are owed by the orchestrator.
