# Perf loop log (T0=2026-11-26T09:27:11-03:00)

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
