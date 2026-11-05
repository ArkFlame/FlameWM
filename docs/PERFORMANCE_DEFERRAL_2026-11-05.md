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
