# Unit p66-desktop-idle-steady
STATUS: PARTIAL

Crate flamewm-desktop: steady-state idle wakeups (measured ~9/s per 4 s sample, hotlist B1): find every timer/poll (rg 'Duration|timerfd|sleep|poll|tick'), coalesce or make event driven. Output identical. Max 3 edits.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-desktop/src/widgets/sampling.rs:58-84` | Only periodic sources are a wall-clock tick deadline (1/s with seconds/analog/Custom, 1/60 s with Clock/Calendar), already boundary-aligned by `wall_clock_delay_ms`, and a 4 s `sample_due` armed only for CPU/memory/ring/history/network/custom widgets. `WakeDeadline::set(min)` coalesces both into one earliest wake; the sub-millisecond monotonic anchor fix from p02 is already present. | NONE - already event-driven and coalesced; re-applying would duplicate p02 | UNBUILT (worker forbids build/test/runtime; static evidence only) |
| `crates/flamewm-desktop/src/widgets/lifecycle.rs:29-49,51-81` | On full obscure the widget wake slot is dropped and sampling/sync suspended; on uncover it re-arms once. `poll` returns before `Instant::now()` when no deadline or media change is pending, so a widget-free desktop has zero periodic wakes. | NONE - already landed | UNBUILT |
| `crates/flamewm-desktop/src/sticky.rs:52-55,114-134,286-288` | Persist debounce (500 ms) is a one-shot armed only by an edit, with exponential retry capped at 30 s and its own private `WakeDeadline`; no idle wake. | NONE | UNBUILT |
| `crates/flamewm-desktop/src/downloads.rs:256-271`; `crates/flamewm-downloads-core/src/lib.rs:18` | 100 ms settle poll is armed only while tracked files exist, lives in a private wake slot, and is disarmed when empty; the organizer is gated by `organizeDownloads`, default false, at `main.rs:504`. | NONE - pausing it would delay real housekeeping, not defer an invisible repaint | UNBUILT |
| `crates/flamewm-desktop/src/main.rs:293-313` | Profiler report timer is opt-in (60 s, floor 10 s); startup heap trim is a one-shot at 10 s. Neither contributes to steady-state idle. | NONE | UNBUILT |
| `crates/flamewm-desktop/src/main.rs:355-358,706-748,774-823`, `wallpaper_loader.rs:115-171`, `output_layout.rs:24-27,281` | Inotify, workspace-mutation, media, trash, icon and wallpaper wakes are fd/condvar-driven with no deadline; the `Duration::from_secs(2)` in `output_layout.rs` is a per-call RPC timeout, not a timer. Every `thread::sleep` in the crate is `cfg(test)`. | NONE | UNBUILT |
| `crates/flamewm-reactor/src/lib.rs:503-524` (read only) | `dispatch` bounds its wait by the earliest of three owner deadlines and skips `Instant::now()` when none is armed, so the loop parks in epoll with no periodic timeout from the desktop crate. | NONE - reused as-is | None |

Production edits: 0 (the maximum of 3 was not reached). No tests were edited; none were requested. Reference-first check: `.vendor/smithay-master/anvil/src/udev.rs:1281,1376,1544` uses the same `Timer::from_duration`/`TimeoutAction::ToDuration` one-shot-or-rearm pattern and `insert_idle` for deferred work; `.vendor/xfdesktop-master` has no wall-clock/interval timer precedent; no upstream clone was needed.

PARTIAL - the audit of every `Duration`/timer/poll/tick in flamewm-desktop is complete: all periodic work is boundary-aligned, one-shot, event/fd-driven, gated by a default-off setting, or already suspended while the surface is obscured, and the reactor coalesces the remaining deadlines into a single earliest wake. No source change was made because every candidate cut is already landed in the shared checkout (p02, u41, u63) and any further pause would either duplicate an edit or delay real housekeeping rather than defer an invisible repaint; visuals are untouched. The ~9/s hotlist figure is a pre-correction sample (BACKLOG.md:61 records the baseline as startup churn and asks for a steady-state re-sample), so no wakeup reduction is claimed here. Nothing was built, tested or measured under the worker contract, so all findings are static evidence and UNBUILT. Skills loaded: flamewm-source-first, flamewm-rust-systems, flamewm-performance-proof, no-adhoc-scripts.
