# Perf loop log (T0=2026-11-26T09:27:11-03:00)

## u156: backdrop drag throttle

Expected composition shift during a translucent-window drag: captures per second drop from about 60 to about 20; not runtime-measured. Verification omitted because cargo is prohibited.

## u150: managed-window scene backdrop

Generalized the Dock scene cache to identify Dock and managed-window sites; managed windows capture beneath their own front-to-back scene position, draw their cached rounded client mask below client surfaces, and omit the opaque client fill when eligible. Flat, opaque, missing, or closed sites drop/refrain from retaining cache entries; capture-mode scene composition emits no backdrop elements. Static comparison remains in-place against the retained scene stamp. Added pure site identity/eligibility regression coverage. Verification intentionally omitted by handoff (cargo/git prohibited); static source inspection only, and Managed.material is supplied by u149.

## U149: Window material compositor read

Published XWayland compositor backdrop capability and read, clamped, cached, and refreshed per-window material for X11-managed windows; added parse/clamp regression tests. Native Wayland windows retain the default `(100, 0)`.

## u148: app-side compositor-owned window material

`render-x11` samples root `_FLAMEWM_COMPOSITOR_BACKDROP` once at app creation. When enabled, it selects the composited ARGB visual when available and bypasses snapshot/live-scene capture, leaving the scene's transparent clear intact. Changed material requests write `_FLAMEWM_WINDOW_MATERIAL` as `CARDINAL[2] = [opacity_percent, blur_percent]`; removal/opaque requests delete it. Pure tests cover flag and word encoding. Verification: `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo check -p flamewm-render-x11` passed (22.20s).

## Build 1 (min 30): cargo check --workspace --all-targets green after 4 fixes (render-x11 gpu.rs borrow, explorer places.rs inference x2, projection.rs labels). 24 units landed (u01-u24); u25-u30 in flight.

## Build 2: cargo check --workspace --all-targets green after 2 fixes (archiver progress.rs scratch buffer lifetime, desktop rubber.rs test types). u01-u40 landed. Root disk at ~100% (other projects /tmp/opencode 58G); pool paused disk_low.

## Build 3: cargo check --workspace --all-targets green after 2 fixes (task-manager duplicate Sampler.text field, desktop inotify events.count() then reuse -> peekable). u01-u50 landed, u51-u57 in flight.

## Build 4: cargo check --workspace --all-targets green after 3 fixes (tray watcher match Some(), terminal run_id_into import, terminal test frame_rows) + scripts/perf-loop check fix. Python/ctx_execute banned (.agents/NO-ADHOC-SCRIPTS.md); perf tests scoped to 1 s composition (.agents/skills/scoped-perf-tests). u63-u67, u56, u61, u62 still in flight.

## Build 5: cargo check --workspace --all-targets green, 0 fixes. u56,u61-u67 landed; stale-pool reaped and restarted with abort_requeue.

## Build 6: cargo check --workspace --all-targets green after 1 fix (terminal session.rs PollFd::new takes &fd). u68-u78 landed or in flight.

## Release build + nested smoke after build 6: release build 0 errors (3 dead-code warnings), `scripts/flamewm-nested-smoke` PASS (10 s, no early exit). First real run of u01-u78 changes on the compositor; no composition profile taken yet.

## First composition (scripts/perf-compose, 1 s, nested idle): 100% of sampled ticks on the single main thread `flamewm-wayland`; no per-subsystem split from thread names. Next: add flamewm-profiler counters or perf record for a within-thread split (unit u79).

## Build 7: cargo check --workspace --all-targets green, 0 fixes. Pool stall root cause: ghost "running" workers (41) from finished units filled the lane cap; abort_all does not clear them. Workaround: move open units to a fresh dir (units-b) for a fresh pool id; scripts/perf-loop now scans units*/.

## Build 8: cargo check --workspace --all-targets green after 2 fixes: (1) wm-wayland render/mod.rs collect_occluders match missed macro-generated Elem::_GenericCatcher; (2) shell switcher.rs stray #[test] on a helper fn with args. Batches 12-13 landed.

## Composition #2 (FLAMEWM_PROFILE_COMPOSE=1, nested idle, 13 one-second windows, build 8 release): wait=100% of window; spans: wmwayland.dispatch.flush x1/s at 0.00 ms. cpu_total ~9-10 ms/s (0.9%) with NO span covering it => unattributed work is the profiler dump itself plus the 1 s tick; idle compositor is parked. pss=58 MB rss=119 MB. Conclusion: idle is healthy; the share to cut is under ACTIVE use (drag, client damage, typing). Next unit u92 drives a client scenario.

## Composition #3 (scripts/compose-scenario, u92) is INVALID as an active-use measurement: nested XWayland ready on :2 and client targeted :2 (not the real :0), but the terminal client log stops at FLAMEWM_RENDER_* startup lines and the compositor stayed 99.8% wait, so no load reached it. Do not cite these shares. Unit u93 must make the load real (client maps a window, output visible in client log, non-wait share > 5%).

## Build 9: cargo check --workspace --all-targets green, 0 fixes. Batches 12-13, u92, u93 landed.

## u94 verdict (code-read, not yet run): NOT a bug that X11 updates are dropped; 99% wait is the compositor parked while the X11 client paints in its own process. Two real defects fixed in wm-wayland: (1) frame requests during a paint were dropped (render_frame takes state.backend), (2) skipped paint left the 16 ms winit pacing clock stale. Nested paints measured 100-280 ms, dominated by blocking host swap_buffers in smithay winit => the nested harness cannot give a steady ACTIVE composition; do not tune compositor CPU from it. Active-use composition needs the real DRM session or a headless backend.

## Violation: an OpenCode worker (opencode serve pid 1131738) spawned /tmp/ctx-exec-th0hPN/code.js despite BRIEF rules. Killed by pid (12 s old, 0.3% cpu). Brief already forbids ctx_execute; check again each cycle.

## u99 verdict: no cheap headless backend (smithay has no backend_headless; pixman still needs a display). Steady ACTIVE composition requires the real DRM session; nested/winit cannot provide it. Use code-read hotspot ranking for active paths until a real-session run is possible.

## Build 10: cargo check --workspace --all-targets green after 3 fixes: (1) explorer main.rs settings_wake cloned after writer moved into io thread, (2) shell main.rs load_panels_bootstrap tuple order from subscribe_then_load, (3) explorer thumbs queue test compared retain() Vec to an int. Batches 14-16, u94-u99 landed.

## Release + nested smoke after build 10: release 0 errors (2 warnings); scripts/flamewm-nested-smoke PASS (10 s, no early exit, no panic lines in log). Covers batches 9-16 compositor-side changes at startup only; explorer/shell/terminal/task-manager release binaries not run.

## Build 11: cargo check --workspace --all-targets green after 1 fix: archiver progress.rs tests held &mut seen in the sink closure while asserting; now RefCell. Batches 17-18 landed.

## u134 (platform settings persist): code read confirms `SettingsWriter` already uses the requested coalescer — parked in `recv()` with no pending text, and `recv_timeout` armed at min(idle, max_defer) while a burst is pending, so a drag collapses to one atomic write of the newest text and shutdown drop flushes the last write. Remaining `thread::sleep` calls are test fixtures only. No edit needed; area is idle wakeups. Expected composition shift: zero timer wakeups while parked and one disk write per burst instead of one per scheduled change.

## u133 (terminal PTY sleeps/polling): the read path already parks with zero idle wakeups — `session.rs::read_loop` calls `poll` with no timeout while no wake is owed and merges a burst into one `Output` event (up to `BATCH_BYTES`) before the single frame-boundary wake byte, so no sleep loop remained there. The one remaining timer poll in the owned PTY file was `hangup_then_reap`, which sampled `child.try_wait()` every `HANGUP_POLL` (5 ms) for up to `HANGUP_GRACE` (250 ms) on every tab close. Replaced the sampler with `pidfd_open` + `poll` on the child's pidfd: one blocking wait bounded by the grace, then `SIGKILL` only on timeout; the pre-Linux-5.3 sampling path stays as a fallback. Area: terminal close path (idle read loop already clean). Expected composition shift: a tab close costs one poll wait instead of ~50 `try_wait`/`nanosleep` wakeups; steady-state idle remains zero wakeups.

## u132 (explorer UI-thread fs): audited the five explorer modules. `dndext::spawn_ark_extract` performed the destination `is_dir()` probe synchronously on the drop path, so extracting onto a stale GVFS/SMB mount could freeze the UI before any worker existed; split `validate_syntax` (pure, synchronous) from `validate_request` (syntax plus the stat) and let the worker run the stat. `places::gvfs_shares` probed every entry with `path.is_dir()`; it now uses `DirEntry::file_type`, which reads the d_type already returned with the readdir batch, so listing shares costs no per-entry syscall on the FUSE mount. `lib::home_dir` probed `is_dir` on every call (each sidebar rebuild and path edit); it is now cached once through `OnceLock`, and a currently missing home is not cached so a late mount still resolves. Read-confirmed that `properties::read`/`recursive_size`, `trash::list`/`restore`, the places build and local listings already run on the aux/worker pool, so no change was needed there. Area: explorer drag-drop, sidebar, path resolution. Expected composition shift: drops the one blocking stat from the archive-drop UI path, removes one stat per GVFS share from each sidebar build, and reduces the home probe to once per session. Files: crates/flamewm-explorer/src/dndext.rs, crates/flamewm-explorer/src/places.rs, crates/flamewm-explorer/src/lib.rs.

## Reliability batch: u130 archiver first-bytes progress emission (cancel tests were never firing under throttling), u131 desktop t03, u132 explorer ui-thread fs, u133 pty wakeups, u134 settings_persist coalescer, u135 terminal run cache keyed on alt-screen. Tests for archiver/desktop/terminal/explorer/platform green; CI clippy green on main; PR #27 merged.

## u136 (explorer thumbnail/app-icon cache): read `thumbs.rs` and `appicon.rs` plus the callers in `main.rs`, `workers.rs`, `app/projection.rs`. Already correct and left alone: the decode runs only on the single `thumb_worker` thread, the cache is an LRU with a real pixel-byte budget (`CACHE_BYTES` 256 KB) on top of `CACHE_CAPACITY` 256 entries, and a row that leaves the view is dropped from the queue, cancels the in-flight decode at the next `wants` check, and loses its `pending` mark. App icons need no separate cache: a `.desktop` row's resolved icon is decoded through the same bounded thumbnail entry. Missing: nothing ever released the cache for a covered/minimized window (`UiControllerEvent::Visibility` was ignored), so a background explorer held its pixels forever. Added `HiddenCache` with `HIDDEN_CACHE_GRACE` (30 s), `Thumbnails::clear`/`bytes`, `ThumbQueue::clear` (also abandons the in-flight decode) and the pure `hidden_cache_expired` policy, plus a scoped test proving a quick uncover keeps the pixels and a hidden window drops cache and queue exactly once. Area: explorer thumbnail/icon caches. Expected composition shift: an explorer hidden in the background releases its thumbnail pixels (up to the 256 KB byte cap) and leaves no queued decode work, while an alt-tab back inside the grace still paints ready thumbnails; visible-window cost is unchanged. Wiring still owed outside this unit's paths (main.rs `Visibility` arm): `app.hidden_cache.set_hidden(obscured, Instant::now())` then `check(now, &mut thumbs, queue)`, re-project when it drops, and arm one wake at `deadline()`. Files: crates/flamewm-explorer/src/thumbs.rs, crates/flamewm-explorer/src/thumbs/tests/mod.rs.

## u138 (compositor frame callbacks): the redraw gate (`render_frame`) and the element builder already skip occluded, minimised and off-output windows, and the DRM pass already drops empty frames; `send_frame_callbacks` still walked every non-hidden toplevel and X11 window. Added `render::frame_reaches_output` (frame rect scaled and tested against the output rect) and used it in `scene_elements_into` (replacing the inline `frame_rect.overlaps(visible)`) and in `send_frame_callbacks` for both the Wayland toplevel loop and the X11 filter, so a window dragged past an output edge or sitting outside the shared scene gets no callback. Area: frame callbacks / client rasterisation. Expected composition shift: a fully clipped window no longer receives a callback each frame, removing its 16 ms-throttled commit-and-rasterise share from the paint loop; visible and partially visible windows are unchanged. Files: crates/flamewm-wm-wayland/src/render/mod.rs.

## u137 (task manager hidden/unfocused sampling): the sampler already parked on the `SampleGate` condvar while obscured, but the visibility change had to wait for the 1 s `recv_timeout` to expire, and covering the window while it slept let one extra full `/proc` walk run hidden. Merged the process-action wake and the visibility change into one `WorkerMsg` channel: the `Visibility` handler now nudges the sampler at once, a hide parks it with no timer, and an uncover releases the same wait so the next sample is taken immediately instead of up to 1 s later. Extracted the visible-slot range into the pure `visible_slots` function (paint already formatted only those rows) and added a scoped test locking that in; the graph still gains exactly one point per real sample, so a pause leaves no fake interpolation. Keyboard focus is not exposed by `UiControllerEvent` (only `Visibility`), so a visible-but-unfocused window keeps the 1 s cadence; slowing it needs a focus event in `flamewm-ui-x11`. Area: task manager sampler idle path. Expected composition shift: hidden windows take zero samples after the hide notice (previously up to one extra), and a shown window samples within the same frame instead of sitting on a stale graph for up to 1 s. Files: crates/flamewm-task-manager/src/lib.rs.

## Batch u136-u138: explorer thumbnail cache byte cap + cancel, task-manager sampler interrupted by visibility (hidden parks at once, show samples now, only visible rows formatted), compositor frame callbacks and element culling share one off-output predicate. Tests for task-manager/explorer/wm-wayland green.

## u142 (reactor timer deadlines): dispatch already parks with an infinite timeout when no slot is set (calloop epoll wait, no fixed tick) and `Reactor::remove` drops a timer by token without scanning. The remaining per-dispatch cost was `earliest_wake` re-scanning the shared slot plus every `owned_wakes` slot on each turn. Gave `WakeDeadline` a shared dirty flag written by `set`, cached the earliest deadline in the reactor, and recompute it only when a slot changes or a new slot is created. Added a scoped test that the cache recomputes on each `set` and stays stable otherwise. Area: reactor timer wheel idle/dispatch path. Expected composition shift: idle dispatch now reads one bool and one cached `Option<Instant>` instead of iterating all deadline slots per wakeup; steady idle stays fully parked, so the cut is per-event overhead in the shared loop. Files: crates/flamewm-reactor/src/lib.rs.

## Batch u139/u142: shell launch_feedback collapsed to one badge deadline (parks with no wakeup when nothing in flight); reactor caches the earliest wake deadline, recomputed only when a slot is written. reactor+shell tests green.

## u141 (X11 paint/provider timers): audited the three owned paths. Already event-driven and left alone: provider FDs are reactor sources with one bounded drain per readiness, the battery CPU sampler arms a deadline only while the battery menu is open, the titlebar repaint is damage/motion-driven with a 16 ms pacing cap and a deferred flush armed only while a paint is deferred, and the startup heap trim is a one-shot timer. Two per-turn/per-paint polling costs were removed. (1) `poll_outline_tune` ran on every loop turn and `outline_tokens` on every chrome paint (60/s during a drag), each doing a `getenv` scan and, when the dev tune file is set, a `stat`; the INTERNAL gate is now resolved once per session (`outline_tune_configured`/`OnceLock`), so an unset `FLAMEWM_OUTLINE_TUNE` costs nothing per turn or paint while live tuning is unchanged. (2) the hook rewrote the reactor `wake_deadline` slot every turn, dirtying the earliest-wake cache even when the value did not move; it now writes only when `cpu_deadline_moved` reports a change, so closed-menu turns never touch the slot. Pure tests: env gate (unset/empty/path) and deadline rewrite/no-rewrite across closed menu, first open, between samples and a due tick. Area: X11 WM loop-turn polling and chrome paint. Expected composition shift: idle and drag turns lose their `getenv` (and any tune `stat`) share; the reactor recompute no longer runs on turns that leave the CPU-sampling deadline unchanged. Files: crates/flamewm-wm-x11/src/wm/paint.rs, crates/flamewm-wm-x11/src/runtime/providers.rs, crates/flamewm-wm-x11/src/runtime/mod.rs. cargo check -p flamewm-wm-x11 --all-targets green.

## u141: x11 runtime only rewrites the reactor wake slot when the CPU-sample deadline moves (idle turns leave the earliest-wake cache clean); outline-tune env lookup cached once per process instead of getenv per paint. wm-x11 tests green.
## u140 (compositor dock backdrop): the file already cached the built dock texture under a full key (rect, radius, scale, material, per-element scene stamp); the only eight clone() sites were in the unit tests. Replaced those with borrows, and removed the per-frame stamp vector: refresh_dock_backdrops now checks the cheap key fields first and compares the freshly built elements against the cached stamp in place (scene_matches/stamp_matches), allocating a stamp only when a capture is due. Expected composition shift: a clean frame with a translucent dock spends zero stamp allocations and rebuilds no texture; the element build remains because it is what feeds the comparison. Pixels unchanged.

## Batch u140: dock backdrop keeps the built texture and compares scene stamps in place (no per-frame stamp Vec, no test clones); ordered-stamp unit test added. Tests not run in the worker (no cargo); reviewed by reading.


## u140: dock backdrop builds the scene stamp only when a capture is due; a static scene compares in place with no allocation. wm-wayland tests + clippy -D warnings green.

## u143 (shell audio projection): every audio event and every quick-control tick rebuilt the popup with `format!("audio-slot-{n}-…")` per property (four ids plus the ARIA track per row, three per row for percentages) and a `format!` for the value and the two ARIA numbers, so one refresh allocated ~24 short `String`s and re-wrote ~100 document properties even when the model was identical. `project_audio_full` now computes an allocation-free digest of what it paints (tab, raise-maximum, live accent, and each visible row's id/label/volume/mute/kind) and returns before any write when the thread's previous projection produced the same digest; element ids are assembled once per row in two reused capacity-reserved buffers (`audio-slot-N` base plus `push_str` suffixes, the value and ARIA numbers via `write!`), and text goes through `text_ref`, so a refresh with no visible change costs one hash of the visible window and a repeat refresh writes nothing. Area: shell quick-control Audio popup projection (volume drag events, mute toggles, tab/scroll changes, idle ticks). Expected composition shift: the audio projection's share of a shell event drops from ~24 heap allocations plus ~100 property lookups to zero allocations on a changed refresh and a single digest comparison on an unchanged one, with identical text, visibility, geometry and ARIA state. Scoped pure tests cover the digest and the all-writes-skipped repeat projection. Files: crates/flamewm-shell/src/projection/audio.rs, docs/perf-loop/LOG.md.

## u145 (terminal UI projection allocations): `project_font` cloned `font_family` into a tuple before comparing, so every frame allocated a `String` even when the font was unchanged; it now compares size and family in place and clones only on a real font change. `project_row` cloned the previous row handle (an `Arc` refcount) and then built a fitted copy of that previous row via `fit` to diff slots, which allocated a `Vec` plus merged text for every changed row that overflows the pool; the previous handle is now moved out with `take` (stored back once) and slots are compared against the unfitted previous row by the pure `slot_matches`/`prev_slot_eq` helpers, which reproduce `fit`'s tail merge without allocating (and treat a later merged run's colours as irrelevant, matching what the merged slot actually draws). `write_run` copies run text with `push_str` when it contains no ASCII space instead of decoding and re-encoding every char for the no-break-space substitution. Area: terminal per-frame projection (typing, htop-style ticks, scroll). Expected composition shift: the terminal paint share loses one `String` allocation per frame, the previous-row `Arc` clone per row, and the per-changed-row fitted `Vec` in the overflow path (dense coloured TUIs); glyph text writes are unchanged, so visuals and pooling behaviour are identical. Pure tests added for the allocation-free slot comparison under the tail merge. Files: crates/flamewm-terminal/src/ui.rs, docs/perf-loop/LOG.md.

## u144 (shell network popup projection): every network event rebuilt the popup text from scratch. `project_network_full` called `taskbar::status::network::project`, which builds the whole `StatusViews` set just to read `visible` (the availability matrix, i.e. `availability != Unavailable`), and `popover_view`, which builds a full `NetworkPopoverView`: a cloned pending-secret path, a cloned connected label and up to eight `NetworkRowView`s with cloned `path`/`label`, whose `visible_rows` were unused because the slot window comes from `project_visible_rows` again. Both are gone: the flag is computed from the snapshot and the popover model alone answers scan/disconnect, with the pending-secret path borrowed from `network.pending_secret`. The slot loop now borrows `row.label` through `text_ref` instead of `clone_from` and writes the three ids and the status line into four reused scratch strings (no per-slot `format!`), so only the visible window is formatted; `connection_sentence` returns `Cow<'static, str>` (static phrases borrowed), `project_network_secret` borrows an already-masked run of `*` instead of allocating a second copy, `project_network_focus`, `project_vpn_rows` and `project_network_status` reuse buffers/borrow, `project_bluetooth_row` borrows its status, and `bluetooth_device_names` builds one `String` instead of a `Vec` plus a join. Area: shell quick-control Network popup projection (each network event and each keystroke in the search box). Expected composition shift: a network projection drops from ~60 heap allocations to ~10 (allocation count, scoped pure tests only; no absolute CPU claim) with identical text, visibility and tile/toggle visuals. Files: crates/flamewm-shell/src/projection/network.rs. `cargo check -p flamewm-shell --all-targets` green (1 pre-existing dead-code warning in launch_feedback.rs).

## u143 (shell audio popup projection): `project_audio_full` rebuilt every row id with `format!` and every value with `format!`/`to_owned` on each audio event and each quick-control tick (about ten ids plus a percent and two ARIA numbers per row, and the raise-track rewrite), so an unchanged popup still paid ~24 heap allocations and ~100 node lookups. Ids are now built once per row in one reused capacity-reserved `String` (`truncate(base)` then `push_str` per suffix; the track id serves its four ARIA writes), the percent and ARIA values are formatted into a second reused buffer and written through `text_ref`, and the whole projection is guarded by `audio_projection_digest` (tab, `raise_maximum`, live accent, and each visible row's id/label/volume/muted/kind): an identical repeat on the same thread returns before any write, since the quick-control host keeps one audio document per process and the document already holds those values. Added pure digest tests (equal model => equal digest; any painted change => different) and a scoped test that a repeated projection records zero writes while a changed volume rewrites the row. Area: shell audio quick-control popup projection (volume events, mute, tab switch, idle ticks). Expected composition shift: a no-op audio refresh drops from one write pass over every row property to one hash over the visible rows (no allocation); a real change still rewrites only the visible window, with the same text, visibility, slider geometry, ARIA attributes and accent. Files: crates/flamewm-shell/src/projection/audio.rs.

## u143-u145 shell audio/network projection, terminal ui allocs
- Skip identical snapshots and drop per-refresh clones in shell audio/network projection and terminal ui row projection.
- Review fixes: notification-centre focus index off by one (0-based slot, underflow in debug), switcher thumb cache kept referenced previews, INDICATOR_FRAME test-only, clippy cleanups.
- Evidence: workspace clippy -D warnings clean; shell/terminal/explorer/wm-wayland tests green.

## u147 (downloads-core poll): `POLL` (100 ms) is not dead — the only consumer is `flamewm-desktop/src/downloads.rs` (`arm`), which builds the next settle deadline as `now + POLL`; nothing else in the workspace reads it, so it was kept. The consumer already satisfies the park-when-idle rule: `arm` returns through `disarm` when `tracked` is empty, clearing the organizer's private wake slot, so the 100 ms cadence runs only during the bounded settle window of a tracked candidate (at most `STABLE_AFTER` = 1500 ms, about 15 stats per download) and no timer exists while Downloads is idle; the batch/partial names that never settle (`.crdownload`, `.part`, ...) are rejected by `is_candidate_name` before they can be tracked. Change: documentation only, recording that contract on the constant so a future consumer cannot arm a permanent 100 ms poll. Area: downloads organizer settle timer (idle path). Expected composition shift: none — an idle or disabled organizer already arms no wakeup; the settle-window share is unchanged, and the note keeps it from regressing. Files: crates/flamewm-downloads-core/src/lib.rs, docs/perf-loop/LOG.md.

## u146 (session supervisor readiness tick): `TICK` (100 ms) was the only sleep-poll left in the session supervisor, and its single consumer was the WM-readiness wait (`wait_ready`), which slept 100 ms between `_NET_SUPPORTING_WM_CHECK` probes, so a slow WM boot cost ten wakeups and ten fresh X11 connections per second. The supervisor's steady-state loop was already event-driven (`wait_for_wakeup` on the SIGCHLD/stop self-pipe with the 1 s missed-wakeup bound) and was left alone. `wait_ready` now keeps one X11 connection through `ReadinessWatch`, selects `PropertyChangeMask` on the root window before the first probe, and blocks in `poll(2)` on that socket plus the supervisor wakeup pipe, so it wakes exactly when the WM publishes readiness, a child exits, or a stop signal arrives; `MINIMUM_WAIT` drains signal bytes and `readiness_wait_timeout` bounds each block by the remaining `READY_TIMEOUT` budget and by `SUPERVISOR_SAFETY_TIMEOUT`, which stays only as the dropped-signal fallback. The EWMH probe body was split into `ewmh_ready_on(&connection, screen)` so the watch and the one-shot path share it; a broken watch socket is dropped and reopened on the next pass instead of spinning. Restart, crash-loop, control-bootstrap, no-bus and readiness-timeout behaviour are unchanged. Area: session supervisor WM readiness (login and WM restart). Expected composition shift: the readiness window's wakeup share drops from ten timer wakeups plus ten X11 connections per second to event-driven wakeups (one cheap 1 s re-check only as fallback); steady state still costs zero periodic wakeups, and login latency improves because readiness is seen on the property event instead of up to 100 ms later. Files: crates/flamewm-wm/src/bin/flamewm-session.rs, crates/flamewm-wm/src/bin/flamewm-session/readiness.rs, docs/perf-loop/LOG.md. Pure test covers the wait-timeout choice; no cargo run in the unit.

## u146-u147 session readiness wait, downloads POLL doc
- Session supervisor readiness replaced the 100 ms sleep-poll with a poll() on an X11 root-property watch plus the signal wakeup pipe (1 s safety bound; 100 ms retry kept once EWMH is up because the control socket has no event source). Idle supervisor wakeups during startup drop from 10/s to event-driven.
- downloads-core POLL is armed only while a candidate settles (documented); no behaviour change.
- Evidence: clippy -D warnings and flamewm-wm/downloads-core tests clean.

## U149: Window material compositor read
- XWayland publishes compositor backdrop support and reads per-window opacity/blur material for managed X11 windows.
- Material values default and clamp safely; property changes refresh cached values and request redraw only when changed.
- Regression tests cover malformed and clamped values; native Wayland windows retain `(100, 0)`.

- u148-u150 window backdrop under Wayland: apps publish `_FLAMEWM_WINDOW_MATERIAL`, the compositor sets `_FLAMEWM_COMPOSITOR_BACKDROP` and draws a blurred scene-beneath backdrop for translucent windows. Needed a smithay patch (third_party/smithay) so depth-32 clients get ARGB frames. Nested frame check: xterm text blurred through the translucent task manager.

## u154 (frame callbacks)
- Already implemented: send_frame_callbacks skips hidden and off-output windows (render/mod.rs:984). Occluded-window skip deliberately not added: a covered window that stops getting callbacks stalls its commit. Composition shift: none.

## u151, u153 (audits, no code change)
- u151 terminal: dirty-line reuse and batched PTY reads already in place (view.rs, app/grid.rs, session.rs).
- u153 explorer: transfers.rs stats only unknown paths plus one destination stat per user drop; left synchronous on purpose. Composition shift: none.

## Idle composition sample (scripts/perf-clients, nested, SETTLE=6, 2 s window, 100 ticks = 1 s)
- explorer startup 10 / idle 0 / 21 MB; terminal 14 / 0 / 30 MB; shell 13 / 0 / 22 MB; task-manager 32 / 4 / 31 MB.
- Only idle cost is the task manager's 1 s /proc sampler (about 2% of one core, 100% of its own samples). Everything else parks.
- Next: shrink the sampler cost (skip unchanged per-pid reads, reuse buffers) without losing rows.

## u157 (nested scripted drag) and u156 measurement
- Scripted titlebar drag works in the nested harness; live-content drag shows the window following the pointer with the blurred backdrop strip following it (no visible lag artefact in the frame).
- Measured: 90 steps took 6.9 s (~76 ms/frame, software GL) and made 73 backdrop captures. The 50 ms reuse window rarely applies when frames are slower than 50 ms, so u156's saving is NOT confirmed in this harness; on a 60 Hz GPU it should reuse about 2 of 3 frames (unmeasured). Follow-up: make the throttle frame-count based (reuse up to 2 consecutive frames) so it also helps when frames are slow.
- Default drag mode is outline-only (showContentWhileDragging off), which does not recapture at all.
- Regression caught by running the app: u152 panicked the task manager at startup (RefCell double borrow), fixed in 67819329. Lesson: run the app, not only tests and clippy, after UI-closure changes.

## u156 follow-up: frame-count throttle
- A moving window now also reuses its backdrop for up to 2 consecutive frames when frames are slower than 50 ms (MAX_STALE_FRAMES).
- Measured with the scripted drag (90 steps, nested software GL, one run each): captures 73 -> 60, drag elapsed 6.9 s -> 5.4 s. Smaller than the ~30 captures expected: the reuse also requires the scene stamp beneath to match, and it changes as the window slides over different elements. Single runs, so noise is unquantified.
- Next: let the scene check ignore the moving window's own motion over unchanged elements.

## u156 follow-up 2: drop the scene check for moving windows
- The moving-window reuse no longer requires the scene stamp beneath to match (a sliding window crosses new element edges every frame); staleness is bounded by 50 ms or 2 frames, and every reuse requests a redraw so a recapture at the final position always follows.
- Measured, same scripted drag (90 steps, nested software GL, single runs): captures 73 -> 60 -> 24, drag elapsed 6.9 s -> 5.4 s -> 3.3 s. Final frame (drag9.png) shows the window at its end position with the blurred backdrop strip as in the static layout; no stale backdrop seen.
- Risk: while moving, content beneath that changes (e.g. a video) can look up to 2 frames/50 ms stale; accepted.

## Scout: timers and sleeps (code reading)
- Swept crates for short timers/sleeps: the `thread::sleep` hits in reactor and supervisor are test-only. The one runtime 15 ms poll is the notification icon wait (u158): bounded by the icon load in the normal case, unbounded if the loader never answers. No hot-path change made.
- Scout, explorer places/GVFS (code reading): `gvfs_shares` already uses d_type (no per-entry stat on FUSE) and `gvfs_root` caches the uid; nothing to change. Sidebar rebuild is triggered by mountinfo/block/watch events, not a timer. Not yet checked: whether `places()` (reads mountinfo) runs on the UI thread, and trash.rs:98 listing (reads every .trashinfo) on open of the trash view.
- Scout closed, explorer: `reload_places` runs `build_places` through `spawn_background` (lifecycle.rs:31) once the event loop exists (sync only at startup/no-events fallback); `trash::list` runs in workers.rs. Both off the UI thread. Nothing to change.
- Scout closed, terminal PTY read path (session.rs `read_loop`): blocks in `poll` when idle (no wakeups), at most one wake byte per FRAME, drains the kernel buffer with FIONREAD into one batch up to BATCH_BYTES, recycles buffers from the UI. Nothing to change. Not yet scouted: terminal grid dirty-line rendering and scrollback bounds.
- Scout closed, terminal grid and scrollback (code reading): the screen keeps a dirty-row bitmap and `view.rs` rebuilds only dirty rows between frames; scrollback is a capped `VecDeque<History>` where each line is packed (Plain / Uniform / Packed runs) instead of a Cell per column. Nothing to change. Terminal is closed for this pass; next: task manager per-frame work and the compositor's per-frame allocations.
- Scout closed, task manager `paint_table` (lib.rs:574-673): shared id/value buffers, per-slot `RowText` cache, `text_ref` dedupe, slot visibility restated only when the count moves. Residual: ~14 document calls per visible row per paint (id format + lookup of unchanged cells), paint runs at the 1 s sample, so the share is small; a per-row "unchanged since last paint" skip would save the calls but is not worth the added state. Nothing changed. Remaining unscouted: compositor per-frame allocations (render/mod.rs build_elements, send_frame_callbacks).

## Compositor DRM scene rebuild: build into the cursor vec
- drm/mod.rs: a full scene rebuild did `elements.extend(build_elements(..))`: a throwaway Vec plus a copy of every element per rebuild. It now calls `build_elements_into` on the cursor vec (the winit path already did). `build_elements` had no remaining callers and is removed.
- Scope: rebuilds happen only on scene changes (pointer-only motion splices the cached cursor slice), so the saving is one allocation and one element copy per scene change, not per frame. Verified by clippy `-D warnings` and 450 unit tests; the DRM path is not runnable in the nested harness, so no frame was read.
- Compositor scout otherwise: `scene_elements_into` already skips windows outside the output or fully occluded, and frame callbacks skip hidden/off-output windows.

## Measured composition attempt: thread level is too coarse (no result)
- `scripts/perf-compose` on the nested compositor (the real flamewm-wayland pid, not the `timeout` wrapper, which gives a false "idle: 0 ticks"): idle sample and drag sample both read 100% `flamewm-wayland` main thread. The compositor does its work on one thread, so thread names cannot split it into subsystems, and the output normalises to shares so it cannot show whether idle is near zero.
- Caveat: the harness xterm runs `yes`, so it floods output and the "idle" sample is not idle.
- Function-level profiling needs `perf record`, which is installed but blocked (`perf_event_paranoid=4`). Options: lower it with sudo (user decision), or add scoped timing counters around the frame stages (backdrop refresh, element build, draw pass, frame callbacks) in the winit path and print their shares, like the existing `backdrop_captures` counter. Next unit if pursued: u159 stage timers.

## u159 stage timers: first measured composition of the compositor frame
- `render/stage_timers.rs`: per-stage nanosecond counters around the winit paint (desktop backdrop, dock backdrops, elements, readbacks, draw, submit, callbacks), printed in the `SCRIPT_DRAG` line after a reset at drag start.
- Measured, scripted live-content drag, nested software GL, two identical runs: 89 frames, 2.73 s of compositor frame time: dock_backdrops 70%, elements 27%, draw 1%, submit/callbacks/desktop_backdrop/readbacks ~0%. Drag elapsed 3.4 s, 25 captures.
- So the ranked targets are the backdrop capture itself (~75 ms each, u160: split into render/readback/blur/crop) and the element build (~8 ms per frame, u161). Drawing is negligible here (software GL with damage-limited repaint). Caveat: nested software rendering; on a real GPU the capture's GPU stages will weigh differently, but the CPU blur and element build do not.

## u160 backdrop capture: crop_capture was 64% of a capture
- Profiled with the existing `FLAMEWM_BACKDROP_PROFILE=1` phases (now also emitted by `capture_dock`: texture/render/copy/map/downscale/blur/crop/store). Average per dock/window capture, scripted drag, nested software GL: total 96.7 ms = crop 61.5 + map 15.5 + store 10.6 + blur 4.7 + downscale 2.6 + copy 1.2 + render 0.5.
- `crop_capture` (backdrop.rs) did a float bilinear per output pixel with per-pixel clamp/divide and a 4-byte `extend_from_slice`. Rewrite: per-column index/weight table computed once, per-row source rows hoisted, vertical blend done once per row (bilinear is separable), pre-sized output written by chunks. Output matches the old per-pixel formulation within 1 per byte (test against `crop_capture_reference` on 6 geometries, plus the existing crop test).
- Measured (same drag, one build each, machine load ~16 so absolute numbers are noisy): crop 61.5 -> 41 ms (hoisting only) -> 28.4 / 25.4 ms (two runs, separable); capture total 96.7 -> 59.9 / 56.7 ms. The final frame (q2.png) shows the same window position and blurred strip as before; no visible change.
- Next in u160: `map` (GPU readback map, ~14 ms) and `store` (alpha mask loop + premultiply + buffer upload, ~9.5 ms, scalar per pixel) are now the next largest; `crop` is still the biggest at ~26 ms.

## u161 element build: outline ring was re-rasterised on every move
- Added element-build detail timers (stage_timers: outline / surface / window_backdrop / titlebar, skipped on the backdrop-capture scene build). Drag, 2 runs, before: elements = 31% of compositor frame time (~9.6 ms/frame); detail: outline 62% / 59%, titlebar 35% / 36%, surface and backdrop ~0-3%.
- Cause (titlebar.rs `OutlineRing::matches`): the cache key included the frame origin, so every frame of a drag re-rasterised the outline strips and uploaded new buffers. The strips are offsets from the frame origin and the raster takes only size, radius, width, colour and alpha, so position is not an input. Removed `origin` from the key and the struct; the test now asserts a move reuses the strips (builds stays 1) and resize/radius/settings still rebuild.
- Measured, same scripted drag, 2 runs after: outline 0%; element-build time 876/751 ms -> 360/392 ms; elements share 31% -> 18-20%; total compositor frame time 2766/2394 ms -> 1945/1923 ms; captures 27 -> 23. Frame check: 0 differing pixels (fuzz 3%) along the window's top and right edges between the before and after dumps, and the 1 px outline pixel (#606060 at y=284) is present in both.
- Not changed, deliberately: the titlebar buffer bakes the blurred desktop cropped at its origin, so a move legitimately rebuilds it. It is now 91-93% of the remaining element-build time (~4 ms/frame); candidate next: reuse a titlebar for up to 2 frames or 50 ms while only its position changes, like the u156 window-backdrop rule (visual risk: the tint under the titlebar lags the desktop behind it for a frame or two; needs a frame check).

## u160 capture store step: opaque fast paths
- `store` (window/dock capture, ~9.5 ms) ran the silhouette mask function for every pixel and built the premultiplied buffer with `flat_map().collect()` (no pre-size). Now: window rows clear of the top/bottom corner radii get alpha 255 without evaluating the mask; `premultiplied_argb_bytes` writes into a pre-sized buffer and, for alpha 255, only reorders channels (`(255c+127)/255 == c`). Test: every channel x alpha pair against the formula (65,536 cases).
- Measured (same scripted drag, nested software GL): store 9.5 -> 6.6 ms (run 1; run 2 gave 5.4 ms but painted 0 frames during the drag, an anomaly I did not chase, so I treat it as one clean run); capture total ~58 -> 51 ms (44 in run 2). Frame check against the previous dump: 0 differing pixels in the window's rounded top-right corner (exercises the mask) and along the top edge band.
- Remaining per-capture cost (ms): crop ~24, map ~13, store ~6.6, blur ~4, downscale ~2.6.

## u160 crop_capture in 10-bit fixed point
- The inner loops did an f32 lerp plus `round().clamp()` per channel (a libm call per sample on the baseline x86-64 target). Now both blends use 10-bit integer weights: vertical blend into a `Vec<u32>` row (<= 255<<10), horizontal blend <= 255<<20 fits u32, one `(x + 2^19) >> 20`. Worst-case error vs the float reference < 0.75/byte; the existing equivalence tests (6 geometries, tolerance 1) still pass.
- Measured, same scripted drag, 2 runs: crop 24 -> 8.2 / 8.1 ms per capture; capture total 51 -> 35.4 / 35.2 ms; compositor frame time 1697 -> 1267 / 1234 ms (frames 89, captures 22).
- Frame check: titlebar band (blurred crop) bit-identical (fuzz 0) to the previous dump; full-frame diff is 1833 px (0.2%) but the same count appears between two runs of the previous build (1834), so it is live task-manager content, not the crop.
- Remaining per capture (ms): map ~12 (GPU readback), store ~7.3, blur ~4, downscale ~2.6, crop ~8.2.

## u160 readback map: not reducible in this harness
- Read `read_reduced`: `map` (12 ms) is where the software GL finishes the queued scene render, so it measures llvmpipe, not transfer cost. No honest CPU-side win exists; a GPU-side two-step 2x blit (exact 4x4 box) would cut real-GPU readback bytes 16x but must be measured on the DRM backend. Written up as u162.
- Open from u160/u161: titlebar rebuild on move (~4 ms/frame), store ~7 ms, blur ~4 ms.

## u161 titlebar reuse while only its position changes
- The titlebar bakes the blurred desktop crop at its origin, so every drag frame rebuilt it (91-93% of the remaining element-build time, ~4 ms/frame). Now a titlebar whose width/active/maximized/rounded/title are unchanged and only whose origin moved keeps its pixels for up to 50 ms or 2 frames (same rule as the u156 window backdrop); it is still placed at the new position, only the blurred tint under it lags. Each reuse sets `Decorations::stale_reuse`, and the render loop requests one more frame, so the final position always gets a fresh crop (checked below).
- Measured, same scripted drag, 2 runs: element-build time 360/392 -> 67/62 ms; elements share 19% -> 6-7%; total compositor frame time 1267/1234 -> 1020/850 ms (frames 88-90, captures 22).
- Frame check: end-of-drag titlebar band is bit-identical (fuzz 0) to the previous builds' dumps, so the settled frame is rebuilt at the final position. The mid-drag tint lag was not captured frame by frame (the harness dumps once, at the end); it is bounded to 2 frames / 50 ms.
- Not covered by a unit test: `reused_ok` and the `Decorations::element` reuse decision need a GL renderer or a ChromeBuffer fixture; candidate follow-up.
- Dock backdrops are now 87% of the compositor frame (captures); the remaining levers are in u160/u162.

## u160 fused alpha-mask + premultiply store pass
- `store` made three full-size passes over a capture (mask into the RGBA buffer, `premultiplied_argb_bytes` into a second buffer, then `MemoryRenderBuffer::from_slice` copies). The first two are now one pass writing premultiplied BGRA directly (`premultiply_pixel`; opaque window rows only reorder channels). Test: `premultiply_pixel` equals `premultiplied_argb_bytes` for 256 alphas x 6 channel values.
- Measured, 2 runs: store 7.3 -> 4.05 / 4.03 ms. Caution: crop (untouched) also read lower (8.2 -> 5.5 ms) so the machine was quieter this run; treat total_us (35 -> 24 ms) as partly noise, the store drop as the change. Composition: dock_backdrops 87% -> 85% of frame time.
- Frame check: titlebar band and window top-right corner 0 differing pixels vs the previous dump (fuzz 0); full-frame diff 1739 px vs 1715 px between two runs of the same build (live TM content).
- Remaining per capture (ms, this run): map ~9.9 (renderer wait, see u162), crop ~5.5, store ~4.0, blur ~4, downscale ~2.6.

## u160 capture blur reuses its float work buffers
- `blur_reduced` built a fresh `BlurScratch` (several Vec<f32> work buffers) per capture. It now uses a thread-local scratch on the render thread (output identical, same library path; 457 tests green; titlebar band 0 differing pixels).
- Measured: NOT distinguishable from noise. This run the whole machine was ~1.7x slower (crop 5.5 -> 9.7 ms, store 4.0 -> 7.0 ms, both untouched), blur read 3.6-3.7 ms vs ~4 before. Kept as harmless allocation-churn removal, not counted as a win.
- Larger lever found while reading: the blur runs on a region padded by 2*radius on every side (~2x the area for small captures). Written up as u163.

## u158 notification icon poll is bounded
- Area: shell idle wakeups. While a toast's named-icon request was outstanding, `poll_icon` re-armed a 15 ms wake (66 Hz) until a matching raster arrived; a dropped or mismatched answer meant that polling for as long as the toast was visible. `icon_wait` now carries the request time and expires after 2 s (`icon_wait_expired`, unit-tested including a backwards clock), keeping the fallback glyph. An icon that arrives in time takes the unchanged path.
- Composition shift: bounded worst case 66 Hz idle wake -> at most 133 wakes per toast; typical case (icon arrives within a few ms) unchanged. Not measured at runtime: the lost-answer path was not reproduced, so this is a bound, not a measured saving.
- Also logged: u163 sized (blur padding ~1.6x area, saves ~1.5 ms/capture, changes edge pixels, deferred).

## u164 content-only backdrop refresh is capped by its own cost
- New idle-composition probe: scripted drag of zero distance (`SCRIPT_DRAG=700,106,700,106,30,8000,3000`) = a stationary translucent window over a terminal printing continuously. Before: 53-78 captures in 3 s (every frame), 70-90 painted frames, dock_backdrops 93-94% of compositor frame time. Cause: when only the scene beneath changed, `refresh_dock_backdrops` recaptured on every frame with no rate limit, so the ~35 ms CPU-blurred capture set the frame rate.
- Change: `DockBacking.cost` (wall time of its capture) and `refresh_due`: a window/dock whose fields are unchanged but whose scene beneath changed is recaptured at most once per 2x its own capture cost (cap 250 ms); while waiting the render loop requests another frame, so the newest scene is always captured once the interval passes. Capture work is bounded to about a third of wall time at any machine speed. Unit test covers the 2x rule, the cap, zero cost and a backwards clock.
- Measured (2 runs after): captures 31-39 per 3 s (-40..-50%), painted frames 127-137 (was 70-90: frames are no longer starved by captures), total frame time 1281-1864 -> 997-1058 ms. dock_backdrops share is still 87-90% because each remaining capture is expensive (the u160/u162/u163 levers).
- Visual cost: backdrop under a translucent window refreshes at ~14 Hz in this software harness (2 x ~35 ms), faster where captures are cheaper; the blurred haze lags the content beneath by at most that interval. Frame check: end-of-run frames read side by side (old vs new), same frosted glass, same haze position and tint.

## Compositor re-rank: idle baseline (nested software GL, 3 s, task manager only)
- Probe: `FLAMEWM_WAYLAND_AUTOSTART=tm.sh`, `SCRIPT_DRAG=450,74,450,74,30,8000,3000` (stationary titlebar press; the TM updates once a second).
- Result, 2 runs: 33 frames, 0 backdrop captures, total compositor frame time 13 / 18 ms over 3 s (~0.5% of one core); composition draw 71-76%, elements 9-13%, submit 9%, dock_backdrops 1%.
- Ranking of compositor states by cost (software GL, per second of that state): drag over a busy window (captures, ~25-35 ms each, bounded at ~1/3 wall time by u164 for stationary windows and by u156 reuse for moving ones) >> stationary translucent window over changing content (u164) >> idle (negligible). Remaining levers are all capture cost: map/readback (u162, needs a real GPU), blur padding (u163), crop/store residual.
- Next ranked area: none in the compositor at idle; move to the apps' own idle wakeups and the capture residual on a real GPU session when one is available.

## Apps' idle wakeups: scouted, nothing to fix
- Grepped explorer, terminal, shell and task manager for fixed sub-50 ms timers and sleeps in non-test code. Hits and what they are: explorer `remote.rs` 50 ms `recv_timeout` (only inside a running remote transfer), explorer/terminal scroll `FRAME` 16 ms (only while a scroll animation runs), explorer `search.rs` 5 ms sleeps (only while a search is active), terminal `pty.rs` `HANGUP_POLL` 5 ms (only while a child is exiting; blocking wait is used where the kernel supports it). None run at idle. Task manager sampler already sleeps on a condvar while obscured.
- Shell: the one idle-reachable poll (toast icon, 15 ms) is now bounded (u158).
- Conclusion: app idle wakeups are not a ranked hotspot. The remaining measurable costs are the compositor capture residual (u162/u163). Next scout: RAM (caches without caps, duplicated assets) per app.

## Apps: measured RAM and idle CPU (scripts/perf-clients, SETTLE=8, nested)
| app | threads | RSS | idle CPU ticks in 2 s |
|---|---|---|---|
| explorer | 14 | 21.5 MB | 0 |
| shell | 4 | 22.4 MB | 0 |
| terminal | 3 | 29.4 MB | 0 |
| task-manager | 2 | 30.8 MB | 5 (25 ms/s, ~2.5% of a core, 100% in the sampler thread) |
- RAM is small and even (22-31 MB); no cache stands out, so no RAM unit is ranked. The only idle consumer is the task manager's 1 Hz sampler (581 processes): already buffer-reusing, one `stat` read per task (RSS from it), bounded `status` read, sleeps while obscured; the remaining ~25 ms/sample is the per-process open/read/close syscalls, which a visible process table needs. Not changed.
- Ranking now: compositor capture residual (u162/u163) > task-manager sampler (~2.5% core while visible) > everything else at 0.

## Reliability sweep: runtime unwrap/expect inventory (release is `panic = "abort"`)
- Method: `cargo clippy -p <crate> -- -W clippy::unwrap_used -W clippy::expect_used` (no `--all-targets`, so inline test modules are excluded and counts are real runtime sites).
- Compositor (`flamewm-wm-wayland`): 2 sites, `xwayland.rs` `xwm_state` and `session_lock.rs` `lock_state`. Both implement library trait methods that must return `&mut` (no error path possible) and document their invariant (XWayland handlers fire only while the X11 WM runs; the lock global is created at startup). Left as is.
- Apps (explorer, terminal, shell, task manager): ~45 hits are `build.rs` (build-time, fine). Runtime: explorer `merge_refresh` (4 `expect("peeked row")`), explorer `scroll.rs` (`checked_sub().unwrap()`), shell-core calendar (`try_into().expect("42 cells")`).
- Fixed: `merge_refresh` now matches on the peeked rows (no panic path, same merge semantics) and has its first tests (identical, added/removed rows, leftover rows on either side, empty cases); `ResizeGate::offer` uses `Duration::saturating_sub` (identical value in the guarded branch). The calendar expect is a constant-size loop product, left as is.
- No performance change; reliability only. 252 explorer tests green, clippy `-D warnings` clean.

## u165 terminal output flood is backpressured (RAM bound)
- Finding (measured with a throwaway probe test): the PTY reader thread sent every batch on an unbounded channel and never paused. With nothing draining the channel, `yes` queued 18 MB in 2 s (debug build, PTY-throughput bound). The UI parses under a per-wake budget and leaves the rest queued on purpose (`output_past_the_budget_stays_queued...`), so a sustained flood (`yes`, `cat` of a huge file) made the queue, and RSS, grow for as long as the flood lasted.
- Change: `Session.queued` (Arc<AtomicUsize>) counts output bytes sent and not yet parsed; `App::session_event` reports each parsed batch via `Session::consumed`; the reader pauses (sleeping one `FRAME`, while its frame-cadence wake keeps going so the UI always drains) whenever `queued >= MAX_QUEUED_BYTES` (4 MiB, ~16 ms of a fast flood). The PTY then fills and the child blocks: ordinary flow control, no output is dropped.
- Tests: new `an_undrained_flood_is_capped_and_resumes_when_the_ui_catches_up` (queued <= cap + one batch, no growth while paused, resumes after `consumed`); the existing flood test now reports consumed like the UI does (it had exactly a 4 MiB flood = the cap). 210 terminal tests green, clippy `-D warnings` clean.
- Not measured end to end in the nested compositor (the harness cannot type `yes` into a pane); the throughput path is covered by the flood test (4 MiB, frame-cadence wakes, no byte lost) still passing within its time budget.
- Composition shift: worst-case terminal RAM during a flood is bounded to ~4 MiB of queued output plus recycled buffers instead of unbounded.

## Lock scan (compositor, explorer): nothing ranked
- Listed every non-test `.lock()` site. Compositor locks guard small in-memory state (font glyph cache: one rasterize per cache miss, bounded by `CACHE_LIMIT`; control queues; surface data). Explorer `chunk_for_device` releases its map lock before the sysfs read; thumbnail/search/journal locks wrap map updates only.
- No lock held across disk, network or a blocking call found. No change.

## Hidden-window scout (task manager, shell): nothing ranked
- Task manager: the sampler parks on `SampleGate` while hidden (no timer, no /proc walk), wakes at once on show, sleeps on the action channel between 1 s samples. Already optimal.
- Shell: periodic work is all slow (usage 300 s, weather 30 min, heap trim 10 min, reproject 60 s) or short-lived animation (launch feedback 50/250 ms polling only while a launch indicator is live, OSD hold/fade). No always-on short timer found.
- No change.

## Explorer scroll/thumbnail scout: nothing ranked
- Thumbnail cache is bounded three ways (256 entries, byte cap, source <= 25 MB / 25 Mpx). Projection ids are pre-built once per slot (no per-frame `format!`). No change.
- CI note: the run for 37d8407c failed in "Install native build dependencies" (apt exit 100, runner flake, no build or test ran); later commits carry the same code and are the evidence.

## u161 follow-up: titlebar reuse rule is unit tested
- The moved-titlebar reuse decision is now the pure `stale_reuse_allowed(age, reused)`; `ChromeBuffer::reused_ok` calls it (with `saturating_duration_since`, so a clock step cannot panic). New test covers young, over-budget and past-both cases. No behaviour change. 459 compositor tests green, clippy clean.

## Reliability: calendar grid has no panic path
- `shell-core` calendar built its 42 cells with `try_into().expect(...)`; the constructor already returns `Option`, so a wrong length now yields `None` instead of aborting the shell (`panic = "abort"`). Same result for every valid month. 143 shell-core tests green, clippy clean. This was the last runtime unwrap/expect found by the sweep outside the two documented compositor trait impls.

## u160 capture downscale: row-wise accumulation
- `downscale_strip` (box average of the GPU readback to the stored 1/4 image) now accumulates one source row at a time into a per-stored-row sum buffer and divides once per stored pixel, instead of a nested per-block walk with a per-channel division. Rounding and partial edge blocks are identical: a new test compares it with the old formula for 16x16, 17x13, 5x9, 1x1 and 7x4 images split into strips.
- Measured (90-step drag, release, nested software GL, 108 vs 238 captures): downscale 2601 -> 1819 us/capture. The untouched blur phase moved 3609 -> 3114 us in the same runs (machine speed), so the gain net of that control is about 19%, not 30%. Capture total 39.7 -> 28.2 ms is mostly machine noise and not claimed.
- Composition: downscale was ~6.5% of a capture; this trims it by roughly a fifth, i.e. ~1% of a capture. Small, safe, tested. 460 compositor tests green, clippy clean.

## Capture crop scouted: at CPU floor
- Read `crop_capture` again: nothing left to hoist. Remaining cost is the full-resolution bilinear upscale itself; folded into u162 (GPU does the upscale). No code change.

## Duplicate dependency scan: nothing first-party
- `cargo tree --workspace -d`: duplicates (calloop 0.13/0.14, x11rb 0.13/0.14, digest/sha2, hashbrown, thiserror 1/2, rustix 0.38/1.x, getrandom, syn 2/3) all come from third-party pins: winit 0.30 and smithay-client-toolkit (older calloop, x11rb), vendored smithay (sha2), fontdue (hashbrown). No first-party crate causes one.
- Not changed: removing them means patching vendored smithay/winit for a few hundred KB of binary and no runtime share. Revisit if winit moves to the newer calloop.

## Binary size scout: terminal 14 MB is by design
- `flamewm-terminal` is the largest binary (14.0 MB vs compositor 8.9, shell 7.4, explorer 5.4, task manager 3.6) because it embeds three compiled UI tiers (0.75, 2.4 and 7.2 MB, 24x80 / 48x128 / 96x192 run slots). A window decodes only the smallest tier that fits its grid (`pool_tier_for`); the other tiers are untouched read-only file pages and never resident. Measured RSS stays 22-31 MB (per-app table). Disk size only; no runtime share. No change.

## Reference pass: KWin and other desktops (user request)
- Cloned `.vendor/kwin-master` (sparse) and compared KWin blur, scenefx optimized blur (mangowm) and COSMIC `ext_background_effect`. Findings and how they map to u162/u163 are in `docs/perf-loop/REFERENCE-TRANSPARENCY.md`. Our desktop-level cached backdrop already matches scenefx; the open gap is KWin's GPU pyramid + damage-clipped blur (u162).

## Explorer thread count: parked, no change
- 14 threads at idle are the job/scan worker pools (`workers.rs`): each blocks on `recv` of a shared queue (a send, not a spawn, starts work), and `perf-clients` measured 0 idle ticks in 2 s for the explorer. Threads cost stack address space, not CPU or resident RAM; RSS is 21.6 MB.

## u162 measured on real hardware: the readback is the top capture cost
- Correction: the nested harness runs on the machine's Intel GPU (`FLAMEWM_GL_RENDERER ... Mesa Intel(R) UHD Graphics (TGL GT1) class=hardware`), not llvmpipe, so u162 is measurable here. Earlier notes calling it software GL were wrong.
- New profile-only split: `PhaseClock::gpu_wait` waits on the render's `SyncPoint` (never when profiling is off) so `gpu_wait` is the GPU finishing the scene and `copy`/`map` are the readback alone.
- 90-step drag, 48 phase lines (busy machine, read the shares): total 83.4 ms; map 35.8 (43%), crop 20.9, store 11.7, blur 6.4, gpu_wait 5.0, downscale 2.1, copy 1.5, render 0.4. `gpu_wait` is small, so `map` is the transfer of the full-size readback, not waiting for the scene. Reading 16x fewer bytes is the lever.

## u162 GPU-side capture downscale (done)
Capture total 83.4 -> 26.5 ms on real Intel GPU (map 35.8 -> 2.6 ms) by halving twice on the GPU and reading back 1/16 of the bytes. Output within 1 level/byte of the CPU path (55 live captures). clippy -D warnings and 460 wm-wayland tests pass. Next: crop/store (~14 ms) via GPU upscale.

## Scout after u162
Capture residual: crop 30%, store 23%, blur 12%, gpu_wait 13%, map 10%. Next unit u166 (GPU upscale of the blurred backing) written.

## Task manager idle tick check
`scripts/perf-clients` shows 3 ticks/2 s: the visible 1 s sampler cadence. Sampler parks with no timer when hidden (lib.rs wait_visible). No change needed.

## gpu_wait check
`gpu_wait` (~3.5 ms, 13% of capture) is the GPU finishing the scene render the capture needs; it is inherent work (profile-only split), and without the wait the same time would move into the readback. Only fewer or cheaper captured elements reduce it; no change.

## u166 GPU upscale of the backdrop backing (done)
Capture total 26.5 -> 5.9 ms (quiet run) / 15.0 ms (busy run) per capture (crop 7.9 ms and store 6.1 ms removed; load-dependent GPU waits dominate the busy figure) by drawing the reduced blurred capture at full size on the GPU with the surface clip program. 459 wm-wayland tests, clippy -D warnings pass; frame compared with the CPU-path frame.

## Capture re-rank after u166
Quiet run total 5.9 ms: blur 2.2 ms (CPU separable blur on the 295x161 reduced image via `frosted_backdrop_with`), gpu_wait 1.4, map 0.6, store 0.3. Nothing left above ~2 ms that is not inherent GPU work; blur on the GPU would trade 2 ms of CPU for extra passes and is not worth it at this size. Capture is no longer a ranked hotspot; it only runs while a translucent window or dock moves.

## u166 dock check
Nested run with the real shell dock (`flamewm-shell` autostart): 172 backdrop captures, dock bar at the bottom renders with its dark translucent backdrop, no artifacts or missing backdrop (frame read). The dock here is a full-width bar with no pill radius and nothing under it, so the rounded pill clip on a dock is covered only by the shared clip shader and its tests, not by this frame.

## Drag composition after u166
dock_backdrops 72% (bounded capture), elements 14% (titlebar 84% of it, ~11% of the drag), draw 11%. Next unit u167: move a titlebar without rebuilding it.

## u167 titlebar move without rebuild (done)
Titlebar element cost during a drag 71 ms -> ~1 ms (elements 14% -> 2-3% of frame work); no stale-reuse lag any more. 458 tests, clippy -D warnings; title strips match the old frame within 0.07% RMSE.

## CI after u167
CI green for b7548fe2, 76cbc484, 1e0376bc, d3f427ca. Arch package runs still in progress.

## Terminal wakeup scout: nothing ranked
PTY reader coalesces wakes to one per 16 ms frame; scroll glide thread runs per request only; pty.rs sleeps are test-only. No change.

## Shell timers scout: nothing ranked
Profiler wake gated on enabled(); weather/usage/clock intervals are 5 min to 6 h; launch-feedback polls 50/250 ms only while a launch is pending; weather_host 10 ms sleep is test-only. No change.

## Duplicate dependency scout: nothing ranked
Duplicate crate versions (x11rb 0.13/0.14, rustix 0.38/1, thiserror 1/2) come from vendored smithay/winit/calloop; removing them means patching third_party, size gain small. No change.

## Explorer filesystem scout: nothing ranked
All fs calls (scan, trash list, copy/move/delete in jobs.rs, search, mime, thumbs) run on dedicated worker threads (flamewm-explorer-scan/io/mime/thumbs, refresh, search); no blocking disk work on the UI thread. No change.

## Live idle read (nested harness, Intel GL, ~10 s, no input)
flamewm-wayland RSS ~123 MB, +55 voluntary ctx switches (~5.5/s); flamewm-shell RSS ~23 MB, +18 (~1.8/s). Idle wakeups already very low, no unit. Compositor RSS includes the winit host window and GL driver, so it is not a clean DRM-session figure and is not ranked.

## Compositor RSS breakdown (smaps, nested harness)
RSS 123 MB = libLLVM 45 + nvidia GL libs 31 + gallium 13 (shared, file-backed driver pages) + binary 7 + heap 11.7 + anon 4. Private anon ~25 MB. Renderer reported as Mesa Intel UHD (hardware). No RAM unit: the compositors own footprint is small; why libLLVM is resident on the hardware path is a driver question, not ranked.

## Lock screen poll scout: reverted, no measurable gain
flamewm-lock wayland.rs polls the compositor connection with a 16 ms timeout while locked, which by reading is ~62 wakeups/s. I made it 16 ms only while a PAM result is pending (1 s otherwise) and measured old vs new builds (--features wayland, nested harness, 6 s): old +26 timeslices / 1.6 ms CPU, new +34 / 3 ms CPU, i.e. both idle at ~4-6 wakeups/s. The expected 16 ms rate does not appear, reason not found, so no benefit was demonstrated and the change was reverted rather than shipped to a security-sensitive path. A real session may behave differently; recheck there.

Lock poll scout, explained (strace run from start): in the nested harness flamewm-lock ran its X11 path: 520/535 polls have an infinite timeout, ~6 polls/s (1 s tick + events), no 16 ms poll. So the Wayland loop in wayland.rs (16 ms connection poll while locked) was never exercised and stays unmeasured; the X11 lock path is event-driven. Measure the Wayland lock in a real Wayland session (strace -f from launch; attach is blocked by ptrace scope) before changing it.

## Wayland lock idle poll: 62/s -> 1/s (done)
Measured with strace run from launch, WAYLAND_DISPLAY=wayland-flame so the lock takes the Wayland path (the earlier X11-path runs were the wrong path): old build 817 polls with a 16 ms timeout in ~13 s (~62 wakeups/s while locked); new build 26 polls, ~1/s (1 s timeout). `wait_readable` now takes `authenticating`: 16 ms only while a PAM result is pending (the result arrives on a channel that does not wake the poll), 1 s otherwise; keys and compositor events still wake the poll on the Wayland fd. clippy -D warnings and 176 tests green with --features wayland (includes the unlock tests); old vs new lock frame pixel-identical (RMSE 0).
Separate finding, NOT from this change (old build identical): in the nested harness the Wayland lock frame has swapped red/blue (orange sky), a `--:--` clock, no blur, and the "Are you sure?" dialog plus the Log out menu drawn open. Needs a look in a real Wayland session before calling it a defect: it may be a harness software-render artifact.

## Wayland lock: red/blue swapped (fixed)
`shm_buffer` declared wl_shm `Argb8888` (bytes B,G,R,A) but the painter writes R,G,B,A, so red and blue were swapped on the Wayland lock (orange sky). Now converted by `rgba_to_wl_argb8888` (unit-tested), format kept as `Argb8888` because every compositor must offer it. Frame read in the nested harness: sky blue, lake teal, background blurred (was orange/olive before); 177 tests, clippy -D warnings green with --features wayland.
Still wrong in the same frame (open, unexamined): the "Log out / Cancel" menu and the "Are you sure?" dialog are drawn open, the clock reads `--:--`. Next unit: the software painter on the Wayland path seems to ignore hidden nodes and the clock text.

## Wayland lock: no clock or date (fixed)
All Wayland projections passed `None` for the time, so the lock could never show a clock or date (`--:--`). Now `native_time::local_now()` feeds the projection, and the loop repaints when the minute changes (`minute_changed`, checked on the existing ~1 s wake; unit-tested). Frame read in the nested harness: `01:03` and `Friday, 27 November 2026` shown. 178 tests + clippy -D warnings green with --features wayland. Still open: the "Log out / Cancel" menu and "Are you sure?" dialog are drawn open in the same frame.

## Wayland lock: confirm dialog and Other menu drawn open (fixed)
The Wayland closure in main.rs applied the document and the view but never `apply_power`, so the "Are you sure?" dialog and the Log out/Cancel list stayed at the document default (visible). It now applies the idle `PowerUi` view (rows disabled, dialogs hidden), matching the theme preview path. Frame read: blue sky, `01:05` + date, avatar, username, password field, four dimmed power rows, no open popups. Remaining difference vs the X11 lock frame: the Wayland software painter draws a dashed avatar ring and regular-weight text (X11 shows a solid ring and bold); not examined further.
Wayland lock summary this session: idle poll 62/s -> 1/s, red/blue swap, no clock/date, popups open: all fixed; the Wayland lock had never been looked at in a frame before.
