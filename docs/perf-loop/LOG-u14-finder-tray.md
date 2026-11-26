# u14-finder-tray changes

- finder/search.rs: `rank` now keeps only the `DISPLAY_LIMIT` best matches in a bounded min-heap
  (one `Candidate` = score, path slice, index) and sorts just those. The old path collected one
  `(i64, usize)` per matching entry (up to `MAX_ENTRIES` = 250k, ~4 MiB) on every keystroke and
  `sort_unstable`-ed the whole set before truncating to 200. Per-keystroke work is now
  O(n log 200) with a few KiB of heap instead of O(n log n) with a full-index buffer, so typing
  latency no longer scales with the collection size; ordering (score desc, path asc) and `total`
  are unchanged.
- finder/state.rs: added `item_count()` and `item_at(position)` so the view has a random-access
  row accessor. The old `items()` built a `Vec<Item>` for the whole result list (thousands of
  48-byte `Item`s) on every repaint just to read `ROW_SLOTS` (8) rows, and on every `select`,
  `rerank` and click hit-test. `items()` is removed; those call sites now use the count/row pair,
  which allocates nothing for the file view (commands/scopes still build their short palette
  list).
- finder/paint.rs: `paint_row`/`paint_tile` borrow the `Entry` from one read guard instead of
  cloning it. The clone copied both `path` and `lower` strings for each of the 8 (or 4) visible
  rows on every paint, i.e. ~16 string copies per keystroke that nothing used past the frame.
- finder/paint.rs: node ids for rows, icons, segments and meta are built into one reused `String`
  with a small decimal writer (`write_slot`/`push_decimal`). A repaint used to `format!` one id per
  icon candidate (`8` kinds × `8` rows = 64), one per segment (`8 × 8` = 64) plus row/parent/meta
  ids: about 150 short heap allocations per frame; now the loop reuses a single buffer. Ids and
  node names are byte-identical.
- finder/paint.rs: `paint_query`/`paint_section`/`paint_items` pass borrowed text through
  `text_ref` instead of cloning the query, the placeholder, the section label and the empty-state
  label every paint; `paint_status` takes the index read guard once instead of three times.
- screenshot/clipboard.rs: the new-owner readiness poll starts at 1 ms and backs off to the old
  10 ms instead of sleeping 10 ms before the first check, so `--copy` hands control back as soon
  as the helper owns CLIPBOARD (typical case a few ms earlier, same 1 s upper bound).
- screenshot/ui.rs: the region-overlay keyboard grab retry uses the same 1 ms to 10 ms ramp, so
  the Print-then-press-again case enters the modal UI sooner instead of paying a flat 10 ms poll
  per attempt.

Expected composition shift: finder typing no longer spikes allocator/RAM (multi-MiB ranking
buffer) or copies the visible entry strings and ~150 id strings per frame; command paths on
screenshot start-up lose one to two 10 ms polls. Visuals, ordering and behaviour are unchanged.
