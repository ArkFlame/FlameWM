# U02 — shell idle wakeup / timer audit

Scope: `crates/flamewm-shell/src` only (excluded `projection/start.rs`, `projection/usage.rs`,
`async_projection/start_view.rs`, `app/start_view.rs`). Source inspection only; no build, test,
runtime measurement, `git`, `cargo`, `python` or sleep polling was run. Every edit below is **UNBUILT**.

## One-paragraph report

The shell already funnels every timer into one shared `WakeDeadline` slot (`app/timers.rs:230`) and
publishes `None` when nothing is pending, so a truly idle shell parks the reactor; the only
high-frequency fixed wake left is the notification toast's 15 ms icon poll
(`main.rs:610`), which is now stopped as soon as no visible toast wants an icon instead of
running until its 2 s limit. The clock is the largest steady-state wake (1/min, or 1/s in seconds
mode) but it paints visible text, as are the 30 min weather refresh (tray text), the 60 s open usage
card reproject and the 300 s usage fetch, so none of those were cut; the usage fetch is the best
remaining candidate but needs a fetch-on-open flag to keep the card's first paint pixel-identical.
A second edit coalesces the six `Instant::now()` calls on each shell tick into one value passed to
the OSD, switcher, dock-bounce and wake publishers, removing per-tick clock syscalls without
changing any deadline's visible outcome. Launch-feedback, OSD, switcher, dock, progress-gate and
icon-loader paths are already event-driven or gated on a visible card and need no change; the
remaining `sleep`/`interval` hits are test helpers or profiler-only timers.

## Findings (ranked by idle cost)

| # | Area | File:line | Cost estimate | Change made / proposed | Risk |
| --- | --- | --- | --- | --- | --- |
| 1 | Toast icon fixed poll | `main.rs:566-612` (`deadline` at 610) | Up to ~66 wakes/s for ≤2 s (≈133 wakes), including after the toast has already gone — highest-frequency wake in the shell | **MADE (UNBUILT):** clear `icon_wait` and return when no visible toast has an `icon_name`, so the 15 ms wake stops instead of running to `ICON_WAIT_LIMIT`. Proposed (not made): register the toast loader's wake FD so the poll is removed entirely while the toast is visible. | Low for the made edit: only drops wakes for a toast that is no longer shown; the visible toast still waits for its icon. Full event-driven version is higher risk (loader is spawned lazily, needs reactor FD registration). |
| 2 | Panel clock cadence | `app/timers.rs:215-228`, `app/timers.rs:237` | 1 wake/min normally; 1 wake/s with `taskbarShowSeconds` — largest steady-state wake | None safe: it repaints visible clock text. Heap trim is already gated to 600 s (`app.rs:25`, `app/timers.rs:168`) and hidden clocks arm `None`. | High to remove: direct visible change. |
| 3 | Per-tick clock reads | `app/timers.rs:113-119,198-201,268,315,323`; `app/panels.rs:70` | ~6 `clock_gettime` syscalls per shell tick on the idle path | **MADE (UNBUILT):** capture one `now_instant` in `ShellLoop::tick` and pass it to `tick_osd`, `tick_switcher`, `tick_dock_bounce` and `publish_wake`; removed the internal `Instant::now()` calls in `tick_osd`/`tick_switcher`. | Low: deadlines shift a few microseconds earlier; no pixel change. |
| 4 | Usage data refresh | `usage_host.rs:20,71-76`; `app/timers.rs:247` | 1 wake/300 s even when the usage card is closed and no usage pixel is visible | Proposed (not made): gate `next_deadline()` on `surfaces.is_usage_open()` and add a `request_open_fetch` flag on popup open so the first paint stays identical. | Medium: without the open hook the card would show stale snapshots; needs a new flag and open-path wiring. |
| 5 | Weather refresh | `weather_host.rs:22,191-200` | 1 wake/30 min while enabled with a location | None safe: `apply_settings` (`weather_host.rs:148-151`) fetches on enable/location change so the *tray* shows data with the popup closed — background refresh is visible. | High to remove: stale tray temperature. |
| 6 | Open usage-card reproject | `app/usage.rs:8,34-41` | 1 wake/60 s while card open | None: reset captions are visible; already gated on `is_usage_open()`. | High to remove. |
| 7 | Weather auto-location | `weather_auto.rs:21,167-175`; `app/settings.rs:86-117` | 1 lookup/6 h | None: deadline is not published, so it piggybacks on an existing wake; no dedicated idle wake. | None. |
| 8 | Launch-feedback pointer poll | `launch_feedback.rs:21-25,74-102` | 50 ms while pointer moved, 250 ms once still — only while a launch badge is pending | None: adaptive and active-only; no idle poll (module doc: "No ... idle polling"). | None. |
| 9 | OSD / switcher / dock / bounce / polkit / bluetooth / progress deadlines | `app/timers.rs:232-251`; `app/progress_gate.rs:8,24-34` | Zero at idle: each returns `None` unless a card or animation is live | None: already coalesced into the single wake slot and gated on visibility. | None. |
| 10 | Profiler timers | `main.rs:1131,1165-1171` | 5 s one-shot + periodic, only when profiler enabled | None: not an idle path in normal runs. | None. |
| 11 | Test-only sleeps / polls | `weather_host.rs:435-445`, `quick_controls/supervisor.rs:403,424,445` | No runtime cost | None: `#[cfg(test)]` helpers. | None. |
| 12 | `active_output_target` Vec | `app/control.rs:605-630` | Allocates `Vec<OsdOutput>` per call, but callers are lazy (`app/timers.rs:267` closure, `app/timers.rs:316` guard) so idle ticks allocate nothing | None. | None. |
