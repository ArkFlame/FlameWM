# u51-explorer-async-audit

Workspace 0.1.7. Routes read: `flamewm-source-first` (source/callers first) and
`flamewm-rust-systems` (Rust edits). `git` and `cargo` are forbidden in this shared checkout, so the
review of the recent perf work was reconstructed from `docs/perf-loop/LOG-u01…u42` plus the current
source instead of `git log -p`.

## Area review (prior perf work)

The scan/transfer/search/thumbnail/remote loops already live on workers, the `refresh` inotify watch
blocks in `poll` with a 200 ms quiet period, `view::fs_space` caches `statvfs` for 2 s, and the
thumbnail/icon caches are bounded. Reading the current code found no stale-state or dropped-event
regression in those paths, but four things were wrong or still on the UI event thread:

1. `app/lifecycle.rs::reload_places` ran `flamewm_explorer::places` inline. That reads
   `/proc/self/mountinfo`, the GTK bookmarks file, stats `is_dir()` on every mount point and reads
   the GVFS share directory. A stale NFS/CIFS mount can block any of them, and it ran for every
   `DevicesChanged`, `DeviceDone`, bookmark edit and `explorer.reload`.
2. `app/menus.rs::open_properties` read `symlink_metadata`/`read_link` inline and then
   re-`symlink_metadata`ed the same path just to learn `is_dir`.
3. `main.rs::source_facts` ran one `symlink_metadata` per source on every drop/paste/duplicate/trash/
   delete guard; a large drag on a network mount meant N blocking stats.
4. `ops.rs::validate` deduplicated with `Vec::contains`, so a select-all confirmation was O(N²) path
   comparisons (twice) on the UI thread before the worker even started.

## New optimizations

- Sidebar places off the UI thread (`app/lifecycle.rs`, `main.rs`, `app/input.rs`, `app/places.rs`,
  `app/menus.rs`): `reload_places_async` builds the list (`places()` plus the Trash row) on a worker
  and sends `WorkerResult::Places{generation,list}`; a monotonic `places_gen` drops an out-of-order
  result. Startup still builds synchronously before the event channel exists. `places()` itself is
  untouched, so entries and order are identical.
- Properties metadata off the UI thread (`app/menus.rs`, `properties.rs`, `main.rs`): the read now
  runs as `WorkerResult::Properties` when the channel exists and `show_properties` builds the dialog
  from the result. `Properties` gained `is_dir` taken from the same `stat`, removing the second
  `symlink_metadata`. `open_properties` keeps its synchronous contract when there is no channel
  (tests/startup). Recursive folder size was and still is on its own thread.
- Listing-aware transfer guard (`app/transfers.rs`, `main.rs`): `source_facts` is now a method that
  matches the requested paths against the current listing once and reuses `Entry.directory`; only
  paths from outside the listing stat. The old free `source_facts` is deleted.
- Hash-based validation (`ops.rs`): `validate` uses `HashSet<&Path>` for both the `seen` and
  `runnable` checks, preserving first-seen order, kept set and verdicts while removing the quadratic
  scans on large selections.

## Dead code

- `main.rs`: removed the free `fn source_facts` after folding it into `ExplorerApp::source_facts`.

## Expected composition shift

- Sidebar changes no longer block the UI on mountinfo/bookmark/GVFS filesystem reads; one short-lived
  worker thread per change.
- Opening Properties no longer blocks on `symlink_metadata`/`read_link` for a slow mount, and one
  redundant stat per open is gone.
- Drop/paste/trash/delete of a visible selection no longer runs per-source stats on the UI thread.
- A select-all confirmation drops from quadratic to linear path work before the worker starts.
- Visuals, sidebar contents/order, dialog contents, validation verdicts and status text are
  unchanged.

## Files touched

- `crates/flamewm-explorer/src/main.rs`
- `crates/flamewm-explorer/src/ops.rs`
- `crates/flamewm-explorer/src/properties.rs`
- `crates/flamewm-explorer/src/app/lifecycle.rs`
- `crates/flamewm-explorer/src/app/input.rs`
- `crates/flamewm-explorer/src/app/menus.rs`
- `crates/flamewm-explorer/src/app/places.rs`
- `crates/flamewm-explorer/src/app/transfers.rs`

## Risks / not done

- Still on the UI thread, left because moving them would change the action flow: `fs::metadata` on
  the destination in `confirm_drop`/`start_duplicate`/`start_transfer_to`, `create_dir_all` when
  entering the Trash or creating the download cache, and `path.is_dir()` in `open()`.
- Reusing the listing's `directory` flag means a file deleted from disk but still listed is treated
  as existing by the guard; the worker still reports the real error.
- `Properties::is_dir` keeps the existing non-symlink semantics (a link is described by its own
  metadata), so dialog behaviour is unchanged.

## Checks

- No `cargo`/`git` in this worker (shared checkout): compile-reviewed only. Every added symbol was
  verified against current source (`WorkerResult`, `Place`, `PlaceGroup`, `properties::Properties`,
  `bookmarks::BlockDevice`, `Entry.directory`, `ops::Source`) and each edited function was re-read in
  full. The orchestrator still runs `cargo test -p flamewm-explorer` and the workspace check.
