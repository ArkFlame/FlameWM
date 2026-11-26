# u101-explorer-search

Batch fifteen over the Explorer recursive search path. The walk is already off the UI thread (one
supervisor thread, four parallel walker threads), cancelable per job, and streams hits to the UI in
batches; the keystroke path was not changed, because search is committed from the text field rather
than started per keystroke, so no per-keystroke job exists to debounce. This pass removes the
per-visited-name work in the walk and the per-batch UI wakeups it triggers. `cargo`/`git` are
forbidden in this shared checkout, so the change was made by reading the current source.

- search.rs: the walk matched every visited name with `rank_match(...).is_some()`, which spends a
  case-insensitive exact compare and then a prefix compare before the substring scan it actually
  needs. `name_matches` is now a single substring pass (`!query.is_empty() && ci_contains`), and the
  walker calls it directly. `rank_match`/`result_order` still compute the exact/prefix tiers for the
  result ordering; the walk only ever needs "matches at all", so up to two of the three compares on
  each of up to `MAX_VISITS` (200k) names are gone. Matching semantics are identical: exact and
  prefix matches are both substrings.
- search.rs: the ASCII substring test folded every same-width window (`windows(n)` +
  `eq_ignore_ascii_case` per window), so a non-matching name cost a full pass with a needle-length
  copy compare at every offset. `ascii_contains_ci` scans for the first byte (one compare per
  candidate start) and only compares the full needle on a first-byte hit. Non-ASCII names keep the
  existing lowercase-both-sides path. This is the largest non-filesystem share of a search: the walk
  reads at most 200k directory entries and matches every name.
- search.rs: `SearchWorker` woke the UI once per 64-hit batch (up to ~32 wakes for a 2k-hit burst),
  and each wake is a socket write that runs a full Explorer projection and repaint. A `wake_pending`
  flag now drops a wake that is already queued; `poll` clears it before draining, and every event is
  sent before its flag flip, so a batch sent during or after a drain always schedules a fresh wake
  and none is lost. A fast walk costs one repaint instead of one per batch, while results still
  stream on the next poll.
- app/navigation.rs + main.rs + app/lifecycle.rs: re-committing the same query while its walk is
  still running (Enter pressed again, or the search field re-submitted) cancelled the live job and
  restarted the same tree walk from the root. A `search_running` flag, cleared on `Done` and on
  `clear_search`, makes the identical in-flight query a no-op; a changed query or folder still
  cancels and restarts as before.
- search.rs tests: `name_matches` is checked against `rank_match(...).is_some()` over ASCII,
  whitespace and non-ASCII pairs; `ascii_contains_ci` is checked against
  `to_lowercase().contains(...)`; a 202-match burst asserts one coalesced wake and that every batch
  and the `Done` still arrive on the next poll.

Expected composition shift: within a search sample the walk's name matching drops two compares per
visited entry and the substring scan stops folding non-candidate windows; the UI side serves a
streamed burst in one projection instead of one per 64 hits, and a repeated Enter no longer redoes
the walk. Result order, match tiers, status text, streaming and cancellation are unchanged.
