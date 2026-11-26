# u93-scenario-load log

## Why the old run showed no load

Followed the scenario end to end on this host (nested `winit` compositor, private XWayland display,
`/usr/bin/flamewm-terminal`):

- The client **does map a window**. `xwininfo -root -tree` on the nested display lists
  `0x200006 880x540+60+91` with child `"FlameWM Terminal" ("flamewm-terminal") 880x540+0+0`.
- The PTY feed **does run**: a marker-file command passed through `-e` executed all its bursts, and
  the terminal process burned 23–42 % of one core for the whole sampled window (`ps -p`).
- The client is **X11-only**: `crates/flamewm-terminal/Cargo.toml` depends on `flamewm-ui-x11`, and
  `crates/flamewm-terminal/src/main.rs` calls `flamewm_ui_x11::run_with_controller_events_role_and_two_readable_fds`.
  There is no Wayland client path, so `WAYLAND_DISPLAY` alone (no `DISPLAY`) is not a supported
  option for this binary and cannot be used.
- The compositor still reported `wait share=99%` while the client was painting at 25–42 % CPU.
  So the missing load is **not** the client and **not** the feed: the host session starves the
  nested winit window of frame callbacks (same conclusion as `LOG-u92-active-scenario.md`). The
  compositor renders XWayland damage only in sporadic stalls (observed 7.6 %, 17.8 % and 26.4 %
  `render` single ticks, each a 100–280 ms render call), and those ticks do not land predictably.
- One reporting trap made this worse: the profiler's **first** `report_window()` call after
  `FLAMEWM_PROFILE_COMPOSE=1` anchors on its default 60 s interval, so tick 1 prints
  `wait share=100.0% wall≈59973ms` and divides every row by 60 s. The old parser could select that
  diluted tick (0.1–0.7 %) when the genuine one-second ticks were fully parked.

## Changes in `scripts/compose-scenario` (only owned path)

1. **Continuous burst feed.** The client now streams `yes | head -n 2000` in a loop until killed,
   instead of stopping after one second. The previous one-second burst could finish during client
   startup, leaving every sampled tick idle. A single unbounded `yes` was also tried and rejected:
   it keeps the PTY permanently readable and the client rendered almost nothing.
2. **Wait for client readiness, then warm-up.** Poll the client log for
   `FLAMEWM_RENDER_TEXT_BACKEND` (logged once before the window loop), then pause 1 s so the window
   map/first frame falls outside the measured ticks.
3. **Longer sampling window** (5 × 1 s ticks) because busy ticks are sporadic on a host that
   starves nested frame callbacks; the busiest tick is still a one-second sample.
4. **Ignore the 60 s-anchored first tick.** A tick is eligible only when its `wait` row reports
   `wall <= 2000ms`, so diluted startup spans are never reported as the composition.
5. **No-spans ticks count as parked.** The profiler's one-line `share=wait:100.0` tick is treated as
   an eligible zero-work window instead of being lost.
6. **LOAD-NOT-REACHED guard.** After printing the shares, the script sums the busiest eligible
   tick's non-`wait` shares. If the sum is `< 5.0 %` it prints
   `FLAMEWM-COMPOSE-SCENARIO: LOAD-NOT-REACHED ... < 5%` and exits **3**; an empty/absent eligible
   tick exits 3 too. An idle run can no longer be reported as an active-use measurement.

## Evidence after the change

Same host, four runs (no git/cargo in this unit; `bash -n` clean):

```
FLAMEWM-COMPOSE-SCENARIO: shares (percent of the sampled second, self wall):
  wmwayland.dispatch.render share=0.9
  wait share=99.0
FLAMEWM-COMPOSE-SCENARIO: LOAD-NOT-REACHED busiest tick active share=1.1% < 5%
exit=3
```

With the terminal live, `ps -o pcpu= -C flamewm-terminal` read `42.0, 32.5, 27.6, 25.5, 24.9,
23.5` at one-second intervals, and `xwininfo` showed the mapped `880x540` window. Manual nested
runs with the same binary did produce isolated busy ticks (`render share=7.6`, `17.8`, `26.4`), which
the new parser accepts when they occur and the guard rejects when they do not.

## Expected composition shift

None in product code: this unit changes only the measurement harness. The harness now proves
whether the client actually paints (exit 3 / LOAD-NOT-REACHED) instead of silently reporting idle
`wait` as the active second.

## Residual risk / next unit

The nested host here does not deliver enough frame callbacks to sample a steady active second. The
real fix is in the compositor's XWayland presentation path (`crates/flamewm-wm-wayland`, not owned by
this unit): investigate why an X11 client at 25–42 % CPU produces only a handful of render
dispatches per second while `wait` stays above 99 %. Until then the guard is the correct output on
this host.
