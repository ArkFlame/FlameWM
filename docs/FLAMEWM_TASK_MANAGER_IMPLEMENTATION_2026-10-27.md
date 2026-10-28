# FlameWM Task Manager — implementation and verification

## Product

Single native X11 surface; Windows 11-inspired glass presentation and accent; htop-style screen arrangement. No navigation sidebar, tabs, browser, Electron, GTK or background daemon. Fixed header: up to 16 logical CPU slots; on hosts with more than 16 logical CPUs, slot 15 shows their combined average (CPU 15+), RAM and swap progress, process/thread/running counts, load average, uptime. Scrollable process list: PID, user, priority, nice, virtual and resident memory, CPU%, memory%, state, full command. CPU per-process is normalized to one logical CPU = 100%, matching htop semantics. One-second procfs sampling only while the window runs.

## Controls

- Mouse wheel over process list: scroll by three processes; click row to select; right-click opens priority/termination menu.
- F3 or `/` starts text filtering; Backspace edits; Enter/Esc ends editing.
- F6 changes sort column; CPU/MEM/PID header buttons select sorting and reverse order.
- Arrows and PageUp/PageDown navigate selected processes.
- F7/F8 adjust nice by one; `renice` only operates with the caller's standard Linux permissions.
- F9 or the context menu requests SIGTERM, **after** confirmation. Check PID and `/proc/PID/stat` start time immediately before invoking process operation; residual PID reuse race still exists between the check and the external `kill`/`renice` invocation.

## Production ownership

- `crates/flamewm-task-manager/src/model.rs`: procfs process sampling, CPU delta accounting, permission-limited process operations; no X11 or rendering dependencies.
- `crates/flamewm-task-manager/src/lib.rs`: UI projection and window loop; no filesystem/process I/O on UI callbacks. A bounded queue passes process-control requests to the existing on-demand sampler worker, which validates PID/start-time, executes `renice`/`kill`, and wakes the UI through a bounded Unix stream with operation results.
- `ui/task-manager`: compiled native HTML/CSS template; no web browser at runtime.
- `flamewm-ui-core::surface` and `flamewm-ui-x11::surface_style`: existing shared border/fill policy reused; `accent` setting matches FlameWM named presets.
- `flamewm-apps`: same installed multi-call executable for shared code pages; process monitoring worker is spawned only when Task Manager is opened.
- `scripts/package`, `packaging/install-manifest.txt`, `packaging/flamewm-task-manager.desktop`, `scripts/xephyr`: install/development discovery.

## Build and live validation commands

```sh
cargo check --locked -p flamewm-task-manager -p flamewm-apps -p flamewm-explorer -p flamewm-wm-x11
cargo test --locked -p flamewm-task-manager -p flamewm-ui-core -p flamewm-wm-x11
cargo build --locked --release -p flamewm-apps -p flamewm-task-manager
./xephyr
./.target/release/flamewm-task-manager
```

Verify CPU counters against `htop`, `ps -eo pid,ni,rss,%cpu,cmd` and `/proc/stat`; monitor per-process RSS and the worker wake rate while the app is open. Verify normal user cannot renice other users' tasks, use right-click to change a task's nice, and confirm cancellation sends no signal. Test filtering, wheel scrolling and resizing on 720p/1080p and nested X11. Check root surface contour with different colors and 0/25/100% border opacity and live settings updates.

## Exclusions and risks

No kernel pidfd-based atomic PID target: selection uses (PID, starttime) followed by standard Linux tools; a very narrow target-reuse race remains. No process trees, affinity editing, cgroup manipulation, container namespaces or GPU process metrics. Per-core header displays cores 0–14 individually and a mean CPU 15+ slot when the host has more than 16 logical CPUs. Priority value reflects conventional `nice`/static Linux scheduling and is **not** a real-time scheduler. Procfs process visibility is limited by host `hidepid` settings and permissions. Actual interactive visual parity and Cargo compilation are not verified by the source packaging environment.
