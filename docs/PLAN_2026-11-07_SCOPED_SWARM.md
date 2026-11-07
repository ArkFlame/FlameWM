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

### Results: A (X11 client compatibility)
Probe: `/tmp/claude-1000/fw-a/probe_a.py` (python-xlib, DISPLAY=:92, isolated WM from `target-swarm-q/debug/flamewm`).
- OK: types dialog/utility/splash/notification/dock/dropdown_menu/tooltip map and get `_NET_FRAME_EXTENTS` (2,2,32,2); transient child stacks above parent; min/max/inc/base hints clamp client resize; `_NET_WM_STATE_FULLSCREEN` add/remove; `_NET_ACTIVE_WINDOW`; `WM_DELETE_WINDOW` on `_NET_CLOSE_WINDOW`; override-redirect untouched; 200 map/destroy burst, WM alive.
- FIXED: `_NET_MOVERESIZE_WINDOW` had no handler (`wm/events.rs` dispatch, `wm/configure_handlers.rs::handle_moveresize_window` feeds the ConfigureRequest owner). Before: size unchanged. After: width/height applied (250x180 observed). Test: `wm::configure_handlers::moveresize_tests::moveresize_flags_select_geometry_axes`.
- OPEN, not fixed: `_NET_WM_STATE_ABOVE`/`BELOW` ignored, newer normal window stacks above (`wm/events.rs::apply_net_wm_state` has no layer). Needs stacking-layer design.
- OPEN, not fixed: `_NET_WM_PING` never sent; `close()` (`wm/window_ops.rs:117`) sends WM_DELETE only, so a hung client has no timeout/force-kill path.
- OPEN, not fixed: `_NET_WM_STATE_DEMANDS_ATTENTION` ignored (no handler, no taskbar attention).
- UNRESOLVED: x/y of `_NET_MOVERESIZE_WINDOW` and a plain client ConfigureRequest did not move the window in the last isolated run; `wm.configure.summary requests=0`. Not yet explained; do not treat as fixed.
- UNVERIFIED: `_GTK_FRAME_EXTENTS`, aspect hint, dialog centring relative to parent, dock strut placement.
- Checks: `cargo test -p flamewm-wm-x11` exit 0 (525 passed); `cargo build -p flamewm-wm` exit 0.

### Results: B (startup time-to-ready)
- Probe: `scratchpad/ready_probe.py` + `run_bench.sh` (display :91, release bins copied to `/tmp/claude-1000/fw-b/base-bin`, start.sh order: wm t=0, shell +3s, desktop +2s). Ready = first viewable top-level window (shell title `FlameWM panel`, desktop `FlameWM Desktop`), wm = `_NET_SUPPORTING_WM_CHECK` on root. Proxy for first map, not first painted frame.
- BEFORE (3 cold starts, ms from own spawn): wm 28.3 / 29.8 / 33.3 (median 29.8); shell 34.9 / 43.9 / 62.2 (median 43.9); desktop 86.5 / 181.2 / 162.2 (median 162.2). Raw: `/tmp/claude-1000/fw-b/base.{1,2,3}.json`.
- Profiler (`FLAMEWM_PROFILE=1`): wm `wm.startup.catalog` 8.1 ms + `wm.startup.host` 9.2 ms (on path). Desktop first two frames: `render.layout` 50 ms wall / 47 ms CPU, `render.draw` 28 ms; layout cost is first Xft use (`ensure_xft` in `flamewm-render-x11/src/app/render.rs:~200`, `xft.rs::font_for` opens fonts lazily). Out of this worker's scope.
- Candidates not changed: desktop scans the directory twice at startup (`flamewm-desktop/src/main.rs` ~L163-177, rescan after layout-store positions); removing it changes ordering semantics, not measured. Desktop `query_settings` is a sync control roundtrip (`refresh.rs:69`), `refresh.rs` dirty under another worker.
- AFTER: none. No code changed, so no before/after delta claimed. Ready eprintln not added (external probe covers it).
- Blocking item for a real win: move Xft/font open off the desktop main thread (needs its own X connection; large change) or make the first frame skip text until fonts are ready. Not attempted.
- Tests: none run (no source change). Cargo build of wm/shell/desktop release at working-tree state exit 0 (wm built after another worker's `atoms.rs` edit settled).

### Results: A2 (X11 compatibility follow-up)
Harness: `/tmp/claude-1000/fw-p2/` (display `:88`, WM only). Probes: `probe_p2.py` (ping, layers, moveresize), `probe_p3.py` and `probe_p4.py` (observations). Logs: `run2.out`, `run3.out`, `run4.out`.
- FIXED, ping and force-kill: `wm/ping.rs`. Close sends WM_DELETE, then `_NET_WM_PING` when the client lists it in WM_PROTOCOLS. One reactor deadline (5 s, folded into the wait in `runtime.rs`). A pong on root (`data[2]` = client) clears it. A missed pong marks the window unresponsive, and the next close runs `XKillClient`. Live: responsive client survives the second close (P1 PASS); silent client force-killed on the second close (P2 PASS).
- NOT DONE, ping: no periodic pings; no visible "not responding" UI (second close click kills).
- FIXED, ABOVE/BELOW: `wm/stack_layer.rs`. The `_NET_WM_STATE` client message sets the layer. After a focus raise, only misplaced frames are restacked (Below under the lowest normal frame, Above to top), and `_NET_CLIENT_LIST_STACKING` is republished. Live L1-L3 PASS (L3 discriminates: removed ABOVE no longer pins the window).
- NOT DONE, ABOVE/BELOW: `_NET_WM_STATE` set at map time is ignored; `_NET_WM_STATE` is not republished on the client.
- FIXED, `_NET_MOVERESIZE_WINDOW` x/y (the "requests=0" mystery): root cause in `frame/geometry.rs::plan_client_configure`. When a size was also requested, the requested origin was discarded and gravity applied to the previous origin. New `plan_client_configure_explicit(..., origin_requested)`; the ConfigureRequest handler passes X|Y. Live M1: x/y and w/h applied (requested 444,300 gave 445,301; the +1 is the baseline offset plain configure also shows). Test: `frame::geometry::...::client_configure_explicit_origin_wins_over_gravity_on_resize`. Risk: any ConfigureRequest with X/Y and W/H now honours the origin (ICCCM). Size-only requests are unchanged.
- NOT DONE, `_NET_WM_STATE_DEMANDS_ATTENTION` and WM_HINTS urgency: no code. Needs a `WindowSnapshot.attention` field across about 15 constructors, control-wire encode/decode, and the shell projection.
- VERIFIED, `_GTK_FRAME_EXTENTS`: CSD client gets `_NET_FRAME_EXTENTS` `[0,0,0,0]`. Position varied between runs; not asserted.
- VERIFIED, aspect hint 1:1: 400x200 request gave 200x200.
- GAP, dialog centring: a DIALOG transient over a 400x300 parent at (401,301) mapped at (2,32), not centred. Not implemented.
- VERIFIED, dock strut (set before map): `_NET_WORKAREA` `[0,40,1600,860]`, dock at 0,0; destroy restores `[0,0,1600,900]`.
- CORRECTION to Results A: probe_a.py passed `t.lower()` to `T()`, so type atoms such as `_NET_WM_WINDOW_TYPE_dock` never existed and those windows were Normal. The "types ... OK" claims in Results A need re-verification with uppercase atom names.
- Checks: `cargo test -p flamewm-wm-x11` exit 0, 534 passed, 0 warning lines (`test_full.log`). `cargo build -p flamewm-wm` exit 0, 0 warnings. Focused: `cargo test -p flamewm-wm-x11 -- ping_tests stack_layer_tests moveresize_tests client_configure` exit 0 (22 passed). `-p flamewm-api` and `-p flamewm-shell` untouched, not run.
- Changed: `src/atoms.rs`, `src/wm/{mod,construct,events,window_ops,manage}.rs`, `src/runtime.rs`, `src/frame/geometry.rs`, `src/wm/configure_handlers.rs`, new `src/wm/ping.rs`, new `src/wm/stack_layer.rs`. Earlier uncommitted edits by other workers were preserved.
