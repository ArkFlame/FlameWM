# u42-explorer-warn

Batch six over the Explorer warning and render/input path. `git` and `cargo` are forbidden in this
shared checkout, so the review of the recent perf commits was done by reading the current source of
the owned crate instead of `git log -p`. The landed passes (cached ids, cached row labels, cached
breadcrumbs/address/count/free/progress, kept thumbnail jobs, header-first thumbnail reads,
event-driven scroll/refresh, allocation-free `rank_match`, condvar staging wait) are present and were
checked for regressions; no regression or stale-state bug was found. The changes below are the next
hotspots by composition share plus the dead helper the current tree no longer needs.

- input.rs: `key_inner` lowercased every key name with `to_lowercase()`, which allocates a `String`
  on every key press. Typing in the path or search field therefore allocated once per keystroke on
  the input path. The name is now borrowed as `&str` and only lowered on the rare name that contains
  an uppercase character, so the common `up`/`return`/`backspace`/single-character path allocates
  nothing. The match arms are unchanged; only the allocation is gone.
- navigation.rs + view.rs + lib.rs: type-ahead with an active filter collected every visible name
  into a `Vec<&str>` (`tab.visible().map(name).collect()`) on every typed character, up to ~800 KB of
  pointers in a 100k-row filtered folder, and then scanned that vector. `view::typeahead_match_iter`
  now scans any name iterator in place with the same start-at-`from` wrap ordering, and
  `Tab::typeahead_match` feeds it the filter's own iterator with the cached `filtered_count()` as the
  length. The filtered branch no longer allocates; the unfiltered branch keeps its existing
  allocation-free slice scan.
- app/projection.rs: the context-menu offer pass built a `Vec<&str>` of the offered items on every
  full projection, even when the menu was closed, and then looked each item's index up with a linear
  `MENU_ITEMS.iter().position`. It now fills a fixed `[usize; MENU_SLOTS]` of offered indices and
  uses them directly, so the per-frame menu pass allocates nothing and drops the O(n²) lookup.
- app/places.rs + app/projection.rs + app/navigation.rs: the projection compared the tab path with
  `self.trash_base.join("files")`, building a fresh `PathBuf` on every full pass; the entry menu and
  the bookmark test built the same path on each menu rebuild, and navigation built it on every
  folder change. `is_trash_files` compares the parent and the `files` file name instead, so the
  frame path and the navigation check stop allocating.
- view.rs: deleted `typeahead_match(&[&str], …)`, which the allocation-free filter path superseded
  and whose only remaining caller was its own test; the test now drives `typeahead_match_iter`.

Dead-code note: the crate's only test-only helper (`thumbs::read_source`) is already behind
`#[cfg(test)]`, the private-function scan finds no unreferenced non-test functions, and the
`pub` `Progress`/`staging_count` items are exported library API rather than dead code, so there is
nothing left to delete in the owned paths beyond the superseded helper above. The cargo warnings the
unit mentions for `flamewm-desktop` and `flamewm-settings` are outside this unit's owned paths.

Expected composition shift: typing in the path/search field and type-ahead in a filtered folder no
longer allocate per keystroke (the filtered type-ahead drops an O(rows) pointer copy per character);
the context-menu pass and the status-strip trash check no longer allocate per full projection.
Visuals, menu ordering, type-ahead matching order and every status/feature are unchanged.
