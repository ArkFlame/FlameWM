# Log u40-misc-apps

Area: `flamewm-{settings,finder,imageviewer,archiver,tray,ribbons,lock}`.

Reviewed the recent perf slices that touched this area (u14 finder/tray, u15
imageviewer/archiver, u17 settings/lock) by reading the current code. Ordering,
precedence, cache keys and progress semantics match those logs; no behavioural
regression was found. One latent bug was fixed, then the next hotspots by
composition share were attacked.

Bug fixed
- `finder/paint.rs` (`paint_duplicates`): the sheet indexed
  `self.view().entries[group[0]]` directly. `groups` stores index positions and
  the inotify worker replaces the shared index asynchronously, so a removal
  between `accept_changes` and a repaint could index past the end and panic the
  window. The row is now read with `entries.get` and simply hidden when its
  entry is gone; no visual change for a consistent index.

Optimizations
- `finder/watch.rs` (`run`): the watcher cloned the whole shared index
  (`Vec<Entry>`, up to `MAX_ENTRIES` = 250k entries with two `String`s each, tens
  of MiB) only to read directory paths, then dropped it. It now registers
  watches from a read guard of the live index. Expected: the watcher starts with
  no full-index copy, removing the largest single RAM/time spike of finder
  start-up under a large home.
- `finder/index.rs` (`Index::finish`): resolving the recent-document list built
  a `HashMap<&str, usize>` over every entry. `commit` runs `finish` after each
  watcher batch, so under filesystem activity this allocated and filled a
  250k-entry map per batch (several MiB). It now builds a set/map bounded by the
  recent-document count and scans entries once for membership. Same resolution
  and order (last duplicate entry wins, document order preserved).
- `finder/index.rs` (`Entry`) + `finder/search.rs` (`score`): the depth penalty
  was recomputed as `path.matches('/').count()` for every entry on every
  keystroke, i.e. one full path scan over the index (tens of MiB) per query.
  `Entry` now stores `depth`, folded once in `Entry::new` (walk and cache load).
  Scoring reads the field instead. Expected: typing no longer pays an O(path)
  scan per entry in addition to the term match.
- `archiver/progress.rs` (`copy_tracked`): the 64 KiB copy buffer was allocated
  and zeroed per entry. Creating or extracting an archive of many small files
  did this for every file (e.g. 100k allocations/zeroings of 64 KiB). A
  thread-local scratch buffer is reused and only grown to `COPY_BUFFER`.
  Semantics (cancel between chunks, `Interrupted` retry, byte accounting) are
  unchanged. A re-entrant copy that cannot take the scratch cell falls back
  to a temporary buffer instead of panicking, so writer callbacks stay safe.
- `settings/startup_page.rs`: dropped the now-unused `rows()` and `candidates()`
  wrappers that re-opened the environment by themselves; every live caller uses
  the shared facade through `rows_with`/`candidates_with`.
- `finder/paint.rs` (`paint_duplicates`, `paint_setup`): the duplicates sheet
  took the index read lock several times per row and cloned a full `Entry`
  (two `String`s) per group; it now holds one read guard and borrows entries. The
  setup sheet took the lock twice per paint; it now takes it once. Removes per
  frame lock traffic and short-string churn while an overlay is open.
- `settings/app_impl_3.rs` (`sync_startup`) + `settings/startup_page.rs`
  (`rows_with`, `candidates_with`) + `settings/startup_extras.rs`: the Startup
  page resyncs on every input event and opened `ApplicationSettings` twice per
  repaint (once for the rows, once for the picker candidates). One facade read
  now serves both. Expected: one fewer environment read per Startup-page repaint
  (the directory scans themselves are unchanged and still gated to the visible
  page).

Files touched
- crates/flamewm-finder/src/index.rs
- crates/flamewm-finder/src/search.rs
- crates/flamewm-finder/src/watch.rs
- crates/flamewm-finder/src/paint.rs
- crates/flamewm-archiver/src/progress.rs
- crates/flamewm-settings/src/startup_page.rs
- crates/flamewm-settings/src/startup_extras.rs
- crates/flamewm-settings/src/app_impl_3.rs

Dead code: `startup_page::rows` and `startup_page::candidates` were only reading
the environment to be thrown away by the page projection; both are gone now
that the projection owns the facade read. Other unreferenced helpers were kept
because they are used from their own module or tests.

UNVERIFIED: no `cargo`/`git` was permitted for this unit, so this is reviewed by
reading only. Compilation and runtime behaviour were not executed. The changes
keep public types, node ids, ordering and error texts unchanged.
