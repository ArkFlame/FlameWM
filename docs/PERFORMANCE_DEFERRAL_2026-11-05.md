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

Slider A/B: see the measurement section below (pending).

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
