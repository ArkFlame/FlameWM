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
| Polkit helper answer timeout | `polkit_answer.rs:126` (`HELPER_ANSWER_TIMEOUT`, `polkit_answer.rs:24`) | 30 s one-shot timer kills an unanswered helper. | None: armed only after a helper starts a conversation and dropped with it; no timer exists at idle. | Low: event-driven timeout, no idle wake to cut. |
| Startup heap-trim / first profiler one-shot | `main.rs:917`, `main.rs:1124` | One-shot startup heap trim and first profiler window (5 s). | None: both are one-shot; the profiler one is gated on `flamewm_profiler::enabled()` just like the repeating window, so a profiler-off shell registers neither. | Low: no repeating idle wake. |

## Verification

- No `cargo`, `git`, tests, builds, Python or sleep loops were run in this unit, per the brief;
  all four code edits are **UNBUILT** and need the orchestrator's build and visual check.
- Static check only: each deferred waker is passed to the same spawn function, and the bus retry
  chain preserves the original "attach once, never re-attach" behavior while dropping the wake
  after success.

## Step 1 — raw timer scan (third try, narrow)

Command exactly as given in the brief:

```text
rg -n 'Duration::from_(millis|secs)' crates/flamewm-shell/src --glob '!*test*' | head -40
```

The scan matches 92 lines in total, and `rg` emits files in parallel, so which lines land inside
`head -40` varies between runs. The record below is the same scan re-run with `-j1` (single
thread), which is reproducible; the glob filters file names only, so `#[cfg(test)]` blocks inside
matching files are present. All edits below remain **UNBUILT**.

```text
crates/flamewm-shell/src/polkit_answer.rs:24:pub const HELPER_ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
crates/flamewm-shell/src/polkit_answer.rs:251:        let deadline = Instant::now() + Duration::from_secs(10);
crates/flamewm-shell/src/polkit_answer.rs:255:                .dispatch(Some(Duration::from_millis(20)))
crates/flamewm-shell/src/polkit_answer.rs:280:            run_script(HANG, "hunter2", Duration::from_millis(200)),
crates/flamewm-shell/src/osd.rs:25:pub const HOLD: Duration = Duration::from_millis(1200);
crates/flamewm-shell/src/osd.rs:26:pub const FADE: Duration = Duration::from_millis(180);
crates/flamewm-shell/src/osd.rs:28:pub const WORKSPACE_HOLD: Duration = Duration::from_millis(620);
crates/flamewm-shell/src/osd.rs:583:        assert_eq!(WORKSPACE_HOLD + FADE, Duration::from_millis(800));
crates/flamewm-shell/src/osd.rs:603:        assert!(!state.observe_workspace(1, true, t0 + Duration::from_millis(100)));
crates/flamewm-shell/src/osd.rs:629:                .checked_sub(Duration::from_millis(1))
crates/flamewm-shell/src/osd.rs:633:        assert!(state.expire(t0 + Duration::from_millis(800)));
crates/flamewm-shell/src/osd.rs:823:        assert!(state.observe(Level::brightness(70), t0 + Duration::from_millis(100)));
crates/flamewm-shell/src/osd.rs:826:            Some(t0 + Duration::from_millis(100) + HOLD)
crates/flamewm-shell/src/osd.rs:828:        assert!(!state.observe(Level::brightness(70), t0 + Duration::from_millis(400)));
crates/flamewm-shell/src/osd.rs:831:            Some(t0 + Duration::from_millis(100) + HOLD)
crates/flamewm-shell/src/osd.rs:833:        assert!(state.observe(Level::new(50, false, 150), t0 + Duration::from_millis(500)));
crates/flamewm-shell/src/osd.rs:915:        assert!(!state.observe(Level::new(40, false, 150), t0 + Duration::from_millis(500)));
crates/flamewm-shell/src/osd.rs:916:        assert!(state.observe(Level::new(45, false, 150), t0 + Duration::from_millis(500)));
crates/flamewm-shell/src/osd.rs:919:            Some(t0 + Duration::from_millis(500) + HOLD)
crates/flamewm-shell/src/osd.rs:928:        state.observe(Level::new(45, false, 150), t0 + Duration::from_millis(100));
crates/flamewm-shell/src/osd.rs:929:        state.observe(Level::new(50, false, 150), t0 + Duration::from_millis(900));
crates/flamewm-shell/src/osd.rs:932:            Some(t0 + Duration::from_millis(900) + HOLD)
crates/flamewm-shell/src/osd.rs:937:            Some(t0 + Duration::from_millis(900) + HOLD)
crates/flamewm-shell/src/osd.rs:947:        assert!(!state.expire(t0 + (HOLD.checked_sub(Duration::from_millis(1)).unwrap())));
crates/flamewm-shell/src/osd.rs:962:        assert!(!state.expire(t0 + (HOLD.checked_sub(Duration::from_millis(1)).unwrap())));
crates/flamewm-shell/src/osd.rs:985:        let t1 = t0 + HOLD + Duration::from_millis(50);
crates/flamewm-shell/src/osd.rs:988:        assert!(!state.expire(t1 + (HOLD.checked_sub(Duration::from_millis(1)).unwrap())));
crates/flamewm-shell/src/app/usage.rs:8:pub(super) const USAGE_REPROJECT_INTERVAL: Duration = Duration::from_secs(60);
crates/flamewm-shell/src/app/timers.rs:224:            now + clock_boundary_delay(display.show_seconds) + std::time::Duration::from_millis(1)
crates/flamewm-shell/src/app/timers.rs:331:    std::time::Duration::from_millis(flamewm_reactor::wall_clock_delay_ms(now_ms, with_seconds))
crates/flamewm-shell/src/app/timers.rs:338:        assert!(super::clock_boundary_delay(false) <= std::time::Duration::from_secs(60));
crates/flamewm-shell/src/app/timers.rs:339:        assert!(super::clock_boundary_delay(true) <= std::time::Duration::from_secs(1));
crates/flamewm-shell/src/app/timers.rs:347:        let soon = t0 + Duration::from_millis(100);
crates/flamewm-shell/src/app/helpers.rs:185:pub(super) const ZONE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1100);
crates/flamewm-shell/src/app/panels.rs:350:            Duration::from_millis(500),
crates/flamewm-shell/src/app/progress_gate.rs:8:pub(crate) const MIN_INTERVAL: Duration = Duration::from_millis(100);
crates/flamewm-shell/src/app/progress_gate.rs:47:            gate.decide(start + Duration::from_millis(10)),
crates/flamewm-shell/src/app/progress_gate.rs:51:            gate.decide(start + Duration::from_millis(99)),
crates/flamewm-shell/src/app/progress_gate.rs:64:            if gate.decide(start + Duration::from_millis(ms)) == PaintDecision::Now {
crates/flamewm-shell/src/app/popups.rs:188:            Duration::from_millis(500),
```

Recurring-interval constants seen in the rest of the 92-line scan, with their literal durations:
`launch_feedback.rs:22` 50 ms, `launch_feedback.rs:24` 250 ms, `main.rs:53` 30 s,
`main.rs:1160` `profile_interval` (>= 10 s, profiler-gated), `usage_host.rs:20` 300 s,
`app.rs:25` 600 s, `weather_host.rs:22` 1800 s, `weather_auto.rs:21` 21600 s. A 15 ms icon
deadline (`main.rs:610`) and a 10 ms test-only sleep (`weather_host.rs:441`) are bounded one-shots,
not periodic intervals.

## Step 2 — three shortest periodic intervals

The three shortest recurring intervals are the 50 ms and 250 ms launch-feedback pointer polls and
the 30 s degraded-start bus retry. Each was read in full; a row follows each one.

| Area | file:line | Cost | Change | Risk |
| --- | --- | --- | --- | --- |
| Launch-feedback moving pointer poll | `launch_feedback.rs:22` (`poll_interval` at `:76-82`, deadline at `:263-267`) | 20 pointer queries per second, but only while a visible launch badge follows recent pointer motion. | **None (UNBUILT):** already active-only and deadline-driven; `next_tick` returns `None` once the last launch ends (`:218-222`) and the `concealed` branch returns `expiry` with no poll (`:249-258`), so an idle shell owns no badge timer. | Lengthening delays badge tracking, a visible change; not provably idle-safe with identical visuals, so left untouched. |
| Launch-feedback still pointer poll | `launch_feedback.rs:24` (`MOTION_WINDOW` at `:25`) | 4 pointer queries per second while a badge is visible and the pointer has been still for at least 1 s. | **None (UNBUILT):** same active-only chain; `POLL_STILL` applies only between `MOTION_WINDOW` and launch expiry, and no launch means no timer. Lengthening would also delay re-detection of motion, since motion is observed through this poll. | Visual: the badge would lag after motion resumes; not provably idle-safe with identical visuals, so left untouched. |
| Degraded-start bus retry | `main.rs:53` (`arm_bus_retry` at `:1413-1432`, call at `:933-935`) | A repeating 30 s wake survived every successful bus attach (2 idle wakeups/min); the callback only checked the flag and returned. | **EDIT (UNBUILT, made earlier in this unit):** the `repeat = true` registration was replaced by `arm_bus_retry`, a self-rearming one-shot chain that stops after the first successful attach. Confirmed present in source by read-only inspection this try. | Low: calloop allows inserting a source from a dispatch callback and the fired one-shot drops itself; attach behavior is unchanged because the old timer never re-attached after success. |

All three rows are from static source inspection only; no compiler, runtime, build or test evidence
was collected here. Every code edit in this unit remains **UNBUILT** and needs the orchestrator's
build and visual check.

PASS — the raw scan was recorded and the three shortest recurring intervals were read and rowed;
the two badge polls are already active-only and event-driven, and the 30 s bus retry was already
converted to the one-shot chain in this unit, so this pass adds no further code edit. All findings
stay UNBUILT pending the orchestrator's build, test and visual verification.
