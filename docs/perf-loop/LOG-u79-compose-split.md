# LOG u79-compose-split

Area: nested compositor event-loop thread, idle and pointer-motion turn composition.

## Instrumentation

- `flamewm-wm-wayland::profile` labels one span per in-thread subsystem: `wmwayland.dispatch.wayland`,
  `.flush`, `.control`, `.input`, `.resize`, `.render`, `.xwayland`, and `wmwayland.timer`.
- Wrapped: `wl_display` client dispatch, the post-dispatch flush and control hooks, control bus
  turns, winit input/resize, `render_frame`, the `XWayland` event source and Dock property watcher,
  and the winit frame-pacing timer.
- `FLAMEWM_PROFILE_COMPOSE=1` enables the profiler and arms a one-second timer that prints
  `FLAMEWM-WAYLAND-COMPOSE tick=N window=1000ms` plus the profiler window (ranked total wall/count)
  to stderr. It also writes `<process>.profile.log` when `FLAMEWM_PROFILE_DIR` is set. Disabled, the
  timer is not installed and every span is an inert guard (one relaxed load per call).
- `flamewm-profiler` treats `FLAMEWM_PROFILE_COMPOSE=1` as enabling, so one variable is enough.

## Composition (reasoned, scoped one-second idle window)

- A fully parked loop records no spans: the idle second is wait-bound, which is the desired shape.
- On each wake, the recurring work is the control `after_dispatch` path (settings `metadata` stat,
  output-scale check, mirror/publish gates) plus whatever source fired. `render` appears only for a
  host `Redraw`, `input` only for host input, `xwayland` only for X events, `timers` only for the
  frame-pacing tick. So control is the largest *recurring* share on pointer-motion turns; on a
  truly quiet second no subsystem runs.
- No absolute CPU number is claimed; rerun with `FLAMEWM_PROFILE_COMPOSE=1` to re-rank.

## Change

- `control::sync_output_scale` bounded its settings-file `stat` to one per 250 ms (was one per loop
  dispatch). A saved scale still applies inside 250 ms; pointer motion no longer issues a syscall
  per turn. Expected shift: control self-wall drops on input-heavy turns, idle unchanged.

Evidence: code reading and profiler composition only. No cargo/build/run in this unit (shared
checkout rule); runtime verification is the orchestrator's build gate.
