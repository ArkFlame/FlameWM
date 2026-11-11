# perf-bgaudit: background thread priority audit

Row: `docs/BACKLOG.md` `perf-bgaudit` (not edited here).
Base: `origin/main` at fetch time, worktree `/tmp/opencode/wt-bgaudit`.

## Facade and owner

- Existing facade: `crates/flamewm-integrations-linux/src/background.rs`, `lower_current_thread()` and `BACKGROUND_NICE = 10`, per-thread `setpriority(PRIO_PROCESS, gettid())`.
- Earlier slices using it: `bc54681d` (nice +10 for background workers), `f23fe81b` (explorer thumbnail and watcher), `748027f2` (A/B: no reliable gain, idle and stress-ng within noise).
- Callers today: desktop wallpaper, explorer (main, refresh, search), integrations-linux (icon_service, removable_mounts), shell (icon_loader, weather_host).
- Not reachable from finder, imageviewer, archiver-app. Their `Cargo.toml` has no `flamewm-integrations-linux` edge. Adding one is a new edge into a 12139-line facade (`docs/ARCHITECTURE_EXCEPTIONS.toml`, `facade-source-loc>5000`), and it touches `Cargo.toml` outside this slice's allowed files. Decision: not added. Doc only.

## Classification legend

- LOWERED: the spawned thread (or its spawn path) calls `lower_current_thread`.
- BACKGROUND: background candidate, not lowered.
- INTERACTIVE: UI, input, reactor, WM, lock, PTY, or user-triggered latency path. Must stay at normal priority.
- TEST: test, example, or harness only.
- Basis: `read` = source read at the line; `grep` = lowering call found by grep; `role` = classified from file/function role without reading the line; `backlog` = taken from the BACKLOG row. Rows marked `role` are unverified.

## Site table

| Crate | File:line | Thread | Class | Basis |
|---|---|---|---|---|
| flamewm-finder | src/lib.rs:347 | app catalog + file-walk index | BACKGROUND | read |
| flamewm-finder | src/quick.rs:630 | logout command (user action) | INTERACTIVE | read |
| flamewm-imageviewer | src/decode.rs:173 | prefetch loop | BACKGROUND | read |
| flamewm-imageviewer | src/decode.rs:382 | run() in decode test | TEST | read |
| flamewm-imageviewer | src/decode.rs:571 | test loader | TEST | read |
| flamewm-imageviewer | src/decode.rs:605 | test loader | TEST | read |
| flamewm-imageviewer | src/lib.rs:478 | decode worker for the displayed image | INTERACTIVE | read |
| flamewm-archiver-app | src/window.rs:185 | archive list worker | BACKGROUND | read |
| flamewm-archiver-app | src/window.rs:222 | archive job worker | BACKGROUND | read |
| flamewm-archiver | src/sevenz.rs:79 | child pipe drain (inside job) | BACKGROUND | read |
| flamewm-archiver | src/tests/sevenz.rs:208 | test timer | TEST | grep |
| flamewm-applications | src/lib.rs:716 | desktop catalog discovery | BACKGROUND | role |
| flamewm-assistant-core | src/download.rs:460 | mock server | TEST | backlog |
| flamewm-assistant-core | src/lib.rs:145 | assistant worker | INTERACTIVE | role |
| flamewm-assistant-core | src/openai.rs:319 | streaming reply | INTERACTIVE | role |
| flamewm-assistant-core | src/process.rs:71,77,78,151,281 | child process I/O | INTERACTIVE | role |
| flamewm-assistant | src/lib.rs:276,305,507,557,945 | assistant UI and turns | INTERACTIVE | role |
| flamewm-assistant | src/osd_host.rs:87,144 | OSD host | INTERACTIVE | role |
| flamewm-assistant | src/voice_background.rs:28 | voice speech setup | BACKGROUND | role |
| flamewm-assistant | src/voice_setup_ui.rs:23 | voice setup UI | INTERACTIVE | role |
| flamewm-audio-helper | src/main.rs:66 | audio helper main | INTERACTIVE | role |
| flamewm-audio-helper | src/session.rs:163,203 | audio session I/O | INTERACTIVE | role |
| flamewm-clipboard | src/keeper.rs:26 | selection keeper | INTERACTIVE | role |
| flamewm-clipboard | src/owner.rs:29 | selection owner | INTERACTIVE | role |
| flamewm-control-core | src/mutations.rs:449 | control mutation | INTERACTIVE | role |
| flamewm-control-dbus | src/mutation_queue.rs:100,287 | mutation queue | INTERACTIVE | role |
| flamewm-desktop | src/input.rs:45,85,200 | input | INTERACTIVE | role |
| flamewm-desktop | src/menu_actions.rs:129 | menu action | INTERACTIVE | role |
| flamewm-desktop | src/wallpaper_loader.rs:162 | wallpaper loader | LOWERED | grep (lower at :165) |
| flamewm-explorer | src/app/convert.rs:47 | user-started conversion | INTERACTIVE | role |
| flamewm-explorer | src/app/lifecycle.rs:29 | app lifecycle | INTERACTIVE | role |
| flamewm-explorer | src/app/menus.rs:13,237 | menus | INTERACTIVE | role |
| flamewm-explorer | src/app/navigation.rs:168,191,276,297,338 | directory navigation | INTERACTIVE | role |
| flamewm-explorer | src/app/scroll.rs:7 | scroll | INTERACTIVE | role |
| flamewm-explorer | src/app/transfers.rs:169,187 | user copy/move | INTERACTIVE | role |
| flamewm-explorer | src/dndext.rs:254,346 | drag and drop | INTERACTIVE | role |
| flamewm-explorer | src/main.rs:351,355,370,374,384 | explorer main workers | INTERACTIVE | role |
| flamewm-explorer | src/main.rs:379 | worker (lowered) | LOWERED | grep |
| flamewm-explorer | src/refresh.rs:59 | refresh worker | LOWERED | grep (lower at :62) |
| flamewm-explorer | src/search.rs:289 | search worker | LOWERED | grep (lower at :292) |
| flamewm-explorer | examples/thumb_bench.rs:36 | bench example | TEST | role |
| flamewm-integrations-linux | src/audio_helper.rs:82,197 | audio helper | INTERACTIVE | role |
| flamewm-integrations-linux | src/background.rs:65,81,89,103 | facade's own tests | TEST | read |
| flamewm-integrations-linux | src/device_watch.rs:60 | device events | INTERACTIVE | role |
| flamewm-integrations-linux | src/icon_service.rs:159,270 | icon service (270 lowered) | LOWERED (159 role) | grep |
| flamewm-integrations-linux | src/icon_service.rs:629 | icon worker (lowered) | LOWERED | grep |
| flamewm-integrations-linux | src/icon_service.rs:1404,1427,1435 | not read | INTERACTIVE (unverified) | role |
| flamewm-integrations-linux | src/media_store.rs:208 | media store | INTERACTIVE | role |
| flamewm-integrations-linux | src/removable_mounts.rs:375 | mount worker | LOWERED | grep (lower at :378, :383) |
| flamewm-integrations-linux | src/themes/xz.rs:905 | not read; likely test | TEST (unverified) | role |
| flamewm-integrations-linux | tests/theme_import.rs | test | TEST | grep |
| flamewm-lock | src/power.rs:85,102 | lock power | INTERACTIVE | role |
| flamewm-lock | src/run.rs:332 | lock run | INTERACTIVE | role |
| flamewm-lock | src/run/input.rs:76 | lock input | INTERACTIVE | role |
| flamewm-lock | src/wayland.rs:649 | lock wayland | INTERACTIVE | role |
| flamewm-lock | tests/wayland_unlock.rs | test | TEST | grep |
| flamewm-platform | src/services/settings_persist.rs:65 | settings debounced writer | BACKGROUND | read |
| flamewm-profiler | src/process.rs:99 | profiler process I/O | INTERACTIVE | role |
| flamewm-remote-fs | src/client.rs:151 | remote FS client | INTERACTIVE | role |
| flamewm-remote-fs | src/transport.rs:53,253 | remote FS transport | INTERACTIVE | role |
| flamewm-render-x11 | examples/gpu_switch_pss.rs:86 | example | TEST | role |
| flamewm-render-x11 | tests/foreign_event_livelock.rs, xdnd_source_gone.rs, xdnd_two_windows.rs | tests | TEST | grep |
| flamewm-screenshot | src/notify.rs:80 | notify wait | INTERACTIVE | role |
| flamewm-settings | src/actions.rs:380 | settings action | INTERACTIVE | role |
| flamewm-settings | src/app_impl_2.rs:482 | settings app | INTERACTIVE | role |
| flamewm-settings | src/file_chooser.rs:76,142 | file chooser | INTERACTIVE | role |
| flamewm-settings | src/theme_picker.rs:394,414 | theme picker | INTERACTIVE | role |
| flamewm-shell | src/event_router/drag.rs:70 | drag | INTERACTIVE | role |
| flamewm-shell | src/icon_loader.rs:177 | icon loader (lowered at :180) | LOWERED | grep |
| flamewm-shell | src/weather_host.rs:95 | weather host (lowered at :231, :242) | LOWERED | grep |
| flamewm-task-manager | src/lib.rs:845 | task manager sampler | INTERACTIVE | role |
| flamewm-task-manager | src/sample_gate.rs:78,100 | sample gate | INTERACTIVE | role |
| flamewm-terminal | src/links.rs:123 | link opener | INTERACTIVE | role |
| flamewm-terminal | src/main.rs:101,108,300 | terminal main | INTERACTIVE | role |
| flamewm-terminal | src/pty.rs:83 | PTY | INTERACTIVE | role |
| flamewm-terminal | src/session.rs:42,109 | terminal session | INTERACTIVE | role |
| flamewm-wm-wayland | src/probe.rs:483 | WM probe | INTERACTIVE | role |
| flamewm-wm-x11 | src/frame/titlebar_backdrop.rs:99 | titlebar backdrop | INTERACTIVE | role |
| flamewm-x11-canary | src/scenarios/clipboard.rs:412 | canary harness | TEST | role |
| flamewm-x11-desktop | src/lock.rs:119 | lock | INTERACTIVE | role |
| flamewm-conversion-core | src/conversion/inline_tests.rs | test | TEST | grep |

Coverage: the table covers every file returned by `git grep -n "thread::Builder\|thread::spawn" -- crates` on this base (121 matching lines). LOWERED: wallpaper_loader, explorer (main, refresh, search), icon_service, removable_mounts, shell icon_loader, shell weather_host. BACKGROUND: finder index, imageviewer prefetch, archiver list, archiver job, archiver drain, applications catalog, assistant voice setup, platform settings writer. Rows marked `role` are not verified against source and need a read before any change.

## Decision

- No code change in this slice. The only background candidates in the four named crates (finder, imageviewer prefetch, archiver list/job) cannot reach `lower_current_thread` without a new `Cargo.toml` edge into `flamewm-integrations-linux`, which is outside this slice's allowed files.
- Latency effect: unmeasured. The earlier A/B (`748027f2`) found no reliable gain for the already-lowered explorer and watcher threads, so lowering the remaining candidates is not expected to move foreground latency.
- Follow-up, if pursued: decide whether a lower-level owner for nice-only background priority should exist outside the 12k-line integration facade, then A/B the finder index and archiver job under `stress-ng`.
