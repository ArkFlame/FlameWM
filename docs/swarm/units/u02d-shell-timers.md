# u02d — Shell timers and wakeups audit (narrow retry)

**Status: UNBUILT.** No `git`, `cargo`, Python, build, test or sleep-loop command was run, per the
worker contract. Only `rg`, `sed`, `wc` and bounded file reads were used. The orchestrator owns the
build, test and visual check for the shared checkout; nothing here is compiled or runtime-proven.

**Scope:** `crates/flamewm-shell/src` only. `projection/start.rs`, `async_projection/start_view.rs`,
`async_projection/icon_cache.rs` and `app/icons.rs` were treated as other workers' files: they were
not edited. Routes loaded: `flamewm-source-first`, `flamewm-performance-proof`, `scoped-perf-tests`;
`flamewm-rust-systems` was not needed because this pass applies no Rust edit. Source and current
call paths outrank the earlier `u02-shell-wakeups.md` narrative.

## Method

`rg` for `register_timer`, `register_wall_clock`, `Duration::from_millis`, `Duration::from_secs`,
`interval`, `tick`, `deadline`, `next_tick`, `wake.set` and `sleep` under
`crates/flamewm-shell/src`, then full reads of each owning function. `flamewm-reactor/src/lib.rs`
was read as evidence only for `wall_clock_delay_ms`. Wakeups per second are steady-state rates for
one owner, with the profiler off unless stated.

## Inventory — every repeating timer and wake deadline

| # | file:line | Period | What it refreshes | Wakeups/s | Alignment today |
| --- | --- | --- | --- | --- | --- |
| 1 | `main.rs:911-915` | Next minute boundary (60 s); `wall_clock_delay_ms` in `flamewm-reactor/src/lib.rs:436-444` | Sets `clock_due`, which paints the panel clock text | 0.0167 | Self-realigning wall-clock source; no catch-up drift |
| 2 | `main.rs:916-922` | `STARTUP_HEAP_TRIM_DELAY` 10 s (`main.rs:51`), one shot | Releases free heap after startup | 0 steady | One shot |
| 3 | `main.rs:1121-1128` | 5 s, one shot; profiler only | First profiler window | 0 when profiling off | One shot and gated |
| 4 | `main.rs:1158-1164` | `profile_interval_secs()`, floor 10 s (`flamewm-profiler/src/lib.rs:80-84`), repeating | Profiler window flush; no visible content | 0 when profiling off, <=0.1 on | Gated on `flamewm_profiler::enabled()` |
| 5 | `main.rs:1410-1432`, call at `main.rs:933-935` | `BUS_RETRY_INTERVAL` 30 s (`main.rs:53`), one-shot chain | Late session control-bus attach | 0.033 while degraded, 0 after success | Chain stops on the first successful attach; no repeating wake |
| 6 | `polkit_answer.rs:126`, timeout at `polkit_answer.rs:24` | `HELPER_ANSWER_TIMEOUT` 30 s, one shot | Kills an unanswered polkit helper | 0 at idle | Armed only for a live conversation |
| 7 | `quick_controls/host/run.rs:163-175` | `profile_interval_secs()`, floor 10 s, repeating | Popup-host profiler window flush; no visible content | 0 when profiling off, <=0.1 on | Gated on `flamewm_profiler::enabled()` |
| 8 | `app/timers.rs:213-225`, `app/timers.rs:325-332` | 1 s with `taskbarShowSeconds`, else 60 s; none when clock hidden | Clock text; date text at midnight and on forced ticks | 1 or 0.0167 | `clock_boundary_delay` arms to the next second or minute boundary plus 1 ms |
| 9 | `app/timers.rs:231`, `osd.rs:25-28,208-248` | `HOLD` 1200 ms, `FADE` 180 ms, `WORKSPACE_HOLD` 620 ms | Volume, brightness and workspace card hide | 0 at idle | Only while a card is live; single expiry deadline |
| 10 | `app/timers.rs:232`, `switcher.rs:29,306` | `HOLD_DELAY` 150 ms | Alt+Tab card after a switch | 0 at idle | Only while a switcher frame is held |
| 11 | `app/timers.rs:233`, `app/helpers.rs:185` | `ZONE_SETTLE` 1100 ms, one shot | Forced clock and weather reproject after a timezone change | 0 at idle | One shot |
| 12 | `app/timers.rs:234`, `app/ipc.rs:133`, `app/progress_gate.rs:8` | `MIN_INTERVAL` 100 ms | Task progress fills during a progress burst | 0 at idle | Gate publishes a deadline; no polling loop |
| 13 | `app/timers.rs:236`, `weather_host.rs:22,191-200` | `WEATHER_REFRESH_INTERVAL` 1800 s | Weather tray feed | 0.00056 | Only while enabled with a location; worker completion uses a result fd |
| 14 | `app/timers.rs:237-241`, `app/dock.rs:167,668` | Dock autohide and bounce deadlines | Dock hide and bounce frames | 0 at idle | Only while a dock animation is live |
| 15 | `app/timers.rs:242-244` | Bluetooth agent deadline | Bluetooth agent popup state | 0 at idle | Only while an agent is live |
| 16 | `app/timers.rs:245`, `usage_host.rs:20,71-75` | `USAGE_REFRESH_INTERVAL` 300 s default; 60-3600 s from settings | AI usage snapshots | 0.0033 | Deadline follows the fetch; worker completion uses a result fd |
| 17 | `app/timers.rs:246-248`, `app/usage.rs:8` | `USAGE_REPROJECT_INTERVAL` 60 s | Reset captions on the open usage card | 0.0167 | Only while the card is open |
| 18 | `main.rs:560-614`, `main.rs:81` | 15 ms pointer-equivalent poll, capped by `ICON_WAIT_LIMIT` 2 s | Toast icon on the visible notification | 66.7 while in flight, 0 at idle | Bounded one-shot wait; stops when no toast wants the icon |
| 19 | `main.rs:718-725` | Provider toast expiry | Toast hide | 0 at idle | Earliest toast deadline only |
| 20 | `launch_feedback.rs:21-25,224-268` | `POLL_MOVING` 50 ms while the pointer moved within `MOTION_WINDOW` 1 s | Launch badge placement under the pointer | 20 while a badge is in flight | Deadline from `last_poll`, active-only; no idle timer |
| 21 | `launch_feedback.rs:23-25,224-268` | `POLL_STILL` 250 ms after the pointer has been still for 1 s | Nothing until motion resumes, then badge placement | 4 while a badge is in flight | Same active-only deadline; no idle timer |
| 22 | `launch_feedback.rs:26-29,104-113,457-466` | `INDICATOR_FRAME_MILLIS` 120 ms | Pending-launch dot animation frame | 8.3 while pending | Frame deadline; `None` once painted |
| 23 | `notification_center.rs:21-22,42-50` | `CLOSE_LATCH_TTL` 300 ms | Single-use opener-release latch | 0 | Checked on the release event; no wake |
| 24 | `event_router/dismiss.rs:44` | 300 ms | Dismissal window | 0 | Event-driven deadline, not a poll |
| 25 | `weather_auto.rs:21,135-177` | `AUTO_LOOKUP_INTERVAL` 21600 s | IP-derived weather location | 0 | No dedicated wake; checked on enable, network edge or an existing tick |
| 26 | `app.rs:25`, `app/timers.rs:164-174` | `CLOCK_HEAP_TRIM_INTERVAL` 600 s | Free-heap release inside a clock tick | 0 | Gated by elapsed time; it does not create a wake |

The only unbounded repeating timers owned by the shell are the profiler windows (#4 and #7), and
both are registered only when profiling is enabled. Every other entry is boundary-aligned,
deadline-driven, one-shot, or gated on a live card, launch, fetch or degraded-start state.

## The two fastest recurring sub-second wakes

These are the two candidates the brief's "under 1 s" test selects. Both are in a file this worker
owns; neither is a clock or tray poll, so the example in the brief does not literally occur in the
current source.

| # | file:line | Before (wakeups/s) | After (wakeups/s) | Change and risk |
| --- | --- | --- | --- | --- |
| 20 | `launch_feedback.rs:21-22,76-82,224-268,454-466` (`POLL_MOVING` 50 ms) | 20 while a visible badge follows recent pointer motion; 0 at idle | 20 / 0 — unchanged | **No edit (UNBUILT).** The wake is already the next visible-change deadline: `poll_deadline(last_poll, interval)` arms exactly one wake, `next_tick` folds it with the launch expiry and dot frame, and `next_tick(None, None, None)` is `None` once no launch remains. Lengthening the period would drop badge positions during motion; replacing it with an event needs a pointer-motion source that XFixes does not provide over other clients' windows. Risk if changed: visible badge lag. |
| 21 | `launch_feedback.rs:23-25,76-82,224-268,454-466` (`POLL_STILL` 250 ms) | 4 while a badge is visible and the pointer has been still for at least 1 s; 0 at idle | 4 / 0 — unchanged | **No edit (UNBUILT).** These four ticks per second produce no paint while the pointer is still; they exist only to notice motion resuming. There is no fd readiness for `XQueryPointer`, so an event-driven replacement would have to come from a new native source outside this scope. Lengthening the period delays re-detection and leaves the badge visibly behind. Risk if changed: visible badge lag; out-of-scope native source. |

Neither candidate can be aligned to the next *visible* change without changing what the user sees.
The nearest genuine "wake faster than visible change" cases are also already safe:

- `main.rs:610` (15 ms, #18) is bounded by a 2 s limit and stops when no toast wants the icon; its
  wake equals icon arrival, the visible change.
- `main.rs:1160` and `quick_controls/host/run.rs:167` (#4, #7) wake faster than any visible change
  but are profiler-only and gated off in normal use; removing the gate would remove the profiling
  feature the user enabled.
- `app/timers.rs:245` (#16) and `app/timers.rs:246-248` (#17) were checked against
  `u02-shell-wakeups.md`: gating them on an open card would show stale snapshots or reset captions on
  first open, so the visual rule rejects the cut.

## Applied edits

None. This narrow retry changed no Rust source; only this unit document was added. The two fastest
sub-second wakes are already deadline-driven and active-only, and every other entry in the inventory
is aligned, one-shot, gated or event-driven. No two wakes can be made event-driven or moved to the
next visible change with identical visuals inside the owned files.

## Verification

- No `git`, `cargo`, Python, build, test or sleep-loop command was run; this is static source
  inspection only. All statements stay **UNBUILT** and need the orchestrator's build, test and visual
  check after the batch lands.
- Evidence consulted: the `crates/flamewm-shell/src` files named in the tables, plus
  `crates/flamewm-reactor/src/lib.rs:432-495` and `crates/flamewm-profiler/src/lib.rs:52-84` for the
  period rules. Earlier `u02-shell-wakeups.md` and `u18c-lock-idle.md` were read as prior audit
  evidence, not as authority over current source.

PARTIAL — the full repeating-timer inventory is recorded with period, refreshed content and wakeup
rates, and the two sub-second wakes were picked and rowed. No code change was applied: both are
already aligned to their next visible deadline and active-only, and every other candidate is
one-shot, profiler-gated or event-driven, so no wake can be moved or removed while keeping identical
visuals. Everything in this unit remains UNBUILT pending the orchestrator's build and visual check.
