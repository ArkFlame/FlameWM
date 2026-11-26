# u59-finder-assistant changes

Review note: this shared checkout forbade `git` and `cargo`, so the recent perf commits were
reviewed by reading the current sources rather than `git log -p`. No regression found in the
earlier fixes (off-thread cache/document reads, borrowed row entries, single-guard duplicate sheet,
no full-index path map on watcher commits); the changes below target what those fixes left.

## Finder
- search: fuzzy subsequence matching (the per-entry path every non-substring candidate falls
  through) now compares bytes when name and term are ASCII, skipping per-character UTF-8 decoding;
  non-ASCII text keeps the exact character walk. This is the largest per-keystroke slice because it
  runs for every entry that misses the substring tests.
- state/lib: `rerank` reuses the previous `Hits` when the query text, the new `Index::revision` and
  the day are unchanged. Startup application/session/frecency messages and watcher notifications
  each used to rescore the whole index for the same query. `revision` is a process-wide counter
  bumped by `Index::finish`, so replacing the index (cached listing, then fresh walk) always
  invalidates the cached rank.
- paint: one read guard spans the whole row loop (`paint_row`/`paint_tile` take `&Index`); the row
  paint previously re-locked per row, widening the window where a repaint waited on the watcher's
  write lock. Same guard also serves the empty-view check, so a paint takes two locks instead of
  nine.
- index: `load_cache` reads with one reused line buffer instead of `BufRead::lines`, which allocated
  a `String` per indexed entry - hundreds of thousands of allocations per launch on a full cache.
- format: `parent_for_display` compares the home prefix by bytes instead of building `"{home}/"`
  for every painted row.
- search: deleted the unused `Hits::recent` flag (only a test read it; the empty-query view is
  already the sole `docs`+`recent` path).

## Assistant
- model: `Row` borrows its text (`Row<'a> { text: &'a str }`) instead of owning a `String`, so a
  streaming repaint no longer clones up to `SLOTS` message bodies per delta.
- lib: `paint_models` skips `menu_rows()` while the picker is closed; the filter over every model id
  ran on every chat repaint (each streamed token) for a hidden menu.

Expected composition shift: lower UI-thread CPU per keystroke and per streamed token, fewer startup
allocations from the index cache, and a shorter write-lock stall window between the watcher and the
repaint. No visuals or features removed.
