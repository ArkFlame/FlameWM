# Cold-start latency: measurement and findings (2026-11-05)

Status: harness landed; one runtime change (settings icon scan, section "Change: shared directory listings"
below). Settings mapped median drops about 11%; the other apps are not touched by the change.

## Harness

`scripts/perf/startup.sh` launches each binary on a private Xvfb (`:90+`, 1280x800x24) with a private session
D-Bus and an isolated `HOME`/`XDG_*`. Timing starts just before `exec` and polls the X tree:

- `mapped_ms`: first new top-level window that is `IsViewable` and at least 100x100.
- `painted_ms`: first `GetImage` of that window whose pixels are not one uniform colour.

Runs: 1 discarded warm-up, then 5 timed runs per app; the report shows medians and per-run values. `archiver` gets a
generated `tar.gz` fixture because it requires an operand.

Usage: `scripts/perf/startup.sh [--bin-dir DIR] [--runs N] [--warmups N] [--json FILE] [app ...]`
(build first: `cargo build --profile fast -p flamewm-explorer -p flamewm-terminal -p flamewm-imageviewer -p flamewm-archiver-app -p flamewm-settings -p flamewm-task-manager -p flamewm-finder`).

Limits:

- Built with the `fast` profile (opt-level 1, no LTO) because fat-LTO release builds were too slow on the loaded
  host. Absolute numbers are not release numbers.
- The machine load average was 3 to 24 during the runs; the same app varied by about 2x across batches (settings
  measured 1845 ms and 791 ms in two batches). Compare ratios within one batch only.
- No flamewm control service runs in the harness, so anything that waits on the desktop is measured at its
  worst case (see terminal below).
- `painted_ms` is not detected for `task-manager`: its window stays one uniform colour, so the check reports
  no paint. Its `mapped_ms` is valid.

## Baseline (fast profile, 5 runs, median ms, batch of 2026-11-05 at 11:2x)

| app | mapped | painted | runs (mapped) |
| --- | ---: | ---: | --- |
| explorer | 131.9 | 222.5 | 117.3, 95.6, 187.9, 131.9, 244.8 |
| terminal | 2392.5 | 2599.7 | 2480.6, 2709.5, 2388.1, 2319.1, 2392.5 |
| imageviewer | 133.0 | 213.0 | 117.9, 120.9, 171.4, 165.3, 133.0 |
| archiver | 93.8 | 127.1 | 78.2, 186.3, 74.5, 129.6, 93.8 |
| settings | 791.1 | 831.6 | 491.6, 649.3, 791.1, 1029.7, 1233.5 |
| task-manager | 166.7 | n/a | 292.0, 139.2, 157.5, 192.0, 166.7 |
| finder | 106.3 | 136.2 | 171.6, 106.3, 91.7, 89.9, 194.3 |

## Change: shared directory listings (settings icon scan)

Cause: `flamewm-settings` `theme_picker::scan` builds one `IconLookupIndex` per installed icon theme. Each build
also watches every installed theme tree (`watch_dirs`, depth 3) and re-reads the Breeze and hicolor fallback
directories, so the same directories were read once per theme.

Fix: `DirListings` (`crates/flamewm-integrations-linux/src/icon_theme.rs`) caches directory reads for one scan.
`IconLookupIndex::build_shared` uses it; the scan creates one `DirListings` and passes it to every per-theme build.
`build()` (desktop, WM, shell) keeps its previous fresh-read behaviour, and `needs_rebuild` always reads the disk.
Lookup results are unchanged: golden test `shared_listings_select_same_paths_as_fresh_build`
(`crates/flamewm-integrations-linux/src/icon_theme/tests.rs`) compares shared and fresh lookups over an inheritance
chain, a scalable dir, a symbolic icon, a threshold dir and the hicolor fallback, and checks a second build reads
no new directories.

Measured in one batch (fast profile, 5 runs, median `mapped_ms`; baseline binaries built from `origin/main`
`c3edfd3e`, new binaries from this change; raw JSON outside the repository):

| app | before | after | delta |
| --- | ---: | ---: | ---: |
| settings | 320.1 | 283.3 | -11.5% |
| finder | 45.3 | 48.9 | noise (not touched) |
| explorer | 55.7 | 55.3 | noise (not touched) |
| imageviewer | 44.6 | 47.1 | noise (not touched) |

Not done in this change:

- Lazy per-name lookup without a full file index (the XDG probe walk). It is not exact-equivalent to the current
  per-theme best/exact-first ranking without a larger rewrite, so it is deferred.
- Persistent on-disk icon index cache. Not needed for the settings win; deferred.
- terminal `GetSettings` retry: resolved in the next section (first paint no longer waits on it).

Raw JSON from the baseline run is kept under `/run/media/linsaftw/SDD/iconperf-base.json` and the new run under
`/run/media/linsaftw/SDD/iconperf-new.json` (outside the repository).

## Change: terminal first paint does not wait on settings

Cause: `flamewm-terminal` `initial_settings()` retried `GetSettings` on the main thread for up to 2 s before the
window was built, so a missing or slow control service delayed the first paint by the whole budget.

Fix (`crates/flamewm-terminal/src/main.rs`, `crates/flamewm-terminal/src/settings_cache.rs`):

- First paint uses the last-known snapshot from `$XDG_CACHE_HOME/flamewm/terminal/settings.snapshot` (fallback
  `~/.cache`), or defaults on a first run. The file is a plain line format (no new dependency), hex-encodes text,
  and is written to a temp file then renamed, so a reader sees the old or the new snapshot.
- `GetSettings` runs on a worker thread with the same attempt/budget. A fresh answer is stored when it differs from
  the cache, sent over a channel and announced by one byte on a clone of the wake socket. The UI loop applies it
  through `apply_settings`, which repaints only when the snapshot values differ (a new revision alone does not).
- Live `SettingsSnapshotChanged` signals use the same `apply_settings` path.

Limit (not changed here): the window backdrop is installed once at window creation from the snapshot available then
(`ui-document` `with_window_backdrop` -> `ui-x11` `run_with_controller_events_role_and_two_readable_fds` ->
`render-x11` `WindowBackdropState::spec`). A backdrop that changes after the window exists is not re-installed; the
cache makes the next launch use the new value. Installing a backdrop on a live window needs a `render-x11` hook, which
is outside this change's owner list (report to the render-x11 owner).

Measured (fast profile, `scripts/perf/startup.sh --runs 5 --warmups 1 terminal`, no control service in the harness,
so the cache is empty and the defaults path is measured; host load 18 to 25):

| app | run | mapped median ms | painted median ms | mapped runs ms |
| --- | --- | ---: | ---: | --- |
| terminal | before (origin/main 6d0caa60) | 2310.8 | 2464.8 | 2310.8, 2253.1, 2377.8, 2334.5, 2250.2 |
| terminal | after | 249.7 | 438.4 | 249.7, 270.6, 230.4, 310.3, 222.5 |

Delta: mapped -89.2 %, painted -82.2 %. Baseline binaries: `/run/media/linsaftw/SDD/startup-bin-base`; new build:
`/run/media/linsaftw/SDD/target-startup-defer/fast`.

Tests: `cargo test -p flamewm-terminal` exit 0 (167 passed, 0 failed, 3 ignored), including the cache codec and
`settings_apply` repaint rules. `cargo check --workspace` exit 0. `python3 -I tools/architecture_guard.py` exit 0
(77 ratified violations, no new ones).

Not covered by runtime evidence: the live-answer path with a running control service (the harness has none), and
the cache-hit paint. Both are covered only by unit tests and code reading.

## Audit: other apps' startup sequences (not changed in this pass)

- `flamewm-imageviewer` (`src/lib.rs` `initial_settings`, called in `run()` before first paint) and
  `flamewm-assistant` (`src/lib.rs` `initial_settings`, before first paint): one `GetSettings` with a 250 ms call
  timeout. Connect failure returns at once, so the cost is only a hung control service (up to 250 ms). Measured
  imageviewer mapped median 133 ms (baseline), so the win is small. Same cache pattern applies if a hung service is
  seen in practice.
- `flamewm-shell` (`src/main.rs` ~line 233): synchronous `GetSettings` before `SurfaceRuntime::new`. The comment says
  the first projection applies saved presentation values. Deferring it changes first-frame font, accent and taskbar
  colour, so it needs a visible-behaviour decision (owner: shell).
- `flamewm-lock` (`src/session.rs` `fetch_snapshot`, 2 s timeout, plus `GetDisplays`): lock appearance and PAM/
  timeout settings are read before the lock is shown. Deferring them is a security-relevant decision; not changed.
- `flamewm-archiver-app`: no settings fetch; mapped median 93.8 ms (baseline). No change needed from this audit.

## Profiling (strace -f -tt, private bus, 8 s)

Two slowest: **terminal** and **settings**.

### terminal (about 2.1 s before the first child process)

- `crates/flamewm-terminal/src/main.rs:170-185` and `initial_settings()` (about line 270): `GetSettings` is retried
  until a 2000 ms budget (`SETTINGS_BUDGET`) is spent, with a 300 ms attempt and 100 ms pause. Without a control
  service on the bus, the trace shows 20 connect/auth/`BEGIN` cycles at about 100 ms each, then the window is shown
  opaque. With the desktop running this returns on the first attempt, so the number in this harness is a worst case,
  not the product number. The comment in code says the backdrop is built once at startup; changing this wait would
  change visible behaviour, so it is not changed here.
  Action: rerun the terminal row with `flamewm-desktop` running on the same private bus before drawing conclusions.
- Thread spawns: 8 `clone3` calls in the trace; the first child is the PTY shell wrapper (`pty.rs:200-235`,
  `setsid` -> `env` -> shell). Shell start is intentional.
- `mono_fonts()` (`crates/flamewm-terminal/src/settings.rs:69`, `fc-list`) does not run at startup (no `fc-list`
  exec in the trace); it runs only when the settings view opens.

### settings (about 0.8 s, 4750 file opens)

- 2772 opens under `/usr/share/icons/hicolor`, 443 under `breeze`, 117 under `breeze-dark`, 110 under
  `MacTahoe`, 55 under `Adwaita`, plus about 450 `ENOENT` probes. This is the icon-theme scan
  (`crates/flamewm-integrations-linux/src/icon_theme.rs`, `walk_theme_dir` at line 386 and `read_theme_index`
  at line 468). It walks every installed theme's directory tree, not only the configured one.
  Owner: settings (`crates/flamewm-settings/**`) and `flamewm-integrations-linux`. Not edited here.
  Candidate fix (not done): build the lookup index for the configured theme only, and build other themes'
  indexes when the theme picker first asks for them. Must keep the picker's theme list and fallback chain unchanged.
- Fonts: fontconfig cache opens are small (`/var/cache/fontconfig`, `/usr/share/fonts`, about 40 opens) and are
  not the bottleneck in this trace.

### Other apps (not profiled in depth)

- `explorer`, `imageviewer`, `archiver`, `finder` are at or below 135 ms mapped. The next target after settings
  is `imageviewer` (decode path) if its first frame is confirmed to be decode-bound.
- `task-manager` (owned by another worker): mapped in 167 ms but never reaches a non-uniform frame in 20 s; check
  whether its first paint is genuinely blank until the first sample.

## Owner handoff (not edited in this pass)

- settings: icon-theme scan on open. Shared listings applied in `theme_picker::icon_previews` (3-line change); the
  picker's theme list and fallback chain are unchanged.
- task-manager: confirm the first paint content. The harness cannot detect it.
- render-x11 / ui-x11: none of the profiled hot spots were in those crates.

## Gaps

- No before/after delta: no runtime fix landed. The acceptance "before/after medians" is met only for "before".
- Release (LTO) numbers were not collected.
- The first harness run hit an Xlib connection drop once and was not reproduced; the script now records a probe
  error as a failed run instead of aborting.
