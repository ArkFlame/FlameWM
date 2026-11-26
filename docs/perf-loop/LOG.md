# Perf loop log (T0=2026-11-26T09:27:11-03:00)

## Build 1 (min 30): cargo check --workspace --all-targets green after 4 fixes (render-x11 gpu.rs borrow, explorer places.rs inference x2, projection.rs labels). 24 units landed (u01-u24); u25-u30 in flight.

## Build 2: cargo check --workspace --all-targets green after 2 fixes (archiver progress.rs scratch buffer lifetime, desktop rubber.rs test types). u01-u40 landed. Root disk at ~100% (other projects /tmp/opencode 58G); pool paused disk_low.

## Build 3: cargo check --workspace --all-targets green after 2 fixes (task-manager duplicate Sampler.text field, desktop inotify events.count() then reuse -> peekable). u01-u50 landed, u51-u57 in flight.
