# Unit p02-desktop-clock-align
STATUS: PARTIAL

crates/flamewm-desktop widgets: align the clock widget wake to the next displayed minute (or second if seconds shown); zero wakeups between. Do not touch widgets/sync.rs or render.rs (re-read if you must).

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-desktop/src/widgets/sampling.rs:71-80` | Boundary alignment via `wall_clock_delay_ms` was already present, but the monotonic anchor (`now`, captured by `poll`, `attach_wake` and `refresh_after_change`) was read before the wall clock. The deadline could land microseconds before the displayed boundary and paint the previous value for a whole minute or second. | Read `Instant::now()` after the epoch read and use it as the tick anchor; `now` still drives `sample_due`. One wake at each displayed boundary, zero between, no extra timer and no formatting change. | UNBUILT. Worker contract forbids cargo, build, test and runtime checks. The change shifts the tick instant by a sub-millisecond amount; visuals stay pixel-identical apart from removing a stale frame. |
| `crates/flamewm-reactor/src/lib.rs:432-444` (read only) | `wall_clock_delay_ms` already returns the next second or minute boundary, with a full interval on exact boundaries; `WakeDeadline::set` keeps the single earliest deadline. | No edit: reused as-is. `sync.rs` and `render.rs` were not touched. | None. |
| `docs/swarm/units/u41-desktop-idle.md:44` (read only) | The earlier audit recorded this same early-deadline residual and made no edit. | No edit: this unit closes that residual inside the desktop widget owner. | None. |

No tests were added. The wall clock is not injectable (`crates/flamewm-api/src/clock.rs:15`), so a deterministic ordering test needs a new fake-clock seam outside this unit; the brief did not ask for tests.

PARTIAL — the clock wake is aligned to the displayed minute, or second when seconds or an analog hand are shown, with no wake armed between boundaries. The single production edit replaces a microseconds-early monotonic anchor with one read after the wall clock, removing a stale-frame risk while keeping wake count and rendered pixels unchanged. All findings are static source evidence; nothing was built, tested or measured, so the edit remains UNBUILT and verification is left to the orchestrator.
