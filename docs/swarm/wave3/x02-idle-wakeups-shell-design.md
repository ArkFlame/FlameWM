# x02 idle wakeups in flamewm-shell — design

Scope: `crates/flamewm-shell` shell loop and timer registrations, plus the shared wake slot in
`crates/flamewm-reactor`. Goal of the unit: every timer event is deadline-driven or coalesced, with
each timer's period and owner recorded. Constraint: visuals pixel-identical, no new dependencies,
`.vendor/**` is read-only evidence only. No build, test, Cargo, Git or Python command was run for
this unit (worker rules and the orchestrator's post-batch checks own those).

## 1. Current structure (file:line)

Wake plumbing — `crates/flamewm-reactor/src/lib.rs`:

- `:66-90` `WakeDeadline`: one `Cell<Option<Instant>>` plus a dirty bit; `set` is a no-op for an
  unchanged instant, so equal-period owners cannot retrigger epoll.
- `:158-191` `Reactor` owns the shared slot and exposes `wake_deadline()` / `owned_wake_deadline()`.
- `:430-445` `wall_clock_delay_ms` computes the delay to the next minute (or second) boundary from
  the current epoch time; this is the drift-free alignment primitive.
- `:454-490` `register_wall_clock` installs one self-rescheduling wall-clock timer; a forward jump
  realigns on the following tick.
- `:504-525` `Reactor::dispatch` reads the earliest wake only when an owner set one; idle turns pay
  no clock read before parking.

Shell tick — `crates/flamewm-shell/src/app/timers.rs`:

- `:14-202` `ShellLoop::tick`: drains event/fd results, reconciles settings, then does the clock,
  OSD, switcher, dock, weather, usage and badge turns. Steady state is signal-driven; `:178-180`
  records that it never polls.
- `:206` `attach_wake` stores the reactor wake slot in `ShellLoop` (`app.rs:222`).
- `:213-236` `rearm_clock`: arms `clock_next_at` only when the taskbar and clock item are shown;
  the delay comes from `clock_boundary_delay` (minute boundary, or one second in seconds mode).
- `:242-264` `publish_wake`: publishes the earliest of **11** owner deadlines into the shared slot.
- `:342-346` `clock_boundary_delay`: pure wrapper over the reactor's wall-clock alignment.
- `:349-380` two tests: cadence bound and earliest-deadline selection.

Typed owner state — `crates/flamewm-shell/src/app/`:

- `helpers.rs:185` `ZONE_SETTLE` (1100 ms), `helpers.rs:189-192` pure `earliest_wake`.
- `app.rs:131-135` `clock_next_at`, `clock_settings_key`, `clock_projected_sec` (one wall second
  is painted once even when two sources land together).
- `usage.rs:8` `USAGE_REPROJECT_INTERVAL` (60 s, open card only), `settings.rs:86-115` auto-location.
- `weather.rs:8-18` weather tick; `weather_host.rs:222` and `usage_host.rs:105` expose deadlines.
- `osd.rs:228,287` and `switcher.rs:241` hide deadlines; both return `None` while idle.

Timer registrations and polling cadences (period and owner) in `crates/flamewm-shell/src/`:

| file:line | period | owner | idle contribution |
| --- | --- | --- | --- |
| `main.rs:1097-1101` | next wall minute (≤60 s), self-rescheduling | reactor clock → `clock_due` | **1/60 Hz even when the clock is hidden** |
| `main.rs:1103-1108` | one-shot startup delay | free-heap trim | 0 |
| `main.rs:1309-1315` | one-shot 5 s | profiler window, `flamewm_profiler::enabled()` gate | 0 when profiling off |
| `main.rs:1362-1367` | repeating `profile_interval` | profiler, same gate | 0 when profiling off |
| `main.rs:1632-1642` | `bus_retry_delay(attempt)` one-shot, exponential | session control-bus attach | 0 once attached |
| `polkit_answer.rs:126` | `answer_timeout` one-shot | pending polkit helper | 0 |
| `quick_controls/host/run.rs:165-173` | repeating `profile_interval_secs` | popup-host profiler, gated | 0 when profiling off |
| `timers.rs:213-236` | next wall minute, or 1 s in seconds mode | panel clock, armed only while shown | 1/60 Hz or 1 Hz while shown, 0 hidden |
| `timers.rs:242-264` | earliest of 11 owner deadlines | one shared reactor wake slot | coalesces equal deadlines |
| `launch_feedback.rs:22-25,76-105` | 50 ms moving / 250 ms still | launch badge, only while a launch is visible | 0 |
| `weather_host.rs:222`, `weather_auto.rs:21` | settings interval / 6 h | weather + auto-location fetch | 0 while not due |
| `usage_host.rs:105`, `usage.rs:8` | settings interval / 60 s open-card reproject | usage fetch + card | 0 while card closed |
| `osd.rs:228,287`, `switcher.rs:241` | hide deadline | OSD / Alt-Tab card | 0 |
| `main.rs:1239,1262,1305,1369` + `:1420-1433` | n/a (handles, not timers) | 5 wake handles aliasing one slot | coalesced |

## 2. Counted problems (no fresh measurement was run)

These are static counts from `rg` over the named paths, not perf measurements:

- **1** unconditional periodic wake source at idle: `register_wall_clock` at `main.rs:1097`, which
  fires every minute even when the taskbar is disabled or the clock item is off. At that turn the
  `timers.rs:134` guard sees `clock_next_at == None` and paints nothing — a pure idle wake.
- **2** owners for one clock boundary: the reactor minute timer (`main.rs:1097`) and the shell
  deadline (`timers.rs:229`). `app.rs:133-135` `clock_projected_sec` deduplicates the *paint* but
  not the wake, so both can dispatch around the same wall boundary.
- **6** `register_timer` calls in the crate (`main.rs` ×4, `polkit_answer.rs` ×1,
  `quick_controls/host/run.rs` ×1); 5 of 6 are one-shot or profiler-gated and contribute 0 at idle.
- **5** wake handles alias one slot (`main.rs:1239,1262,1305,1369` plus the badge path at
  `:1420-1433`); this is already coalesced by `WakeDeadline`.
- **11** deadlines are collected by `publish_wake`, but the step-1 test at `timers.rs:358-378`
  exercises `earliest_wake` with only **6** placeholder entries, so the production list and the
  test can drift apart without failing.
- **0** measured idle-wake baseline: no scoped timer trace was collected in this unit. The reactor
  has a `FLAMEWM_TIMER_TRACE` gate at `lib.rs:527-537` ready for step 5.

## 3. Target design

One owner per mechanism, behind the existing typed facades:

1. `WakeDeadline` stays the single reactor wake slot. Every shell owner publishes a deadline; equal
   or later deadlines coalesce in `earliest_wake`. No owner creates its own periodic timer for a
   boundary another owner already publishes.
2. The panel clock has exactly one wake owner. In seconds mode the shell deadline (1 Hz) is that
   owner; in minute mode the wall-boundary alignment comes from `clock_boundary_delay`, and the
   redundant unconditional reactor timer is retired. Hidden taskbar/clock arms no deadline, so idle
   wakeups drop to zero.
3. Fetch/scheduler work (weather, auto-location, usage) stays deadline-driven: `None` while no
   request is in flight, one absolute deadline while one is, never a free-running poll.
4. Transient UI (OSD, switcher, badge, dock) arms a single hide/poll deadline only while visible.
5. Visual alignment is preserved by deriving every deadline from the wall clock
   (`wall_clock_delay_ms`), so `%H:%M` still changes exactly on the minute and seconds still change
   on the second. No new dependencies; no pixel path is edited.

## 4. Steps — each compiles alone

**Step 1 (implemented with tests, UNBUILT, already present in this shared checkout).** Pure
`earliest_wake` in `app/helpers.rs:189-192`; wired into `publish_wake` at `app/timers.rs:242-264`
so all 11 owner deadlines pass through one minimum; two tests at `app/timers.rs:351-380`
(cadence bound, earliest selection). The helper is additive and behavior-neutral: the call site is
one expression over the same deadlines. Rollback: remove the helper and tests and restore the
inline minimum in `publish_wake`.

**Step 2.** Introduce a typed `WakeOwners` value holding the 11 deadlines, use it in `publish_wake`,
and extend the step-1 test to pass the real owner set. Compiles alone because the value is additive
and the call site stays one expression. Rollback: restore the inline array and the 6-entry test.

**Step 3.** Remove the unconditional `register_wall_clock` block at `main.rs:1097-1101` and retire
`clock_due` from `ShellLoop::tick` (`timers.rs:14,17,130`, call site `main.rs:1410`), leaving
`clock_next_at` as the sole clock owner. `rearm_clock` already realigns from the wall clock after a
resume jump. Compiles alone once the signature and its single call site change together; visuals
are identical because both sources targeted the same wall boundary, and the `clock_projected_sec`
guard is unchanged. Rollback: restore the registration, the flag and the parameter.

**Step 4.** Extract a pure `clock_deadline(shown, show_seconds) -> Option<Duration>` seam from
`rearm_clock` (`timers.rs:213-236`) and test hidden/taskbar-off, minute and seconds transitions.
Compiles alone with a delegation-only edit. Rollback: restore the inline body and delete the tests.

**Step 5.** Add a scoped idle-wake harness using the existing `FLAMEWM_TIMER_TRACE` gate
(`reactor/src/lib.rs:527-537`) to count dispatches per minute before and after step 3; harness only,
no production change. Compiles alone. Rollback: delete the harness.

No persistent state, schema or config changes in any step; rollback is a file-hunk revert.

## 5. Reference-first port record

Question: how do upstream shells drive a clock label without a free-running second timer, and how
do they keep transient work from ticking while idle? Active owner: `app/timers.rs` (shell deadline
and shared wake) and `flamewm-reactor` (single wake slot).

- `.vendor/cosmic-epoch/cosmic-applets/cosmic-applet-time/src/window.rs:387-426`. The clock applet
  uses a 1 s `tokio::time::interval` when `show_seconds` is on, re-arms to a 60 s `interval_at`
  aligned to `Timestamp::now() % period` when it is off (`:413-424`), sets
  `MissedTickBehavior::Skip` (`:390`), and re-arms on a settings change (`:405-425`).
  **Choice:** port only the semantics — wall-clock-aligned 60 s period, skip missed ticks, re-arm on
  settings change. FlameWM already has these in `clock_boundary_delay` (`timers.rs:342`) and
  `rearm_clock` (`:213`); no `tokio` interval, constant or code was copied, and no new dependency.
- `.vendor/cosmic-epoch/cosmic-panel/cosmic-panel-bin/src/xdg_shell_wrapper/mod.rs:99-125`. Pacing
  is driven by frame callbacks and calloop sources, so `event_loop.dispatch` can use generous
  timeouts because a source wakes the loop instead of a poll. **Choice:** adopt the same
  single-source discipline, already realized by `WakeDeadline` (`reactor/src/lib.rs:66-90`) and
  `publish_wake` (`timers.rs:242`); nothing ported verbatim.
- `.vendor/vela-main/shell/src/requested.h:26-50`. A `QTimer` is `setSingleShot(true)` and started
  only while a request is pending (`:33-48`), so an idle shell has no request timer at all.
  **Choice:** port the "arm only while work is in flight" rule, already reflected in the OSD,
  switcher, weather and usage deadlines (`timers.rs:249-263`); no Qt type or code was copied.

All reference paths stayed read-only evidence; no path was added to
`docs/ARCHITECTURE_EXCEPTIONS.toml`.

## 6. Verification status

Step 1's pure helper, its production wiring and its two tests are present in the shared checkout
and were verified by reading current source; they were **not duplicated**. All counts above are
static `rg` counts, not measurements. No `cargo`, build, test, Git or Python command was run (worker
rules forbid them); the orchestrator runs checks after the batch lands. Step 1 is **UNBUILT** and
its tests are unrun; every edit recorded in `x02-idle-wakeups-shell.md` is marked UNBUILT.
