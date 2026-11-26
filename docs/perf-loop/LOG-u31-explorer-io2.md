# u31-explorer-io2

Fifth pass over the Explorer file-operation/search/refresh/remote/ark area. `git` and `cargo` are
forbidden in this shared checkout, so the review of the recent perf commits was done by reading the
current source of the five owned files instead of `git log -p`: the prior changes (shared transfer
buffer, allocation-free `rank_match`, event-driven inotify drain, pidfd child wait, streaming search
batches) are present and were checked for regressions. No regression found. The changes below are
the remaining hotspots and the pre-existing bugs that reading them exposed.

- jobs.rs: the pre-transfer size pass (`count_sources`, the progress denominator for every copy,
  move, drop and trash) now walks a multi-item selection with up to four threads bound by one
  atomic work index. A 100-item selection used to serialise one core's stat loop behind the whole
  batch; it now spreads that walk over four cores. Single-item counts (the common one-folder drag)
  keep the old inline path, so a small job spawns no thread.
- jobs.rs: a cross-device move verified its copy by walking **both** trees (`count_tree(from)` and
  `count_tree(to)`) after the copy, so a move cost three full stat walks (copy + verify + verify).
  `copy_tree` now returns the totals it wrote and `cross_device_move` compares them with a single
  walk of the source, removing one complete tree walk per cross-device move; the verification also
  observes the job's `CancellationToken` instead of the fresh token it used to ignore, so a cancel
  during verification stops the walk and removes the published copy (`copy_publish` keeps its
  public `io::Result<()>` signature).
- jobs.rs: window close waited for in-flight staging with a 10 ms polling loop for up to 5 s
  (~500 wakeups). The staging registry is now a mutex plus condvar: `forget_staging` notifies when
  the last entry clears and `await_staging_idle` parks on the condvar until then or the deadline.
  Same contract, zero wakeups while a slow copy drains its last chunk.
- remote.rs: `copy_items` re-parsed every source and the destination with `Url::parse` up to four
  times per item (plan total, `copy_one` source, `copy_one` destination, `size_of`). It now parses
  each URL once into a plan vector and passes the parsed URLs to `copy_one`, so a 1000-item remote
  copy does ~1000 parses instead of ~4000; `size_of` is gone.
- remote.rs: `is_remote_scheme` lowercased the scheme into a fresh `String` on every call, and it
  is on the path of every `url_of`/`is_remote` classification the explorer makes for tabs, drops
  and navigation. It now compares with `eq_ignore_ascii_case` and allocates nothing.
- dndext.rs: `run_with_timeout` read `gdbus`'s stdout/stderr only after `try_wait` acknowledged the
  exit, so a child writing more than one pipe buffer blocked in `write` while the parent waited for
  the exit: the call died on the 5 s timeout even though it had succeeded. Both pipes are drained
  by dedicated threads for the duration of the call (joined after the child is reaped), keeping the
  pidfd wait event-driven and the timeout/cancel/exit-status behaviour identical.
- search.rs: deleted the unused `Debounce` helper and its `DEBOUNCE` constant (no caller in the
  workspace; the app debounces its own search). Dead code only, search results and ordering are
  untouched.

Expected composition shift: multi-item transfers start their progress bar up to 4x sooner on big
selections; a cross-device move removes one of three full tree walks (and stops it on cancel);
closing the window no longer wakes the UI thread every 10 ms; remote copies remove ~3 of 4 URL
parses per item; path classification stops allocating a lowercased scheme. Visuals, features,
status text and result ordering are unchanged.
