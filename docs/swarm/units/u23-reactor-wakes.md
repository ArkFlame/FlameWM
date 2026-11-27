# u23 — Reactor timer realignment and thread wake mechanics (hotlist 17-20)

**Scope:** `crates/flamewm-reactor/src` and this unit file only. No `git`, no `cargo`, no Python,
no builds or tests; shared checkout, orchestrator commits. Visuals are unchanged. **All edits below
are UNBUILT** — no compiler or runtime evidence was collected here.

## Hotlist readback (rows 17-20)

| row | Readback in owned code | Disposition |
| --- | --- | --- |
| 17 | Thread/channel wake mechanics: the pacer, per-input spawns and two-lane pool fixes live outside the reactor. In this crate `child_pipe.rs:22` is a one-shot child-process spawn and `timeline.rs` is pure transition math. | Nothing owned to cut; no thread or channel source inside the reactor. |
| 18 | Desktop watcher name cap (`NAMES_CAP`) lives in `flamewm-desktop`, not in this crate. | Ownership boundary; untouched. |
| 19 | Explorer burst debounce (trailing quiet window) lives in `flamewm-explorer`, not in this crate. | Ownership boundary; untouched. |
| 20 | Reactor timer wakeups: `lib.rs:279-321` repeats already re-arm through `next_repeat_deadline` (`lib.rs:140-148`), an anchor on the first deadline that skips missed periods. `register_wall_clock` (`lib.rs:452-493`) recomputes its delay from wall time every tick. Matches `docs/perf-loop/LOG-u11-reactor-timers.md:11-18,43`. | Realignment already landed; one residual redundant-work path cut below. |

## Applied cuts

| Area | file:line | Cost | Change | Risk |
| --- | --- | --- | --- | --- |
| Wake-deadline republish rescan | `lib.rs:83-88` (`WakeDeadline::set`), comments `lib.rs:58-59,70`, regression test `lib.rs:636-649` | Owners that publish the same absolute deadline on each tick marked the shared dirty flag anyway, so `Reactor::earliest_wake` (`lib.rs:174-185`) rescanned every deadline slot on every dispatch even though the earliest value could not have changed. | **EDIT (UNBUILT):** `set` writes the slot and raises `dirty` only when `Option<Instant>` actually differs; an unchanged republish is now a no-op. Updated the two comments that claimed a write always dirtied the cache, and added a unit test asserting the cache stays clean on republish and is dirtied on a real change. | Low: `Instant` is `Copy` and `Option<Instant>` has exact `PartialEq`, so an unchanged value cannot change the minimum across slots. Any real change still raises the shared flag, including change-then-revert before a dispatch. Standalone `WakeDeadline::default()` slots keep their own flag and the same semantics. |

## Candidates examined and not cut

| Candidate | file:line | Why not cut |
| --- | --- | --- |
| Timers re-armed with a fresh deadline each tick | `lib.rs:300-318`, `lib.rs:475-490` | `Reactor::register_timer` already re-arms to the next grid boundary via `ToInstant`. `register_wall_clock` deliberately uses `ToDuration` recomputed from `clock.now_epoch_ms()`; switching to `ToInstant` off the fired deadline would misalign the wall boundary after a suspend/resume jump and break the documented `LOG-u11-reactor-timers.md:33-34` property. Unsafe. |
| Multiple timerfds that could share one | calloop `Timer` source, `lib.rs:303`, `lib.rs:371`, `lib.rs:477` | calloop 0.14.4 creates one timerfd per `Timer`. Merging requires a shared wheel/source above the reactor with per-owner callbacks, an API change across callers outside the owned files. Not provably safe in this unit. |
| Wakeups with no work | `timeline.rs:229-233`, `lib.rs:502-522` | `Timeline::next_deadline` returns `None` when no transition is active, and `dispatch` only bounds its wait when an owner published a deadline. No idle polling path remains; the only redundant-work path was the republish rescan cut above. |

## Verification

- No `cargo`, `git`, tests, builds, Python or sleep loops were run in this unit, per the brief.
  The single code edit and its regression test are **UNBUILT** and need the orchestrator's build
  and test run.
- Static check only: `WakeDeadline::set` is the sole writer of `at` and `dirty`; every real change
  still raises the shared flag that `Reactor::earliest_wake` clears after its recompute.

PARTIAL — rows 17-19 have no owned code and row 20's timer realignment had already landed. Of the
three requested wake mechanics, one provably-safe cut was applied (unchanged deadline republishes
no longer dirty the shared slot cache), while per-timerfd merging and wall-clock `ToInstant`
re-arming were rejected as unsafe or outside the owned files. The edit is UNBUILT and awaits the
orchestrator's verification.
