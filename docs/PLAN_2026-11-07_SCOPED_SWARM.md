# Plan 2026-11-07: scoped swarm (fast, estimate-driven)

Principle: small timed probes (10-20 s, /proc deltas, one counter, one scoped
test) find a *likely culprit*; fix it; re-run the same probe; accept when the
number moves clearly. No full-workspace profiling runs inside a worker.

## State at start
- Pushed through f7fd3a09 (tray, Tile/Focus keys, XI2 hot corners, restore clamp,
  Alt+Shift+Tab, pager occupancy, XWayland clipboard + PRIMARY, helper dedupe).
- Running: robustness audit (unwrap/panic/unbounded), leak soak.
- Known open: clock tick full layout (~1 ms/s, risky), popup cold-open Xft init
  (~10 ms, lives in lazily spawned quick-control host), multi-monitor untested
  (Xvfb exposes one output).

## Workstreams (this round)
| # | Area | Probe (scoped) | Accept |
|---|------|----------------|--------|
| A | X11 client compatibility | tiny python-xlib / xmessage / xterm clients: _NET_WM_WINDOW_TYPE (dialog, splash, dock, utility, notification), WM_TRANSIENT_FOR, min/max size hints, _NET_WM_STATE (fullscreen, above, skip_taskbar, demands_attention), WM_DELETE timeout for hung client, _GTK_FRAME_EXTENTS | each case maps/places/stacks correctly; no WM log warnings |
| B | Startup time + parallel init | timestamped log of time-to-ready per process (wm, shell, desktop) over 3 cold starts | critical path shortened by moving independent init (icon index, settings parse, wallpaper decode, font open) off it; before/after median |
| C | Graceful degradation | start each process with: no session bus, no pulse, no NetworkManager/bluez, missing icon theme/fonts, read-only HOME, corrupt settings file | no panic, no log spam (>1 line/event), UI still usable |

## Rules
- Workers finish in the foreground; only scoped `cargo test -p <crate>` inside
  workers. The orchestrator runs the workspace test + guard once before commit.
- Each worker appends findings (culprit, probe, before/after) to
  `docs/PLAN_2026-11-07_SCOPED_SWARM.md` under "Results" using its own heading.
- No UI design changes. No timers under 50 ms when idle.

## Results
(workers append below)
