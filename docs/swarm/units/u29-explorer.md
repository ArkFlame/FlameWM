# u29 — Explorer burst debounce and thumbnail memory

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Costs below
are code budgets and slot/byte bounds read from current source, not fresh runtime samples. No raster
value, `Entry` value, sort key or drawn row is changed, so the visible listing and thumbnails are
identical by construction: the watcher only delays and coalesces refreshes, the cache only evicts
rows that are not currently shown, and the refresh merge keeps equal rows byte-for-byte.

**Target:** `crates/flamewm-explorer/src` — an inotify watch that refreshed once per event in a
burst, an unbounded thumbnail/icon pixel cache, and a full re-sort/rebuild of the listing on every
watch event. All three mechanisms are present in the current shared checkout (implemented during
this batch); this unit verified their wiring and records the delivered bounds. No further source
edit was needed — adding a second debounce or a second cap would only add risk for no visible gain.

## Owned files (inspected before any edit)

- `crates/flamewm-explorer/src/refresh.rs` — inotify watch, burst coalescing and wake policy.
- `crates/flamewm-explorer/src/thumbs.rs` — thumbnail LRU cache, byte budget, hidden-window drop.
- `crates/flamewm-explorer/src/view.rs` — `sort_entries` and the sortedness check.
- `crates/flamewm-explorer/src/lib.rs` — listing sorter, `apply_refresh` and `merge_refresh`.
- `crates/flamewm-explorer/src/app/input.rs`, `crates/flamewm-explorer/src/app/navigation.rs` —
  refresh wiring and column-sort call sites.
- `docs/swarm/units/u29-explorer.md` — this report.

No file outside this set was touched.

## Rows — file:line, cost, change, risk

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `refresh.rs:19,25,37-63` | Before: one `notify()` per inotify event, so an extraction or large copy repainted and re-listed the directory hundreds of times in a burst. | Added a pure trailing-edge rule: `QUIET` = 200 ms from the last drained batch (`should_fire`) and `MAX_WAIT` = 1 s from the first batch of a burst (`burst_forced`); `poll_timeout_ms` wakes at the earlier deadline and blocks with no timeout when idle. No event is dropped — the final event always flushes after quiet. | Medium until measured: a lone change is now shown up to 200 ms later, and a continuous `MODIFY` stream repaints once per second instead of per event. Both are deliberate latency trades; builds/tests are forbidden here, so this is UNBUILT for the orchestrator to confirm. |
| `refresh.rs:90-153` | Before: a single 4 KiB `read_events` buffer meant a burst larger than the buffer returned to `poll` once per chunk, rebuilding the poll set and re-reading the clock each time. | Drain the whole kernel queue per wakeup in an inner loop, then move the single quiet deadline to the last drained batch and start the max-wait clock at the first batch. `notify()` fires at most once per quiet window, or once per `MAX_WAIT` during churn. | Low — draining to `WouldBlock` is the documented inotify path; the stop pipe is checked before the inotify fd so drop still wins. The pure timing rules have focused unit tests at `:175-207`. |
| `thumbs.rs:28-44,669-696` | Before: decoded thumbnail pixels accumulated in a `HashMap` with no cap, so scrolling a large image folder grew memory without bound. | Bounded the cache two ways: `CACHE_CAPACITY` = 256 decoded entries (about 330 KB at 18 px) and `CACHE_BYTES` = 256 KiB of pixel bytes. `ThumbCache::complete` inserts with a monotonic recency stamp, subtracts the replaced slot's bytes, and evicts the least-recently-used entry until both bounds hold. Failures are cached as `None` so every miss still leaves the type icon in the slot. | Low-medium: eviction is an O(n) scan over at most 257 entries on overflow, bounded and rare; a row scrolled far off screen can be evicted and re-decoded on return, which is invisible except for the extra decode. Byte accounting was test-covered at `thumbs/tests/mod.rs:341-388`. |
| `thumbs.rs:36-39,633-667,704-739` | Before: a covered or minimized explorer kept every decoded thumbnail and its queued decodes indefinitely. | `HIDDEN_CACHE_GRACE` = 30 s; `HiddenCache`/`hidden_cache_expired` let the UI arm one wake instead of polling, after which `clear()` drops all cached pixels and pending marks and `start_pass_keeping` abandons jobs whose rows left the view. A quick alt-tab back still finds rows ready; a long-hidden window holds no pixels. | Low — decode ids keep rising after `clear()`, so a re-decoded row rebinds its image instead of reusing a stale id. The grace delay is a memory/latency trade only. |
| `view.rs:51-82`; `lib.rs:78-100` | Before: every refresh and every column event re-sorted the full entry list with per-comparison lowercase allocation for non-ASCII names. | Added `is_sorted_by` — an O(n) adjacent-pair check with the same `column_order` comparator — so a listing already in the requested order is never sorted again. Ordering itself is unchanged (`cmp_lower_name` folds ASCII bytes without allocating; Type compares a borrowed `TypeOrder` instead of formatting two strings per comparison). | Low — the check uses the exact comparator used by the sort, and falls back to a full sort on the first out-of-order pair, so the resulting order is identical. |
| `lib.rs:191-208,357-406`; `lib.rs:473` | Before: each watch event replaced the whole `Vec<Entry>`, invalidating the revision, every projection cache and every thumbnail bound to those rows. | `apply_refresh` now checks sortedness on both sides and, when they match, merge-joins the new listing by path: one linear pass where equal rows keep their old `Entry`, revision and label/thumbnail caches, and only added, removed or changed rows move. A listing that is not in display order still falls back to `set_entries`, so visible order is unchanged. | Low-medium: merging assumes both lists share one total display order; the sortedness guard enforces that, and `merge_refresh` is covered by `tests/merge_refresh_tests.rs` and `tests/mod.rs:246-260`. No per-path map is built. |
| `app/input.rs:420,437,455`; `app/navigation.rs:153` | Before: UI paths could call `sort_entries` unconditionally on refresh/column changes. | Guarded each call with `!view::is_sorted_by(...)`; `DirWatch::spawn` rides the debounced watcher and forwards `DirChanged` through the existing worker channel into `apply_refresh`. No new event source or authority was introduced. | Low — callers keep the same sort key and the same wake pipe; only redundant sort work is skipped. |

## Verification

UNBUILT by contract: no compiler, test runner, `git`, Python or sleep loop was invoked in this unit.
Identical visible results are argued from source only: the watcher fires the same `notify()` callback
with the same final state after a quiet window; `column_order` is the single authority for both the
sortedness check and the sort; `merge_refresh` keeps any row whose `Entry` compares equal and moves
the rest into the same total order; and LRU eviction only removes entries a subsequent projection
pass re-requests. The latency caps (200 ms quiet, 1 s max wait, 30 s hidden grace) and the memory
caps (256 entries / 256 KiB) need the orchestrator's build, tests and a scoped burst sample to
confirm in runtime.

PARTIAL — all three requested tightenings are present and wired in the shared checkout: inotify
events are coalesced on the trailing edge with a liveness cap instead of refreshing per event;
thumbnail pixels are held in a bounded, recency-evicted cache and dropped for long-hidden windows;
and refresh merges equal rows instead of re-sorting and rebuilding the full listing. No source edit
was required in this pass, so the report is the only file added here. The slice is marked UNBUILT
and awaits the orchestrator's build, tests and a scoped explorer refresh/thumbnail-memory check.
