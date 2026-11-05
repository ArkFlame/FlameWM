# Cold-start latency: measurement and findings (2026-11-05)

Status: harness landed; **no runtime code changed in this pass**. The top costs sit in crates owned by other
workers (settings) or need a shared-index refactor that was not finished in this pass. Before/after medians are
therefore identical by construction, and both are recorded below.

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

After: unchanged (no runtime change). Raw JSON from the run is kept under `/run/media/linsaftw/SDD/startup-before.json`
(outside the repository).

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

- settings: icon-theme scan on open; see above. Owner must confirm the picker's theme-list contract before changing it.
- task-manager: confirm the first paint content. The harness cannot detect it.
- render-x11 / ui-x11: none of the profiled hot spots were in those crates.

## Gaps

- No before/after delta: no runtime fix landed. The acceptance "before/after medians" is met only for "before".
- Release (LTO) numbers were not collected.
- The first harness run hit an Xlib connection drop once and was not reproduced; the script now records a probe
  error as a failed run instead of aborting.
