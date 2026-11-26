# u69-startmenu-open log

Batch ten over the top-center Launcher open path (`crates/flamewm-shell/src/**` and
`flamewm-applications`). `git` and `cargo` are forbidden in this shared checkout, so the review is
source-based and no build ran here: the orchestrator builds, tests and commits.

## Scenario and composition (1 s of normal use)

Super+Space, a few typed characters, one arrow, Enter. Within the owned code the sample splits into:

1. Application catalog: the first open initialized the process-wide catalog, parsing every XDG
   `.desktop` file on the UI thread, and each open re-read the XDG application dirs from the environment.
2. Result rebuild: each keystroke re-filtered every indexed entry, allocated a new order `Vec`,
   cloned the query, and bumped the icon epoch.
3. Projection writes: three `format!` ids per row per projection, plus a fresh transparent
   placeholder image for every row whose worker request is still in flight.
4. Icon worker/drain: eight requests per changed window; launcher slots shared `START_EPOCH` with
   the Start submenu, so typing in the launcher also staled Start's in-flight results, and a stale
   launcher reply had no cache fallback (placeholder stayed until the next projection).

Popup map/measure/raster stays in `flamewm-render-*`, outside this unit.

## Review of the landed slices (regressions fixed)

- Launcher icon bindings were keyed on `START_EPOCH` (`refresh` called `bump_start_view_epoch` on
  every open and keystroke), so launcher typing staled the Start submenu's pending icon results
  and vice versa. Launcher slots now use their own `LAUNCHER_EPOCH`
  (`bump_launcher_view_epoch`), matching the existing dock/task epoch split documented in
  `icon_binding.rs`.
- A launcher completion that landed after an epoch bump was dropped as stale and left the row on
  its placeholder. The `DependentSurface::Launcher` drain arm now falls back to
  `apply_cached_binding`, the recovery the panel task slots already had.

## Optimizations

- **Catalog discovery leaves the first open (`app_catalog.rs`, `main.rs`, `app/launcher_session.rs`).**
  `warm_application_catalog_in_background` parses the dirs on one named worker at shell startup.
  `ready_shared_application_catalog` is a non-blocking `OnceLock::get`; the launcher open uses it and,
  while warm-up is still in flight, keeps the Start snapshot already received from the session bus
  instead of blocking the UI thread on discovery. Composition shift: first-open read/parse/alloc
  moves to startup idle; an open costs only the dir `stat` gate.
- **XDG dir list cached (`app_catalog.rs`).** `cached_application_dirs` reads the environment once.
  The shared-catalog init now uses `from_dirs` with that list (was: `discover()`, a second env read),
  and the rescan/refresh/launch lookups all reuse it. Removes env reads and `PathBuf` vector
  allocations from every open and launch lookup.
- **Search index reuse on open (`launcher.rs`).** `refresh` returns early when the cached order
  still matches the model generation and query, so a reopen reuses the result indices instead of
  rescanning the index and allocating a new vector. It also keeps the launcher epoch in that case,
  so in-flight launcher icons survive a reopen. Composition shift: reopen filtering O(entries) -> O(1).
- **No per-row id allocations in projection (`launcher.rs`).** Three cleared-and-reused `String`
  buffers replace three `format!` calls per row, matching the `start_view` slot precedent (LOG-u09).
  Removes 24 heap allocations per keystroke projection.
- **Pending-row placeholder not repainted (`async_projection/icon_binding.rs`, `icon_loader.rs`).**
  `schedule_visible` now proves the node already shows this key (`IconLoader::is_pending` plus the
  live binding key/generation) and returns without rebuilding the transparent image or touching the
  document. Cache hits and fresh misses are unchanged, so no icon is delayed.

## Dead code deleted

- `LauncherState::query()`: no caller in the workspace (the field is read directly).

## Not done here / remaining

- Incremental prefix filtering needs a subset API on `StartModel` in `flamewm-shell-core` (outside
  the owned paths).
- Row label/visibility writes still run per projection; avoiding them needs change tracking in the
  document layer, which is outside this unit.
