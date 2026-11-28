# p104 idle wakeup audit (service and shell processes)

STATUS: OPEN

Scope: notification server, tray, clipboard, battery/network/volume (pulse, audio helper), desktop, shell.
Read-only audit. No crate edits, no git, no cargo, no python. Rates are derived from the code constants and
their re-arm paths, not from a runtime sample: UNMEASURED. Measurement method for each fix is in its spec.
Overlaps with PERF_HOTLIST row 33 (usage, p91), row 10 (idle wakeups), row 37 (idle audits) are referenced, not repeated.

## Ranked rows (wakeups/s saved, zero visual change only)

| rank | file:line | interval | runs while idle | can be zero-visual event/deadline driven | saved wakeups/s |
| --- | --- | --- | --- | --- | --- |
| 1 | `crates/flamewm-integrations-linux/src/pulse.rs:26` (`SLOW_RETRY_INTERVAL` 5 s), `pulse.rs:653` (`mark_disconnected`, `None` branch) and `pulse.rs:657` (`restart_timer`) | 5 s forever once `next_reconnect_delay_ms` returns `None` (backoff budget in `crates/flamewm-integrations-core/src/backoff.rs:8-21`, `pulse.rs:159-167`) | Yes, whenever PulseAudio is not running: each tick is a socket connect attempt, nothing visible changes | Yes. Watch the pulse native socket directory (inotify or a calloop fd) and reconnect on creation; no visible change, latency preserved | ~0.20 (to 0 with a watch; to ~0.017 with a 60 s cap alone, but that adds up to 60 s reconnect latency) |
| 2 | `crates/flamewm-shell/src/main.rs:53` (`BUS_RETRY_INTERVAL` 30 s), `main.rs:1522` (`arm_bus_retry`), `main.rs:856` (log text) | 30 s forever while the session control bus is absent | Yes, degraded mode with no visible change until the bus appears | Yes. Subscribe to `NameOwnerChanged` for the control bus name; keep the 30 s timer only as fallback if the subscription fails | ~0.033 (to 0 with a subscription) |
| 3 | `crates/flamewm-shell/src/usage_host.rs:20` (`USAGE_REFRESH_INTERVAL` 300 s), `usage_host.rs:105` (`next_deadline`), `crates/flamewm-shell/src/app/timers.rs:259` | 300 s, plus credential read and network fetch | Yes when the usage popup is closed (`needed` false is the gate that p91 wants) | Already deadline-based; the fix is the popup gate. Owned by PERF_HOTLIST row 33 (p91, UNBUILT). Not duplicated here | ~0.003 plus fetch I/O |
| 4 | `crates/flamewm-desktop/src/widgets/mod.rs:147` (`SAMPLE_INTERVAL` 4 s), `widgets/sampling.rs:58-109` (`arm`, `sample`), `widgets/lifecycle.rs:51` (`poll`) | 4 s, only while a CPU, Memory, Ring, History, Network or Custom widget is visible and the desktop is not obscured | Only when a sampling widget is visible | No. Every sample shifts the CPU, memory and network graph by one point, so each wake is a visible change. Reading `/proc` is required to know whether the value changed, so skipping flat samples saves no reads | 0 under the zero-visual rule. Not a cut |

Verified event-driven or deadline-based, no idle timer (no row): `flamewm-integrations-linux/src/notifications.rs:27-29,398` (`next_deadline`), `crates/flamewm-tray/src/watcher.rs:652` (2 s one-shot D-Bus reply timeout), `crates/flamewm-clipboard/src/lib.rs:43,283-300` (500 ms bound only during a GET; loop otherwise blocks), `crates/flamewm-integrations-linux/src/device_watch.rs:16,75-89` (`poll` timeout is `None` when no burst is pending), `crates/flamewm-platform/src/services/settings_persist.rs:19-21,150` (`recv_timeout` to a deadline), `crates/flamewm-audio-helper/src/session.rs:15-19,58` (`recv_timeout(idle)` exits, `READY_POLL_*` only during start), `crates/flamewm-shell/src/osd.rs:25-28` (one-shot hold deadlines), `crates/flamewm-shell/src/weather_host.rs:22,222` and `weather_auto.rs:21` (30 min and 6 h, `None` when disabled), `crates/flamewm-reactor/src/lib.rs:66,96-107` (`bounded_timeout`, zero idle), `crates/flamewm-shell/src/main.rs:1210,1262` and `crates/flamewm-desktop/src/main.rs:296` (profiler timers, gated by `flamewm_profiler::enabled()`), `crates/flamewm-desktop/src/refresh.rs:297-323` (cursor retry, capped at 300 s, no re-arm without a reload).
Clock ticks (`widgets/sampling.rs` tick rate, `crates/flamewm-shell/src/app/timers.rs:345`) are deadline-based at 1/60 s, and 1 Hz only when seconds are shown (visible change).

## Fix specs (max 3)

### F1 (rank 1): pulse absent-server retry becomes a socket watch
- Edit `crates/flamewm-integrations-linux/src/pulse.rs` only. In `mark_disconnected` (`pulse.rs:653`), replace `restart_timer(timer, SLOW_RETRY_INTERVAL)` with an inotify watch on the pulse native socket directory (the runtime dir `pulse/`), registered through the existing calloop/reactor fd path. On a create or move-to event for the socket name, reconnect at once.
- Keep `SLOW_RETRY_INTERVAL` as a fallback timer of 60 s only when the watch cannot be set up. Port the logic into a pure function so a unit test can run without a server.
- Acceptance (numeric): with no pulse server for 60 s, a scoped strace of the helper or shell shows 0 `connect` calls on a 5 s cadence (was 12 per minute). Start a server and measure reconnect latency under 200 ms.
- Check: `cargo` is not run by this worker; the owner runs the gate.

### F2 (rank 2): control bus retry becomes a name-owner subscription
- Edit `crates/flamewm-shell/src/main.rs` (`arm_bus_retry`, line 1522) only. Subscribe to `NameOwnerChanged` for the control bus name; on acquisition call `attach_control_bus` once. Keep the 30 s `BUS_RETRY_INTERVAL` only as the fallback when the subscription fails.
- Acceptance (numeric): with the bus absent, 60 s of idle produce 0 retry fires (was 2). Bus start attaches within 500 ms.

### F3 (measurement, no cut): scoped wakeup composition of the desktop with one CPU widget visible
- Add no code. Run a per-thread voluntary context-switch sample (the `docs/PERF_BASELINE.md` method) for 60 s, 40 s after launch, with (a) no widgets and (b) one CPU widget. Record wakeups per second per thread.
- Purpose: confirm the 0.25/s sampler share (row 4) and the idle zero for case (a), so that F1 and F2 are ranked against a real number. Report composition, not absolute CPU.

## Notes
- Rows 1 and 2 are the only zero-visual cuts found. Row 3 is owned by PERF_HOTLIST row 33. Row 4 is a visible change and is not cut.
- Consumer of the pulse controller (which process holds it at idle) was not traced in this audit. F1 must check that before it is applied.
- No image was produced; no image review record is needed.
