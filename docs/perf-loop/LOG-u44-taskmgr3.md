# LOG u44-taskmgr3 — task manager batch six

Area: `crates/flamewm-task-manager/src/**` (process table, sort, string cache, graph redraw).

## Review of recent perf commits (regressions/bugs)

Reviewed the batch-four/five slices in `lib.rs`, `model.rs`, `graph.rs` against the current code.
The known hot-set replay bug (u26 skipped `cached.last` for hot pids, so a process leaving the hot
set replayed a sample from before it became hot) is already fixed in the current tree; no new
functional regression was found in the process table, search folding, CPU-delta or depth code.
No dead code or unused imports remain inside the unit's owned paths. The `unused imports/dead
functions` reported by cargo for `flamewm-desktop`, `flamewm-explorer` and `flamewm-settings` are
outside this unit's owned paths and were deliberately not touched.

## Changes (composition-share ranked)

1. **Sort cost: memoized process ordering.** `Application` now keeps a `hits` index vector plus a
   `hits_dirty` flag. Before, every `paint_table` (once per sample *and* on every click, menu
   action and scroll) rebuilt the hit list, lowercased the whole query and, for name sorts,
   lowercased every process name. Now the order is rebuilt only when the snapshot, sort or query
   changes. Scrolling reads the cached length instead of a second full scan+sort
   (`matching_count` removed). Expected composition shift: removes a full O(n log n) sort and two
   per-paint `String` allocations from every non-sample repaint; the per-sample sort is unchanged.

2. **String cache per pid: reusable `comm` `Arc`.** `parse_stat` gained `parse_stat_with_name`,
   which reuses the cached `Arc<str>` name when the `comm` field is unchanged. Each rotating
   `stat` read used to build a `String` in `sanitize` and then a second allocation converting it
   to `Arc<str>`; the dominant sampler op therefore did two heap allocations plus a free per due
   task every tick. It is now a refcount bump. Expected shift: few hundred fewer allocations/s on
   the sampler worker while the window is visible.

3. **Graph redraw: reuse the raster instead of a new `Vec`.** `graph::render` (which allocated a
   fresh 208x88x4 = ~73 KB raster per card per sample) is replaced by `render_into(&mut Vec<u8>)`
   that clears and resizes an existing buffer. `Application` keeps two ping-pong `Arc<Vec<u8>>`
   buffers per card and submits them through `image_rgba8_shared`, so the buffer the document
   still holds is never the one being overwritten (`Arc::make_mut` stays uniquely owned). Expected
   shift: ~220 KB/s of per-sample allocation and copy removed after the first two samples, with
   identical pixels.

Tests: added `ordering_is_cached_until_the_snapshot_or_sort_invalidates_it`,
`unchanged_comm_reuses_the_cached_name_arc`, and
`render_into_reuses_the_caller_buffer_and_clears_it`; updated the existing name-sort and graph
tests to the new APIs.

No cargo/git executed in this unit (shared checkout); changes are compile-reviewed by reading.
