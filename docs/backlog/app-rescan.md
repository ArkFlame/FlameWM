# cmp-app-rescan: runtime installed-app refresh

## Slice 1 (landed)
- `flamewm-applications`: `SharedApplicationCatalog` holds an `Arc` snapshot behind an `RwLock`.
- `rescan_from_dirs` re-reads the dirs, swaps the snapshot, and returns a `CatalogDelta` (added / removed / changed desktop ids).
- `shared_application_catalog()` and `rescan_installed_applications()` are the process-wide entry points.
- `opener::startup_identity_for_target` now reads the shared snapshot instead of a process-lifetime `OnceLock`.
- Tests: delta classification, unchanged-dir empty delta, pre-rescan snapshots stay valid.

## Open (not in this slice)
- No caller invokes `rescan_installed_applications` yet. Other consumers still discover once and cache:
  `flamewm-wm-x11/src/runtime.rs`, `flamewm-x11-desktop` `ApplicationCatalogCache`, `flamewm-wm-wayland/src/control/service.rs`,
  `flamewm-shell` dock (`dock_catalog`), `flamewm-finder` (one load per launch), `flamewm-explorer` MIME catalog.
- Publishing: the shell receives `snapshot.applications` from upstream; which process owns the rescan trigger
  (Start menu open, control action, or a user shortcut) and how the delta reaches that snapshot is an unsettled design decision.
- No filesystem watcher; none is in the repository's existing plumbing for these dirs (`flamewm-desktop` inotify covers desktop icons only).

## Runtime
- Not exercised. Behavior on a live session with a newly installed `.desktop` file is unverified.
