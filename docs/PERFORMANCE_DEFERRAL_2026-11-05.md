# Performance deferral, 2026-11-05

Scope: work that cannot be removed is moved off the UI/event thread, after first paint, coalesced,
or done only when needed. No feature or quality cut.

## flamewm-desktop

### Task 1: settings snapshot repaint gate

Before: `SettingsSnapshotChanged` (`crates/flamewm-desktop/src/main.rs`, `register_workspace_signals`)
re-requested the wallpaper and set `settings_visual_dirty` unconditionally. Every slider motion
therefore re-projected the desktop (`refresh.rs` `refresh_settings_projection`).

After: the handler compares the previous and next snapshot with `effective_value`
(`flamewm_api::settings_groups`) for:

- `DESKTOP_WALLPAPER_KEYS` (`settings_values.rs`): gates `request_wallpaper`.
- `desktop_consumed_keys()`: the keys the desktop reads or projects, plus the switch and member keys of the
  groups it consumes (`accent`, `outline`, `surface`, `cursor`, `fonts`, `items`). Gates the dirty flag.

The snapshot is always stored, so reads stay current. Glass, panel, lock and screensaver keys are not
desktop-consumed (the glass palette reads only `appearance`), so glass slider motion no longer repaints the desktop.

Tests: `settings_values.rs` `settings_change_tests` (6 cases: revision-only change, glass slider motion,
wallpaper key, consumed key, group member behind off switch, switch toggle).

Checks (worktree `/run/media/linsaftw/SDD/wt-desktopdefer`, `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-desktopdefer`):

| command | exit |
|---|---|
| `cargo test -p flamewm-desktop --lib settings_change` | 0 (6 passed) |
| `cargo test -p flamewm-desktop` | 0 (174 passed) |
| `cargo check --workspace --all-targets` | 0 |
| `python3 -I tools/architecture_guard.py` | 0 |

### Measurement: glass-opacity slider drag, base vs new

Setup: release builds of `flamewm-wm-wayland`, `flamewm-desktop`, `flamewm-shell`, `flamewm-settings`;
base = `6d0caa60`, new = `30081712`. Nested session via `scripts/perf/settings_session.sh` (scratch copy with
a descendant-aware process filter), Surfaces page, "Custom liquid glass" on, drag on the Glass opacity track
(`--slider 588,768,588`), 60 motions per run. Three alternating base/new pairs. Desktop CPU was sampled from
`/proc/<pid>/stat` (utime+stime, 100 Hz) over the 28 s window. Raw logs: `/run/media/linsaftw/SDD/scratch-desktopdefer/ab4/`.

| metric (per run) | base run 1/2/3 | new run 1/2/3 | median base -> new |
|---|---|---|---|
| flamewm-desktop CPU | 0.54% / 0.57% / 0.57% | 0.04% / n.a. (pid restart) / 0.04% | 0.57% -> 0.04% (-93%) |
| flamewm-desktop ctx switches/s | 0.6 / 0.6 / 0.6 | 0.3 / n.a. / 0.1 | 0.6 -> 0.3 |
| whole session CPU (scenario total) | 100.8 / 101.5 / 100.6 | 101.4 / 107.0 / 102.2 | 100.8 -> 102.2 (+1.4%, noise) |

Reading: the desktop process win is real and clears the 3% same-process bar. Whole-session CPU is dominated by
`flamewm-wayland` (about 93 to 99% CPU during the drag), which the desktop does not own; no session-level
change is measurable there.

Evidence gaps:
- The earlier baseline of desktop 73.6% CPU / 649 wakes/s (`scratch-settings-cpu/base`) did not reproduce on
  current main: the base desktop used about 0.6% in this run. Cause not established (different build/harness state).
- `settings_scenarios.py` `snap()` reported `flamewm-desktop` as all zero although the process ran in the nested
  process group (confirmed with `ps`). Harness row unreliable for the desktop; numbers above come from `/proc` sampling.
- Animation-speed slider (`--slider 588,768,296`, Animations page) also gives desktop about 0.6% in base: not a
  meaningful desktop A/B. Not used.

### Task 2: deferral candidates in the desktop process

| candidate | finding | action |
|---|---|---|
| wallpaper decode/scale | Already on the `wallpaper_loader` worker thread (`wallpaper_loader.rs` `thread::Builder`), previous wallpaper shown until ready. | none needed |
| icon / thumbnail loads | Already on the icon worker (`icons_pending` flag, `refresh_icon_theme` polls on UI tick). | none needed |
| sticky persist | Already debounced (`PERSIST_DEBOUNCE`, `arm_persist_after`, `take_due`, retry with capped backoff) and wake-deadline driven. | none needed |
| file-watcher bursts | inotify drains then sets one `dirty` flag per burst (`register_inotify`), with settle timing. Already coalesced. | none needed |
| initial scan after first paint | Startup runs `rescan_with_grids` twice when `desktop-layout.state` exists. Collapsing to one scan changes placement: the first scan writes positions for unstored entries that the second scan keeps (`model.rs` rescan write-back). Semantics change, not kept. | evaluated, not changed |
| settings wallpaper request | Gated by Task 1 (`wallpaper_settings_changed`). | done, `30081712` |

Startup: `scripts/perf/startup.sh` does not list the desktop (apps: explorer, terminal, imageviewer, archiver,
settings, task-manager, finder). Desktop time-to-first-paint was not measured. Not claimed as a win.

## Checks (commit `30081712`, worktree `/run/media/linsaftw/SDD/wt-desktopdefer`)

| command | exit |
|---|---|
| `cargo test -p flamewm-desktop --lib settings_change` | 0 (6 passed) |
| `cargo test -p flamewm-desktop` | 0 (174 passed) |
| `cargo check --workspace --all-targets` | 0 |
| `python3 -I tools/architecture_guard.py` | 0 (77 ratified violations, existing) |
| `cargo build --release -p flamewm-wm-wayland -p flamewm-desktop -p flamewm-shell -p flamewm-settings` (new and base) | 0 and 0 |

Risks: the glass-drag win is measured on one page with 60 motions; wallpaper and icon paths were not run as
runtime tests. Harness snap() defect above is unfixed.

## Explorer: directory listing (2026-11-05)

Scope: `flamewm-explorer` local directory listing. Not covered: imageviewer, finder, archiver-app
(see "Audited, not changed").

### Root causes

1. A local listing reached the UI only after the worker had stat'ed and sorted every row
   (`list_directory`, one `Scanned` message). Time-to-first-rows equals the full directory read.
2. The UI re-sorted every received listing (`app/input.rs`, `Scanned` handler) even when the worker
   had already produced the same order (default Name ascending). That is a full O(n log n) pass on the
   UI thread per listing.

### Change

- `lib.rs`: `list_directory_streamed` reads in readdir order and emits batches (`FIRST_BATCH` = 256,
  then `LATER_BATCH` = 2048); `emit` returning false stops the read. `list_directory` is now the
  streamed read plus `sort_listing`. One comparator (`listing_order`) serves both.
- `workers.rs` `scan_worker`: sorts each batch and sends `WorkerResult::ScanBatch`; a newer queued scan
  abandons the current one between batches (no stale batch is sent); then sends the complete sorted
  listing as `Scanned`.
- `lib.rs` `Explorer::append_scan` and `app/input.rs`: batches apply only while the tab is loading a
  fresh listing. Refresh (`DirChanged`) batches are ignored, so the old rows stay until the final replace.
- `view.rs`: `column_order` shared by `sort_entries` and new `is_sorted_by`; the UI re-sorts only when
  `is_sorted_by` is false.
- Known trade-off: the worker keeps a clone of all streamed rows to build the final sorted listing
  (extra RAM for the listing duration, about one more copy of the entries). The final list is a move to the UI.
- Known trade-off: partial rows show readdir order within each sorted batch, then reorder when the
  listing completes.

### Measurements

`scripts/perf/explorer_dir.sh` (release, 20 000-file fixture, 7 iterations, median ms).
Host load was not constant (load1 22.4 in run 1, 26.4 in run 2); compare rows within the same run.

| Metric | Before (run 1) | After (run 2) |
| --- | --- | --- |
| Time to first rows, bulk path (old: full listing) | 84.456 | 161.226 (same bulk code path, slower host) |
| Time to first rows, streamed | n/a | 1.353 |
| Streamed complete (batches + final sort) | n/a | 123.865 (bulk in same run: 161.226) |
| UI thread per listing, old re-sort on sorted input | 1.291 | 1.642 (same run as check below) |
| UI thread per listing, new order check | n/a | 1.085 |

Same-run ratios (run 2): first rows 161.2 / 1.35 = about 119x sooner. UI per-listing cost 1.64 / 1.09 =
about 34% less, roughly 0.6 ms saved per listing; this is a small stall win, not a large one.

Not measured: Xvfb frame timing and event-loop latency of the live app. No live-app A/B was run,
so no UI responsiveness claim is made beyond the per-listing numbers above.

### Verification

- `cargo test -p flamewm-explorer` exit=0 (209 passed, 0 failed; includes 4 new tests in
  `src/tests/listing_stream_tests.rs`: streamed order equals bulk for Name/Size/Modified with and
  without hidden files; small and empty directories; a superseded stream stops after one batch;
  UI order check agrees with `sort_entries`).
- `cargo check --workspace --all-targets` exit=0. One pre-existing warning (unused import
  `DEFAULT_ACCENT_RGB`, `crates/flamewm-shell/src/settings_projection.rs:3`), not in this change.
- `python3 -I tools/architecture_guard.py` exit=0.
- `python3 -I -m unittest discover -s tools -p 'test_architecture_guard.py' -q` exit=0 (6 tests OK).
- Logs: `.agents/runtime/perf-defer/` (bench-before.txt, bench-after.txt, test-explorer.log, check2.log, guard.log).

### Audited, not changed

- Thumbnails (`thumbs.rs`, `workers.rs` `thumb_worker`): decode already runs on one worker thread, off the
  UI. Missing: visible-row priority, cancellation on scroll/navigate, and coalescing one repaint per frame
  (each `Thumbnail` result wakes the UI). Deferred: needs a render-side change; not measured.
- File-watch bursts (`refresh.rs`): already debounced by `should_fire` in the watch thread. No change.
- MIME lookups: `mime.rs` discovery runs on a spawned thread (`menus.rs`); no per-row UI lookup found. No change.
- Archive listing/extraction progress, imageviewer decode, clipboard/DnD, and finder: not audited in this
  pass. Open.

### Open

- Live-app frame timing under Xvfb for explorer scroll and navigation (needs a separate runtime pass).
- Thumbnail priority, cancellation, and per-frame coalescing (see above).

## Explorer: thumbnail scheduling (2026-11-05, second pass)

Scope: `flamewm-explorer` row thumbnails only (`thumbs.rs` queue, `workers.rs` `thumb_worker`,
`app/projection.rs` `project_rows` / `row_thumbnail`). Imageviewer, archiver-app, finder and
the live-app frame timing were not changed or measured in this pass (see "Not done").

### Root causes

1. The UI sent every cache miss into one FIFO `mpsc` channel and never removed anything. A fast scroll
   queued every row passed, so rows on screen waited behind decodes of rows already scrolled away.
2. The request policy asked for all window slots, including the two slots drawn off-view.

### Change

- `thumbs.rs` `ThumbQueue`: `Mutex<VecDeque<ThumbKey>>` + `Condvar`. Worker `pop` blocks, oldest first.
  `retain` drops queued jobs.
- `thumbs.rs` `Thumbnails::start_pass(&queue)`: called at the start of each full projection. Drops every
  queued (not yet decoding) job and clears its `pending` mark, so the pass re-requests what is on screen.
  An in-flight decode is not cancelled (it finishes and is cached).
- `projection.rs`: `project_rows(full=true)` calls `start_pass`. `row_thumbnail(entry, in_view)` queues a
  decode only for rows in view. Glide frames (`full=false`) never request, as before.
- Tried and reverted: coalescing completion wakes with an armed flag. Bench showed wakes equal decodes
  (each decode finished after the UI had drained), so no measured win; removed per the keep rule.

### Measurements

`scripts/perf/explorer_thumbs.sh` (same process, release, 300 PNG 640x480 fixture, window 30, 3 iterations,
median). `old` reproduces the pre-change policy (FIFO, all window slots, no pass cancel). `new` is this change.
Host load1 was 18.5 (run 1) and 23.2 (run 2): compare rows within one run only.

| Scenario | Metric | old | new | Result |
|---|---|---|---|---|
| scroll: 120 passes, window moves 2 rows/pass | settle ms (last window cached) | 1952.2 (run 1), 3000.2 (run 2) | 383.0 (run 1), 384.2 (run 2) | about 5x to 8x sooner |
| scroll | decodes performed | 268 (run 1), 268 (run 2) | 72 (run 1), 70 (run 2) | about 74% fewer decodes |
| steady: one pass, window cached | settle ms | 377.7 (run 1), 391.5 (run 2) | 404.7 (run 1), 471.9 (run 2) | UNRESOLVED: new is 7% and 21% slower; same decode count (30) |

Steady-view code path is the same work in both modes (one start_pass on an empty queue, same 30 decodes).
The slowdown is not explained by the code and is not confirmed beyond two noisy runs; treat as an open
regression risk to re-measure on a quiet host. Max per-frame stall on the UI thread was not measured
(decode already ran off-thread before this change).

### Verification

- `cargo test -p flamewm-explorer` exit=0 (213 passed, 0 failed). Includes 4 new tests in
  `src/thumbs/tests/queue.rs`: FIFO order; pass start drops stale jobs and frees their keys for re-request;
  an in-flight decode is not re-requested or dropped; `retain` keeps order.
- `cargo check --workspace --all-targets` exit=0. Only warning: pre-existing unused import in
  `crates/flamewm-shell/src/settings_projection.rs:3`.
- `python3 -I tools/architecture_guard.py` exit=0 (77 ratified violations, unchanged).
- Logs: `/run/media/linsaftw/SDD/swarm-logs/deferral-test2.log`, `deferral-bench2.log`,
  `deferral-check-ws.log`, `deferral-guard.log`.

### Not done (honest status)

- Imageviewer (decode off UI thread, placeholder, idle prefetch): not audited or changed. No measurement.
- Archiver listing/extraction and progress throttling: not audited or changed. No measurement.
- Finder ~1 wake/s: `scripts/perf/app_wakes.sh` was not run in this pass. No measurement.
- Live-app frame timing (Xvfb scroll/navigate): not run.
- Steady-view slowdown above: unresolved.

## Explorer steady-view regression: resolved as host noise (2026-11-05, third pass)

Re-measured the second pass's open item. Root-cause review of the steady path: `start_pass` on an
empty queue is one mutex lock and an empty `retain` (no allocation); the in-view row lookups are
identical to the old policy; the decode count is 30 in both. No lock, wake, or ordering difference
exists on that path. The earlier 405/472 ms vs 378/392 ms gap came from one fixed run order on a
loaded host.

Fix to the bench, not the queue: `examples/thumb_bench.rs` now alternates policy order per iteration
(old-new, new-old, ...), so host drift cannot favour the policy that runs first.

### Measurements (same process, release, 300 PNG fixture, window 30, median)

Build: `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-coder-perf cargo build --release -p flamewm-explorer --example thumb_bench`
exit=0. Run: `thumb_bench /run/media/linsaftw/SDD/perf-fixtures/thumb-bench 5` (run 1 and 2), then 7 iterations x3.
Bench exit codes: runs 1 and 2 not captured as exit codes (`| tail` pipe). The 7-iteration runs printed
complete results; the `PIPESTATUS` capture in that shell was empty, so the bench's own exit is unverified
for those three runs.

| Scenario | Metric | old | new | Result |
|---|---|---|---|---|
| steady, run A (5 it, old-first order) | settle ms | 273.5 | 244.7 | new 11% faster (noise-level) |
| steady, run B (5 it, old-first order) | settle ms | 240.1 | 237.4 | equal |
| steady, run 1 (7 it, alternating) | settle ms | 171.3 | 170.2 | equal |
| steady, run 2 (7 it, alternating) | settle ms | 191.7 | 191.8 | equal |
| steady, run 3 (7 it, alternating) | settle ms | 163.1 | 163.4 | equal |
| scroll, run A | settle ms | 2246.6 | 296.3 | about 7.6x sooner |
| scroll, run B | settle ms | 1873.2 | 261.2 | about 7.2x sooner |
| scroll, run 1 | settle ms | 1048.4 | 183.5 | about 5.7x sooner |
| scroll, run 2 | settle ms | 1002.8 | 171.4 | about 5.9x sooner |
| scroll, run 3 | settle ms | 1104.2 | 175.7 | about 6.3x sooner |
| scroll decodes | count | 268 | 86 to 120 | about 55 to 68% fewer |

Load1 during runs: 20.5, 18.5, 13.4, 10.9, 8.2. Steady is equal within noise at every load level tested.
The scroll win holds at every load level. Keep the change; no revert.

Not measured: the queue lock and worker wake cost were not timed in isolation (not needed; the
steady-path code is identical). Live-app frame timing was not run.

## Imageviewer, archiver-app, finder (2026-11-05, third pass)

### imageviewer: audited, no change

- Decode already runs on `decode::run` (one worker thread). The worker drains `Load` before `View`
  before `Prefetch`, so prefetch runs only when no load or view is queued (`decode.rs` `run`). Idle-only prefetch is in place.
- A load keeps the previous frame on screen (`App::load` does not clear `PHOTO`) and shows "Loading…".
- Remaining gap: one prefetch decode in flight is not preempted, so a `Load` arriving mid-prefetch waits up to one
  full decode. Fixing that needs a cancellable decoder. Not measured (needs large fixtures and a live window). No change.

### archiver-app: listing off the UI thread (changed)

- Before: `window.rs` `App::open` ran `flamewm_archiver::list` on the UI thread before the first paint
  (`run` called `app.open` synchronously). A large archive held the whole window.
- After: `open` spawns `flamewm-archiver-list`, sets status "Reading archive", and the first paint shows at once. The
  thread sends `Reply::Listed { archive, result }` and writes the wake byte. `reply` applies it only when
  the archive is still the window's target (stale listings are dropped).
- Progress throttling: not changed. Progress replies already coalesce per drain (one paint per wake, the wake byte
  is gated by the `wake_pending` flag). No measured win, so no change (keep rule).
- Tests: `cargo test -p flamewm-archiver-app` exit=0, 10 passed, including
  `open_lists_on_a_worker_and_replies` (real zip), `listed_reply_applies_entries_for_the_open_archive`,
  `stale_listing_is_ignored`.
- Not measured: UI-thread stall before vs after (no large archive fixture was generated).

### finder: audited, not measured

- Placeholder tick thread writes one byte every `PLACEHOLDER_INTERVAL` = 5 s (`flamewm-finder/src/lib.rs`). That is
  0.2 wakes/s, not the reported ~1/s, so the extra wakes come from another source.
- `scripts/perf/app_wakes.sh` was NOT run: no live finder window was started (needs an isolated X display).
  Source-level change: none. Open item.

## Checks (2026-11-05, worktree `/run/media/linsaftw/SDD/wt-coder-perf`)

| command | exit |
|---|---|
| `cargo test -p flamewm-archiver-app` | 0 (10 passed) |
| `cargo build --release -p flamewm-explorer --example thumb_bench` | 0 |
| `python3 -I tools/architecture_guard.py` | 0 (77 ratified violations, unchanged) |
| `cargo check --workspace --all-targets` | 0 |

Logs: `/run/media/linsaftw/SDD/swarm-logs/coder-archiver-test.log`, `coder-guard.log`, `coder-check-ws.log`.
