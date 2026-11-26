# u25-explorer-lists

Third pass over the Explorer list/grid/details views. Earlier units removed the per-frame template ids,
row labels, statvfs/HOME syscalls, the menu offer probing and half of the thumbnail-key work; this pass
attacks the remaining per-entry cost of listing itself and the deep-scroll/full-projection path.

- lib.rs: `Entry` no longer stores a `lower_name: String`. Every listed entry paid one `to_lowercase`
  allocation (a second heap string per row) at scan time and again on every clone. Name order now folds
  both names in the comparator (`lower_cmp`): ASCII names compare folded bytes with no allocation at all,
  and only the rare non-ASCII pair allocates the two lowercase forms. Ordering is unchanged
  (`to_lowercase()` semantics), the entry row is 24 bytes smaller, and a 100k-entry streamed listing no
  longer makes 100k extra strings. The search filter (`contains_lower`) matches the raw name: both-ASCII
  input stays an in-place byte scan, anything else lowercases both sides inline.
- lib.rs: `filtered_count`, `visible_nth`, `visible_index_of` and `visible_window` take an empty-query
  fast path. With no filter (the normal case) they read `entries` directly instead of walking the whole
  filtered iterator: the count is O(1), the nth window row is O(1) instead of `nth()` from row 0, and the
  window is a listing slice. This also removes the O(n) recount on every streamed batch: a 100k-row load
  used to recount the growing listing on each of its ~50 revision bumps (quadratic), and every full
  projection/hit-test used to filter from row 0 to the scroll origin.
- app/projection.rs: a full pass with no active query copies its 24-row window straight out of the sorted
  listing (`entries.get(first..)`); the filtered walk remains only for a real query.
- view.rs / app/navigation.rs: typeahead matches names in place (`typeahead_match_entries`) when no query
  is active, so typing a letter no longer collects every visible name into a `Vec<&str>` (about 800 KB
  per keystroke on a 100k-row folder plus 100k pushes) before scanning it.
- thumbs.rs: `Thumbnails::begin` checks the pending set before cloning the key. A re-projection of a row
  whose decode is still queued (or in flight) used to clone its path into the set only for `insert` to
  report the duplicate; now a lagging queue costs no per-frame allocation.
- lib.rs: retaining the selection across a live listing apply no longer builds a set of every listed path
  for a small selection (the usual one or two rows are checked by a scan); the set is used only above 32
  selected paths, where it wins.
- mime.rs: `OpenWithChooser::show_every_app` indexed `self.apps[index]` with a possibly stale selected
  index; it now uses `get`.
- app/menus.rs: `spawn_terminal` indexed `argv[0]` even when the terminal resolver returned no command;
  an empty command now reports "Terminal unavailable" instead of panicking.

Expected composition shift: listing load allocates one string fewer per entry and counts rows in O(1)
instead of quadratically while streaming; full projections and hit-testing no longer scan from the top of
the listing; type-ahead drops a per-keystroke O(n) name collection; a full thumbnail queue no longer
allocates per row per frame. Rendering, labels, ordering, selection and dialogs are visually identical.
