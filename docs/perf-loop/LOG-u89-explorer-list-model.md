# u89 explorer list model: diff refresh, off-thread sort, stable row ids

Batch thirteen. Owned paths: `crates/flamewm-explorer/src/**`. No `cargo`, no `git` (shared
checkout); this slice is reasoned by call composition and the orchestrator builds it.

## Composition reasoning

A watcher event (`DirChanged` from `refresh::DirWatch`) ran this path on the UI thread:

1. `request_listing` bumped the generation with `next_generation`, which marked the tab loading.
2. The scan worker streamed sorted batches; `append_scan` accepted them because the tab was
   loading, so the visible pane was cleared and refilled batch by batch.
3. `Scanned` then called `apply_listing`, which installed the whole listing again with
   `set_entries`: one `Vec` of `Entry` dropped (path and name deallocations for every row), a new
   `Vec` kept, a revision bump, and — because the worker had only sorted by the legacy `SortBy`,
   not by the header's `ColumnSort` — an O(n log n) re-sort on the UI thread whenever the user
   sorted by Size/Modified/Type or descending.

For a folder of 100k rows that is two full listing installs plus one UI-thread sort per event;
`ROW_LABELS` and the thumbnail pass are keyed on the tab revision, so the bump also rebuilt every
visible row label and re-queued the visible thumbnails even when the listing was identical.

## Change

- `lib.rs`: `Tab::apply_refresh(items, sort)` now merge-joins the complete sorted listing into the
  rows on screen, matching by path (the stable row id). Unchanged rows keep their `Entry` and the
  revision, so the label cache and the thumbnail pass stay warm; added/removed/changed rows are
  the only ones replaced. A listing in a different display order falls back to a full replace.
  Selection/anchor reconciliation moved to `reconcile_selection` and is shared with `apply_listing`.
- `lib.rs`: `Tab.streaming` is now authoritative. `Explorer::refresh_generation` marks a live
  refresh (rows stay on screen); `next_generation` keeps a fresh load streaming. `append_scan`
  ignores batches unless the tab is streaming, so a refresh never clears and refills the pane.
- `lib.rs`: `apply_listing` merges a live refresh via `apply_refresh` instead of installing it,
  and the dead `Explorer.sort` (`SortBy`) field is gone.
- `main.rs` / `workers.rs`: `WorkerJob::Scan` carries the header `ColumnSort` plus a `stream` flag.
  The worker sorts in the exact display order with `view::sort_entries` and, for a refresh, builds
  one complete listing without cloning streamed batches. The UI-thread `is_sorted_by` guard now
  only fires if the column changed while the scan was running.
- `app/navigation.rs`: `request_listing` picks the refresh generation when the pane already has
  rows and sends `stream: !refresh`; both scan requests pass `self.column_sort`.

Expected composition shift: a watcher event on an unchanged listing removes the pane
clear/refill, the full `set_entries` swap and the UI-thread sort, and no longer invalidates the
row-label or thumbnail caches. A changed listing costs one linear merge pass instead of a full
install. The worker does the O(n log n) sort where it already reads the directory.

Not done: path-keyed label cache across row moves (a sort click still relabels the visible rows);
it needs a per-row map and the click is user-paced, so it was not the dominant watcher-event cost.
