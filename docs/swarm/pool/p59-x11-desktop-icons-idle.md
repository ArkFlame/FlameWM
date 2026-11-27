# Unit p59-x11-desktop-icons-idle
STATUS: DONE

Crate flamewm-x11-desktop: desktop icon layout, file watching: inotify not polling, zero wakeups idle, bounded caches; missing XDG dirs degrade silently with one report.

## Result

Audit only: no production edit was needed because every property in the brief already holds in the
current source. Investigation used `rg`/`sed` source inspection only (no `git`, `cargo`, build,
test, Python or sleep loop). Reference-first: `.vendor/xfdesktop-master/src/xfdesktop-file-icon-model.c:1165-1167`
uses one `g_file_monitor()` with `G_FILE_MONITOR_WATCH_MOVES` and coalesces
`CHANGES_DONE_HINT`/create/delete events in `file_monitor_changed` (`:900-1050`) instead of any
periodic scan — the same shape as this inotify design. `.vendor/kwin-master/src` was listed for
the KDE precedent. Reading a reference never made it a dependency or runtime input.

Every row below is **UNBUILT** (static evidence only; the orchestrator builds and tests after the
batch lands). No production file was changed, so no gate was loosened and no pixel can move.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-desktop/src/main.rs:207-219` | One `inotify::Inotify::init()` and exactly one watch on the resolved XDG Desktop directory with CREATE/DELETE/MOVED_FROM/MOVED_TO/CLOSE_WRITE/ATTRIB masks; there is no periodic rescan. | NONE — already inotify-only; UNBUILT. | None. |
| `crates/flamewm-desktop/src/main.rs:706-747` | `register_inotify` hands the inotify fd to the reactor via `register_fd_with_source_action`; it drains until `WouldBlock`, records changed names, sets one `dirty` flag and keeps level-triggered `FdAction::Continue`. No timers, intervals or polling loops. | NONE — fd-driven; UNBUILT. | None. |
| `crates/flamewm-desktop/src/refresh.rs:399-406` | `refresh_if_changed` consumes the dirty flag once per burst (`swap(false, Acquire)`) before one rescan; an idle directory produces no events, no wake and no rescan. | NONE — coalesced already; UNBUILT. | None. |
| `project_sources/FEATURE_DESKTOP.md:28` | Spec authority says "Use inotify and coalesce create/delete/rename/relevant-attribute bursts. No periodic directory scan." Current code satisfies it. | NONE — no change needed; UNBUILT. | None. |
| `crates/flamewm-x11-desktop/src/ports.rs:243-256` | `MainLoopPort::add_poll`/`add_timer` both return unsupported ("desktop host owns X11 event loop/timers"), so this crate registers no poll or timer source. | NONE — no source to remove; UNBUILT. | None. |
| `crates/flamewm-x11-desktop/src/lock.rs:81-115` | The only `sleep` in the crate is in the lock-helper supervisor: it blocks on `child.wait()` and sleeps only after a crash, with a bounded 30s respawn window. It is not a desktop-icon idle wake source. | NONE — out of scope; UNBUILT. | None. |
| `crates/flamewm-x11-desktop/src/lib.rs:56-73` | `ApplicationCatalogCache` uses a `OnceCell<Arc<ApplicationCatalog>>`: discovery happens at most once and the cache cannot grow; tests at `:542-570` pin that contract. | NONE — bounded already; UNBUILT. | None. |
| `crates/flamewm-desktop/src/refresh.rs:519-556` | `ChangedPaths` bounds recorded names at `NAMES_CAP = 1024`; overflow clears the set and forces one full re-check instead of growing without limit. | NONE — bounded already; UNBUILT. | None. |
| `crates/flamewm-integrations-linux/src/icon_service.rs:147-150`, `crates/flamewm-desktop/src/projection.rs:456-477` | Shared raster result cache is bounded by `RESULT_CACHE_ENTRIES = 2048` and `RESULT_CACHE_BYTES = 512KiB*4`; the `ParallelIconService` facade retains no rasters. | NONE — bounded already; UNBUILT. | None. |
| `crates/flamewm-desktop/src/projection.rs:462-470` | Dropped icon requests are reported once through a static `AtomicBool` latch ("further drops not logged"), so a burst cannot flood stderr while idle. | NONE — one-shot already; UNBUILT. | None. |
| `crates/flamewm-desktop/src/main.rs:146-149`, `crates/flamewm-desktop-core/src/model.rs:212-243` | A missing `~/.config/user-dirs.dirs` is read with `.ok()` and `DesktopDirResolver::resolve` falls back to `~/Desktop`; the file is parsed as data, never executed as shell. | NONE — silent fallback already; UNBUILT. | None. |
| `crates/flamewm-applications/src/app_catalog.rs:191-208`, `:323-330`, `crates/flamewm-applications/src/desktop_entry.rs:120-140` | `xdg_application_dirs` uses `xdg_dir` defaults and application dirs are cached once; `from_dirs_with_visibility` skips missing dirs with `if !dir.is_dir() { continue; }`, so absent XDG dirs degrade silently. | NONE — silent already; UNBUILT. | None. |
| `crates/flamewm-debug/src/log_limit.rs:3-5,49-56`, `crates/flamewm-desktop/src/refresh.rs:99-105` | Repeatable degradations go through `log_limit::emit`: first occurrence prints, then at most once per 60s window with a suppressed-repeat count. `warn_degraded` keeps the desktop alive on subscription failure. This is the "one report" for failures that can repeat every tick. | NONE — already rate-limited; UNBUILT. | None. |
| `crates/flamewm-desktop/src/refresh.rs:327-370` | Cursor-theme failures are logged once per distinct (theme, message) with exponential backoff; a missing theme keeps the previous theme rather than aborting. | NONE — one-report already; UNBUILT. | None. |

No compiler, test runner, `git`, Python or sleep loop was invoked, so no runtime measurement backs
this result; every entry is static source evidence marked UNBUILT. No production file was changed,
therefore no gate was loosened and no pixel can move. The only edit is this unit file. One caveat
kept as risk, not changed: `main.rs:149-150` still fails startup if the resolved Desktop directory
cannot be created (`create_dir_all(...)?`); that is an uncreatable path, not a merely missing XDG
directory, so it is outside the "degrade silently with one report" contract as written.

PASS — the audit covers every icon-layout, file-watch, wakeup, cache and XDG-fallback path named in
the brief: watching is one level-triggered inotify fd coalesced into one rescan per burst with no
timer or periodic scan, idle produces no wakeups, all three caches (application catalog, changed
names, icon rasters) are explicitly bounded, and missing XDG directories fall back silently while
repeatable failures are reported at most once per key per 60s window (icon request drops and cursor
failures once per distinct value). All listed properties already hold, so no production fix was
applied and visuals remain pixel-identical; with no code change there is nothing for the
orchestrator's build to break, though the static-only evidence and UNBUILT rows remain as stated.
