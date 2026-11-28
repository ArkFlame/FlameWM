# z10 clock timer coalesce — design

Scope: panel clock wake-up cadence in `crates/flamewm-shell` and the next-deadline
math it shares with `flamewm-shell-core` / `flamewm-reactor`. Goal: the clock
wakes once per minute boundary only, and the deadline math is tested across DST
and minute edges. No new deps; visuals stay pixel-identical.

## Current structure (file:line, evidence before step 1)

| Path | Role |
| --- | --- |
| `crates/flamewm-api/src/clock.rs:5-19` | Product timestamps only: `unix_seconds`, `unix_nanos`. No deadline math. |
| `crates/flamewm-reactor/src/lib.rs:408-444` | `WallClockSource` / `SystemWallClock` (native event owner) and `wall_clock_delay_ms` at `:436`. Returns full interval when `now % interval == 0`. |
| `crates/flamewm-reactor/src/lib.rs:446-495` | `register_wall_clock`: self-rescheduling native timer; recomputes from current wall time on each fire (no drift, one catch-up after a jump). |
| `crates/flamewm-reactor/src/lib.rs:709-755` | Tests: boundary alignment and suspend-jump recompute (no sleep). |
| `crates/flamewm-shell-core/src/clock.rs:14-33` | Typed clock model: `ClockTimerPlan`, `clock_timer_plan`, `clock_shows_seconds`. |
| `crates/flamewm-shell-core/src/clock.rs:34-48` | **Second copy** of `wall_clock_delay_ms`, documented as mirroring the reactor primitive "so shell-core stays dependency-free". Correct boundary rule. |
| `crates/flamewm-shell-core/src/clock.rs:49-73` | `ClockDateTracker`: one calendar rebuild per local-date change (midnight/DST/suspend). |
| `crates/flamewm-shell-core/src/clock.rs:74-125` | Fake-clock tests; suspend jump only, no DST-offset case. |
| `crates/flamewm-shell-core/src/lib.rs:463-481` | **Third copy** of the plan, with inline arithmetic at `:475-476`. This copy omits the zero-remainder guard. |
| `crates/flamewm-shell-core/src/lib.rs:1182-1196` | Plan tests: one non-boundary minute value, one seconds interval. No boundary or DST coverage. |
| `crates/flamewm-shell/src/app/timers.rs:113-176` | `tick`: minute timer covers minute boundaries + suspend realignment; shell deadline covers seconds mode; hidden clock consumes the tick without a repaint. |
| `crates/flamewm-shell/src/app/timers.rs:210-233` | `rearm_clock`: arms `Instant::now() + clock_boundary_delay(...)` after the wall read. |
| `crates/flamewm-shell/src/app/timers.rs:235-270` | `publish_wake`: coalesces every shell owner into one reactor deadline via `earliest_wake`. |
| `crates/flamewm-shell/src/app/timers.rs:345-352` | `clock_boundary_delay`: product reaches into `flamewm_reactor::wall_clock_delay_ms` directly instead of the typed shell-core facade. |
| `crates/flamewm-shell/src/main.rs:1094-1102` | One `register_wall_clock(SystemWallClock, false, ...)` timer sets `clock_due`: minute-only, no free-running second timer. |
| `crates/flamewm-desktop/src/widgets/sampling.rs:79` | Another consumer of the reactor primitive (different owner, left alone). |

## Counted problems

1. **3 definitions** of "milliseconds to the next boundary": reactor
   `lib.rs:436`, shell-core `clock.rs:40`, and inline in shell-core
   `lib.rs:475-476`. Plus 1 raw call site from product (`timers.rs:348-352`)
   and 1 other consumer (`sampling.rs:79`).
2. **1 divergent definition**: shell-core `lib.rs:475-476` computes
   `interval - remainder` with no zero-remainder guard. At an exact minute or
   second edge it returns **0 ms**, so a tick re-arms an already-due timer and
   spins wakeups until the boundary passes. The two canonical copies return the
   full interval instead.
3. **2 definitions** of seconds-token detection: `clock_shows_seconds` at
   `clock.rs:29` and the inline `["%S","%T","%X","%r"]` scan at `lib.rs:471-473`.
4. **0 tests** for the plan API at exact boundaries and **0 tests** for a DST
   offset jump; existing tests cover a non-boundary value and a suspend jump
   only.
5. **1 product boundary bypass**: `timers.rs` uses the reactor primitive, so
   the typed clock facade is not the authority for shell wake math.

## Target design

- `flamewm-shell-core::clock` is the single typed owner of clock deadline math:
  `ClockFormat`, `ClockTimerPlan`, `clock_timer_plan`, `clock_shows_seconds`,
  `wall_clock_delay_ms`, `ClockDateTracker`. `lib.rs` re-exports it and keeps no
  second definition.
- The rule stays pure and recomputed from current epoch time every fire:
  `delay = interval - (epoch_ms % interval)`, and when the remainder is `0`,
  `delay = interval` (never `0`). No sleeps, no drift, no accumulated state.
- `flamewm-reactor` keeps its own `wall_clock_delay_ms` because it owns native
  event acquisition and cannot depend on a higher-level product crate; it is
  the native boundary primitive and carries the same documented rule. This is
  one semantic rule with a native mirror, not two independent owners.
- `flamewm-shell::app::timers::clock_boundary_delay` consumes the typed facade
  (`flamewm_shell_core::clock`) rather than the raw reactor primitive.
- DST: epoch millis are timezone-independent. All current DST offset changes
  are whole minutes (±1 h, +30 min), so the next epoch-minute boundary is the
  next local-minute boundary. The display still reads `Local::now()` on the
  due tick, so `%H:%M` text, date text and layout are unchanged — pixel-identical.
  Known limit recorded, not fixed here: a historical offset with sub-minute
  seconds would need local-time boundary math; out of scope and not present in
  the supported settings.

### Steps (each compiles alone)

1. **Implemented.** In `flamewm-shell-core/src/lib.rs`, make the root
   `clock_timer_plan` delegate its seconds detection and delay math to the
   typed `clock` facade (`clock::clock_shows_seconds`,
   `clock::wall_clock_delay_ms`), removing the divergent arithmetic and its
   zero-on-boundary bug. Add tests at exact minute/second boundaries, across
   DST offset jumps (±1 h, +30 min), and asserting equality with the facade.
   No API or dependency change.
2. Move `ClockTimerPlan` and `clock_timer_plan` out of `lib.rs` into `clock.rs`
   (which already has correct copies) and re-export with
   `pub use clock::{ClockTimerPlan, clock_timer_plan, ...}`; update imports in
   `lib.rs` tests. Compiles alone; no caller changes needed because the root
   path is preserved by the re-export.
3. Route `flamewm-shell/src/app/timers.rs::clock_boundary_delay` through
   `flamewm_shell_core::clock::wall_clock_delay_ms` and add a shell-level
   coalescing test. Compiles alone (shell already depends on shell-core).

### Rollback

Each step is one self-contained patch with no cross-step dependency; revert
that patch only. Step 1 is behavior-preserving except at exact boundaries
(`0 ms` → full interval), so reverting it restores the busy-loop bug and
drops the new tests, but does not change any type, public path or dependency.
Steps 2 and 3 roll back by restoring the original definitions/call site.

## Reference precedent (`.vendor`, evidence only)

- Path + choice: `.vendor/vela-main/updates/src/check-time.h:7-16`
  (`vela_up_wall_deadline`) keeps the scheduling deadline arithmetic pure so
  tests can simulate discontinuous wall-clock changes without setting system
  time, and keeps the monotonic deadline fixed while only the displayed
  timestamp changes. Choice: adopt that pattern — pure, injectable, recomputed
  deadline math in the typed `flamewm-shell-core::clock` owner, with a fake
  clock in tests. No C code, layout or dependency is transplanted.
- Supporting path: `.vendor/vela-main/brightness/tests/fake_clock.hpp:15-38`
  models suspend as advancing only the boot clock; motivates the existing
  fake-clock approach in `clock.rs:78-96` and the new DST-jump cases here.
- `.vendor/vela-main/shell/src/clockformat.h:14-28` confirms the taskbar is the
  only clock that may show seconds — matches `rearm_clock`'s `show_seconds`
  gate, so no visual behavior changes.

`.vendor/**` was read only as evidence; it is not a dependency or runtime input.

## Verification status

UNBUILT by this worker (brief forbids `cargo`, builds, git and test runs). The
orchestrator must run the shell-core test target; step 1 adds three tests and
changes no public signature.
