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

- Notifications spec coverage (CODER): Results: integrations-linux `cargo test -p flamewm-integrations-linux` exit 0 (25/25 notification-filtered tests pass); `cargo build -p flamewm-shell` exit 0, zero warnings; `cargo test -p flamewm-shell` BLOCKED: `crates/flamewm-shell/src/cursor_theme.rs:88` missing `error_log` (other worker). Live harness verification not run.

- Results: C (CODER, graceful degradation). Probe: `/tmp/claude-1000/fw-c/probe.sh <scenario>` (Xvfb :90, 20 s, per-process liveness via `/proc/<pid>/exe`, same-line >3 per log, panic grep, `shot.png`). Logs: `/tmp/claude-1000/fw-c/run-<scenario>/`.
- Matrix after fixes (wm / shell / desktop / explorer alive at 20 s; panics 0 everywhere):
  - base (dbus-run-session): all alive. NM/bluez not on bus: no log line about them, no failure.
  - nobus (no DBUS env, direct launch): all alive. Caveat: libdbus autolaunches a private session bus, so this is not a true no-bus run.
  - nobusaddr (`DBUS_SESSION_BUS_ADDRESS` to a missing socket, true no-bus): WM exits 1 with `flamewm: Unavailable: D-Bus connection failed: ...`; shell exits 1 with `connect control session: ...`; desktop and explorer alive and degrade (`startup settings subscription degraded`, `media widget unavailable`). Clear message, no panic. NOT FIXED: WM needs the control server (`runtime.rs:140`, `ControlServer::connect_session()?`). Making it optional is a design change; the message also lacks the "control session" context.
  - nopulse (`PULSE_SERVER` to missing socket): all alive; `flamewm-audio-helper: PulseAudio not ready after 2s` logged once.
  - nobluez: no bluez/NM on the harness bus, all alive. Logs show no NM/bluez lines.
  - noicons (empty `XDG_DATA_DIRS`, empty fontconfig): all alive. Window titles are blank (`FLAMEWM_RENDER_TEXT_BACKEND core-x11 reason=Xft could not open a UI font`). Screenshot `run-noicons/shot.png` shows panel, explorer and icons render; the titlebar has no title text. Known degradation, not a crash.
  - ro (`HOME` and `.config` read-only): WM, shell, explorer alive. Desktop exits 1: `Error: "create desktop directory /tmp/.../run-ro/home/Desktop: Permission denied (os error 13)"`. Clear message; the path is now included (was missing before).
  - corrupt (truncated or garbage settings.conf, window-rules.conf, window-geometry.conf, notification-history.tsv, state.json): all alive. Only `window-rules.conf not applied: line 1: 'rule' before any 'app ='` logged. No panic.
  - lang (`LANG=C LC_ALL=C`): all alive. Screenshot `run-lang/shot.png` renders correctly.
- Defects fixed (log spam, rule: same line >3 in one log):
  1. `flamewm-wm/src/main.rs` `apply_live_cursor` and `flamewm-shell/src/cursor_theme.rs` `sync_snapshot`: a persisting cursor reload failure re-logged on every settings sync (noicons: 36 lines in 20 s). Now logs once per distinct failure, clears on success. Helper: new `crates/flamewm-api/src/repeat_log.rs` (`RepeatLog`, test `repeats_are_suppressed_until_text_changes_or_clears`). Before 36, after 1.
  2. `flamewm-render-x11/src/app/mod.rs`: `FLAMEWM_RENDER_PROVIDER`, `FLAMEWM_RENDER_CURSOR_THEME`, and `FLAMEWM_RENDER_TEXT_BACKEND` banners were printed once per app (shell 7x, baseline). Now once per process via `std::sync::Once`. Before 7, after 1.
  3. `flamewm-desktop/src/main.rs`: `create desktop directory` error now names the path.
- Checks (actual exits): `cargo build --bin flamewm --bin flamewm-shell --bin flamewm-desktop --bin flamewm-explorer` exit 0 (x2, after fixes); `cargo test -p flamewm-api repeat_log` exit 0 (1 passed); `cargo test -p flamewm-wm` exit 0 (16 + 20 passed); `cargo test -p flamewm-shell cursor` exit 0 (2 passed, after fixing the test literal to `..Default::default()`). `flamewm-desktop` and `flamewm-render-x11` have no new tests and only compile-level changes; the desktop message change is not runtime-tested beyond the `ro` probe.
- Not done: the desktop cursor path (`flamewm-desktop/src/refresh.rs:301`) has the same retry-per-snapshot shape. The file was modified by another worker during this run, so I did not edit it. Settings binary (`flamewm-settings/src/app_impl_1.rs:397`) also unchanged.
- Flakiness: two early runs (noicons, lang) lost wm/shell/desktop at 20 s with no panic or exit message. Later reruns of the same scenarios were all alive. Cause unknown; a `pgrep -f flamewm$` showed 2 processes outside this harness, so another worker may kill by name. Treat these scenarios as single-sample.
- Risk: per-process banners are now logged once, so a provider change mid-run is not re-reported. Harness: `/tmp/claude-1000/fw-c` is a copy of `fw-o` runtime scripts only; the repo itself was built in place.

- Results: WF (CODER, wm-x11 focus/placement; scope `crates/flamewm-wm-x11` only, not committed).
  - WF1 WM_TAKE_FOCUS: `src/wm/take_focus.rs` (`focus_delivery`, `parse_input_hint`, `read_input_hint`). `focus_with_stacking` (`window_ops.rs`) skips `SetInputFocus` for no-input/globally-active clients that list WM_TAKE_FOCUS unless WM_HINTS input is true; sends WM_TAKE_FOCUS for any listed protocol. Input-false clients without the protocol keep the old behaviour.
  - WF2 placement: `src/wm/transient_placement.rs` (`centre_over_parent`). `manage.rs` centres a transient `WindowKind::Normal` over its parent frame, clamped to that output's work area. Pointer/cascade policy unchanged for others. "Dialog" is not a distinct WindowKind; any transient normal window is treated as a dialog.
  - WF3 focus stealing: `src/wm/focus_guard.rs`. Button presses (`pointer.rs`) and WM key presses (`events.rs`) note user activity; 2 s window. Map-time focus is skipped after recent input unless no active window or transient. `_NET_ACTIVE_WINDOW` with source != 2 (pager) is ignored after recent input. DEMANDS_ATTENTION: NOT done (needs state writes in 4+ files).
  - Checks: `cargo test -p flamewm-wm-x11 --no-run` exit 0, 0 warnings. `cargo test -p flamewm-wm-x11` exit 0, 542 passed (534 + 8 new), 0 warning lines. Log: `/tmp/claude-1000/-run-media-linsaftw-SDD-GitHub-ArkFlame-flamewm-webrender/84cf5e3a-57bb-4dfd-9c4b-adc66fab352b/scratchpad/wf-test.log`.
  - Unverified: X wiring (WM_HINTS read, real map/focus/placement under Xvfb) has no runtime probe. Typed keys inside client windows are not seen by the WM, so the recent-input window only covers clicks and WM-bound keys.
  - WF addendum (Xvfb :88 probe, `/tmp/claude-1000/fw-wf/run.sh`, binary rebuilt, `probe.out` ALL_PASS, 0 panics): (a) dialog frame centre delta (0,0) over parent; (b) WM_TAKE_FOCUS received by globally-active and no-input clients, globally-active focus on pager activation; (c) first map focuses, background map after key press keeps focus, app activation after input blocked, pager activation focuses, map after 2.5 s quiet focuses. Negative control without key press: background map steals focus (`probe_ctrl.py`). Typing now counts: `raw_keys.rs` `handle_raw_event` notes key presses from the XI2 raw feed, which sees keys regardless of client focus.
  - WF addendum checks: `cargo test -p flamewm-wm-x11` exit 0, 542 passed, 0 warnings (after raw-key change).
  - Harness note: the first managed window takes about 5.5 s after WM start (startup cost, not measured further); probe waits up to 15 s per window.

- Results: NF notifications (CODER, supersedes the BLOCKED line above; not committed).
  - Checks (actual exits): `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-swarm-q cargo test -p flamewm-integrations-linux -p flamewm-shell` exit 0; 856 passed, 0 failed, 0 warning lines. Log: `.agents/runtime/nf-test3.log`.
  - Live (display :87, fresh binaries, `/tmp/claude-1000/fw-nf/`): wm.log, shell4.log, desktop.log, explorer.log, mon2.log each 0 warn/error/panic lines.
  - Actions: toast action click confirmed `ActionInvoked` then `NotificationClosed` (reason 2) in `mon2.log`. Screenshots `toast-actions2.png`, `center2.png`.
  - Default body click: `ActionInvoked "default"` then `NotificationClosed` (reason 2).
  - Close reasons: expire = 1, CloseNotification = 3 (monitored).
  - Critical: no expiry, still visible at 9 s (`toast-critical.png`).
  - Transient: absent from center history.
  - Image-data: icon rendered top-right (`toast-image2.png`).
  - Long body: clipped to max-height 36px, does not overlap actions (`toast-long2.png`). It does not wrap.
  - Markup: tags stripped in summary and body.
  - Layout: 14px parent-padding inset fixed in CSS (left/right/top 0); verified by screenshot.
  - Not done: center-row action buttons (item 3). Absolute `image-path` files are not decoded (name-based icon lookup only). Long body does not wrap.
  - Processes: all harness PIDs (Xvfb :87, dbus, flamewm, shell, desktop, dbus-monitor) killed by verified cmdline; none remaining.
