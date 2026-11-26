# Perf loop log (T0=2026-11-26T09:27:11-03:00)

## Build 1 (min 30): cargo check --workspace --all-targets green after 4 fixes (render-x11 gpu.rs borrow, explorer places.rs inference x2, projection.rs labels). 24 units landed (u01-u24); u25-u30 in flight.

## Build 2: cargo check --workspace --all-targets green after 2 fixes (archiver progress.rs scratch buffer lifetime, desktop rubber.rs test types). u01-u40 landed. Root disk at ~100% (other projects /tmp/opencode 58G); pool paused disk_low.

## Build 3: cargo check --workspace --all-targets green after 2 fixes (task-manager duplicate Sampler.text field, desktop inotify events.count() then reuse -> peekable). u01-u50 landed, u51-u57 in flight.

## Build 4: cargo check --workspace --all-targets green after 3 fixes (tray watcher match Some(), terminal run_id_into import, terminal test frame_rows) + scripts/perf-loop check fix. Python/ctx_execute banned (.agents/NO-ADHOC-SCRIPTS.md); perf tests scoped to 1 s composition (.agents/skills/scoped-perf-tests). u63-u67, u56, u61, u62 still in flight.

## Build 5: cargo check --workspace --all-targets green, 0 fixes. u56,u61-u67 landed; stale-pool reaped and restarted with abort_requeue.

## Build 6: cargo check --workspace --all-targets green after 1 fix (terminal session.rs PollFd::new takes &fd). u68-u78 landed or in flight.
