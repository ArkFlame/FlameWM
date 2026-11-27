# u02 — Shell idle wakeup / timer audit (retry)

**Scope:** `crates/flamewm-shell/src` only, excluding `projection/start.rs`, `projection/usage.rs`,
`async_projection/start_view.rs` and `app/start_view.rs`, plus this unit file. No `git`, no
`cargo`, no builds or tests, no Python, no sleep loops; shared checkout, orchestrator commits.
Visuals are unchanged: the edits move when a wake writer is cloned and stop a wake that painted
nothing. **All edits below are UNBUILT** — no compiler or runtime evidence was collected here.

## Findings

| Area | file:line | Cost | Change | Risk |
| --- | --- | --- | --- | --- |
| Degraded-start bus retry timer | `main.rs:931` (now `main.rs:933-935`, `main.rs:1410-1432`) | Repeating 30 s timer kept waking forever after the bus attached; its callback only checked `attached` and returned, so 2 idle wakeups/min with no paint. | **EDIT (UNBUILT):** replaced the `repeat = true` registration with `arm_bus_retry`, a self-rearming one-shot chain that stops after the first successful attach. | Low: calloop supports inserting a source from a dispatch callback and the fired one-shot drops itself; attach behavior is identical because the old timer never re-attached after success either. |
| Weather wake-writer clone | `app/weather.rs:9-13` (now `app/weather.rs:11`) | `ResultWake::waker()` cloned the `Arc` on every shell tick even when weather was disabled or no fetch was due. | **EDIT (UNBUILT):** borrow `&self.result_wake` and call `wake.waker()` inside the `FnOnce`, so the clone happens only when a fetch actually starts. | Low: disjoint field borrows (`result_wake` vs `weather`); the same waker reaches `spawn_weather_fetch`. |
| Usage wake-writer clone | `app/usage.rs:24` (now `app/usage.rs:26`) | Same unconditional `Arc` clone every tick; the usage card is usually closed and most ticks start no worker. | **EDIT (UNBUILT):** same deferral into the `FnOnce` passed to `UsageHost::tick`. | Low: disjoint field borrows; fetch payload and waker unchanged. |
| Automatic-location wake-writer clone | `app/settings.rs:104` (now `app/settings.rs:106`) | `Arc` clone on every tick while auto-location was enabled but not due (6 h cadence). | **EDIT (UNBUILT):** same deferral into the `FnOnce` passed to `AutoLookup::tick`. | Low: disjoint field borrows with `shell` and `auto_lookup`; the lookup worker gets the same waker. |
| Usage refresh deadline with card closed | `app/timers.rs:245`, `usage_host.rs:71-75` | 5 min deadline published unconditionally, so the shell wakes and runs a worker even if the card is never opened. | None: gating the deadline on `is_usage_open()` would leave the card showing stale capture counts on first open. | Visual: rejected as unsafe; recorded for a future change with an open-edge fetch. |
| Usage card reproject deadline | `app/timers.rs:248`, `app/usage.rs:8` | 60 s wake while the card is open to refresh reset captions. | None: removing it makes visible caption text go stale. | Visual: rejected as unsafe. |
| Launch-feedback pointer poll | `launch_feedback.rs:22-24`, `launch_feedback.rs:100-102`, `launch_feedback.rs:206-268` | 50 ms / 250 ms pointer query while a launch badge is in flight. | None: already active-only; `next_tick` returns `None` when no launch remains, so idle parks with no badge timer. | Visual: removing the poll would freeze badge tracking. |
| Wall-clock / clock deadline | `main.rs:912`, `app/timers.rs:215-227` | Minute-boundary wall-clock timer plus a shell deadline for seconds mode. | None: drives the visible time text. | Visual: necessary wake, no cut. |
| OSD / switcher / notification / dock deadlines | `app/timers.rs:230-246`, `osd.rs:228`, `notification_center.rs:22` | Deadlines exist only while a card, toast or bounce is live; `publish_wake` publishes the earliest and idles otherwise. | None: already deadline-driven. | Low: no idle wake to cut. |
| Profiler timers | `main.rs:1160`, `quick_controls/host/run.rs:167` | Repeating 1 s window timer in shell and quick-control host. | None: both are gated on `flamewm_profiler::enabled()`, so an idle shell with profiling off owns no timer. | Low: gating already avoids the wake. |
| Dock bounce id formatting | `app/dock.rs:1005` | `format!("dock-slot-{}-icon", …)` allocates on each bounce frame. | None: this runs during an active launch hop, not at idle, and avoiding the owned `String` needs a runtime API change outside this unit. | Low: out of idle scope; left untouched. |
| Notification icon 15 ms poll | `main.rs:610` | 15 ms wake while a toast icon request is outstanding. | None: bounded by `icon_wait_expired` and dropped when no toast wants the icon; event-driven, not idle. | Low: no idle wake to cut. |

## Verification

- No `cargo`, `git`, tests, builds, Python or sleep loops were run in this unit, per the brief;
  all four code edits are **UNBUILT** and need the orchestrator's build and visual check.
- Static check only: each deferred waker is passed to the same spawn function, and the bus retry
  chain preserves the original "attach once, never re-attach" behavior while dropping the wake
  after success.
