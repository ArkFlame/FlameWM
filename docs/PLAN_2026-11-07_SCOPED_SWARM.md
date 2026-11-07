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

- Results: NC (CODER, notifications; not committed; live verification NOT run).
  - Checks (actual exits): `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-swarm-nc CARGO_BUILD_JOBS=3 cargo test -p flamewm-shell -p flamewm-integrations-linux -p flamewm-ui-core -p flamewm-skin` exit 0; 571 + 262 + 104 + 38 + 16 + others passed, 0 failed, 0 `warning:` lines. Log: `/tmp/claude-1000/nc-test1.log`.
  - Done: (1) center rows show up to 2 client action buttons (`nc-item-G-I-action-K`, `#ffffff1f` like `.nc-clear`, no new colours, no shell_tones change); click invokes `provider.invoke_action` (ActionInvoked, then close) and removes the row; buttons hidden once the toast is gone. (2) absolute PNG `image-path` (plain path or `file://`), regular file <=1 MiB, IHDR edge <=512, decoded via `flamewm_image_core::png`, downscaled once by `downscale_to_icon`; falls back to named icon. (3) toast body `white-space:normal`, `max-height:32px` (2 lines at 12px/1.3).
  - Changed: `crates/flamewm-integrations-linux/src/notifications.rs`, `crates/flamewm-shell/src/notification_center.rs`, `crates/flamewm-shell/src/main.rs` (out of listed scope: the host owns the provider call), `ui/shell/notification-center.html`, `ui/shell/flamewm.css`.
  - Not verified: live Xvfb run, screenshots, dbus-monitor, body wrap rendering, center button click. No `cargo clippy` run.
  - Risks: center action visibility updates only when history revision changes, so a toast expiring while the center is open keeps its buttons until the next change; a click then returns "no longer active" and does nothing.

### Results: LK (leak soak, partial, not accepted)
- Build: cold `cargo build -p flamewm-wm -p flamewm-shell -p flamewm-desktop --bins` with CARGO_TARGET_DIR=target-swarm-lk, CARGO_BUILD_JOBS=3, exit 0, 2m43s.
- Run: Xvfb :90, flamewm and flamewm-desktop up. flamewm-shell exited (GetShellBootstrap: com.arkflame.FlameWM1 not provided, needs session service). Soak reached cycle 30 of 60, then timed out. Quick-control popup not exercised (no trigger identified).
- Samples (RSS kB / PSS kB / fd): wm start 29592/20187/25, mid(30) 29592/20093/25; desk start 36672/26886/18, mid 36480/26574/18. No end sample.
- Slope: not established (one mid point). No fd growth observed. No fix made.
- Rerun (supersedes partial above): Xvfb :91, wm+shell+desktop started with staged delays (3 s/2 s) as in fw-nf start.sh. 40 cycles (xterm open/close 0.25 s + dbus Notify with 5 s cap), elapsed 10.2 s, notify_errors 0. Popup: no trigger/flamewm-ctl present, not exercised.
  - wm: RSS 29896 -> 29896 kB, PSS 20064 -> 20062, fd 27 -> 27 (slope ~0).
  - shell: RSS 31500 -> 31604 kB (one-time +1.1 MB at start, flat start->mid->end at 31604), PSS 21730 -> 21834, fd 18 -> 18.
  - desk: RSS 36232 -> 36232 kB, PSS 26157 -> 26153, fd 18 -> 19 (+1, single sample; not a trend).
  - Under 2 MB threshold; no real growth; no fix made. Single run, not statistically established.

### Results: ST (CODER, startup first-window latency)
- Culprit: the titlebar backdrop canvas (`crates/flamewm-wm-x11/src/frame/titlebar_backdrop.rs`). The first managed frame's chrome paint built the wallpaper canvas (decode, compose, blur) synchronously on the WM event loop. The cache under `$XDG_CACHE_HOME` is empty on a cold start, so the first window waited on that build. The WM's own catalog scan (`wm.startup.catalog`, about 22-33 ms) and the audio helper (`PulseAudio not ready after 2s`, helper process) are not on the critical path.
- Fix: canvas build moved to a `flamewm-titlebar-canvas` worker. The first paint is flat until the canvas is stored, then the worker wakes the reactor (`runtime.rs` wake pair), and `wm/paint.rs::refresh_titlebar_backdrop` repaints chrome once per ready canvas (called after dispatch in `wm/event_loop.rs`). Stale keys are dropped; a failed worker spawn falls back to flat.
- Probe: `scratchpad/st/run.sh <tag>` (Xvfb `:87`, `timeout 15` WM with `FLAMEWM_PROFILE=1`, Xlib client mapping at t=0, debug binary `target-swarm-st/debug/flamewm`).
  - BEFORE (base3/base4): client managed and viewable at 3.08 s. WM log `wm.resize.motion.drain max_wall=2938-3061 ms`, `wm.chrome.backdrop.canvas` on the loop.
  - AFTER (after1/after2): managed and viewable at 0.17 s. `wm.loop max_wall=15.33 ms`, `wm.resize.motion.drain max_wall=25.92 ms`, canvas CPU (`3638 ms`) on the worker.
  - Repaint check (after4): titlebar mid-row at map time `distinct=1` (flat `1c1c1c`), 4 s later `distinct=5` (translucent, `332838ff`).
- Tests: `cargo test -p flamewm-wm-x11 -p flamewm-wm` exit 0 (16 + 20 + 544 passed, 0 failed), 0 `warning` lines. New tests: `cold_image_titlebar_paints_flat_and_leaves_the_canvas_to_a_worker`, `worker_result_is_stored_once_and_a_stale_key_is_dropped`.
- Not run: the `/tmp/claude-1000/fw-wf/run.sh` harness itself (display `:88` is held by another WM, pid 2709987; not killed). Probe used `:87` with a single client, not the 10-window `probe.py` suite. Release build not measured.
- Risk: the first window's titlebar is flat for the cold canvas build (about 3.6 s CPU in debug, on a worker), then repaints. Wallpaper change rebuilds the canvas off-loop, so the old flat/canvas state shows for that period.
  - NC live addendum (Xvfb :89, harness `/tmp/claude-1000/fw-nf2`, fresh binaries from `target-swarm-nc`): VERIFIED.
    - Toast `nc-toast.png`: image-path PNG icon top-right; long body wraps to 2 lines; Reply/Later row below, no overlap.
    - Center `nc-center3.png`: action buttons labelled; empty slots hidden; inactive (pre-restart) row has no buttons.
    - Click Reply on live row: `dbus-monitor` shows `ActionInvoked uint32 3 "reply"` then `NotificationClosed 3 reason 2`; row removed (`nc-verify-pair.png`).
    - Stale-button fix: timed notification with actions in open center; after expiry buttons are gone (`nc-stale-fixed-crop.png`).
    - Defect found and fixed live: empty action button made `set_text` fail ("node has no text child"), aborting the whole center projection. Fixed by a zero-width text child plus a regression test. Both the log and screenshots confirmed it.
    - Engine risk (outside scope, not fixed): a leaf `visible(false)` under a visible row did not repaint. Worked around by toggling a per-row `nc-item-G-I-actions` wrapper.
    - Processes: all own PIDs killed (verified by cmdline and `/proc` check). Logs: `/tmp/claude-1000/fw-nf2/`.

### Results: CB (CODER, X11 clipboard persistence; not committed)
- Task A, DONE: no clipboard manager, `SAVE_TARGETS`, or `CLIPBOARD_MANAGER` handling existed (grep: none). Added `crates/flamewm-clipboard/src/keeper.rs`: a thread on its own connection watches CLIPBOARD via XFixes (`SET_SELECTION_OWNER`, `SELECTION_WINDOW_DESTROY`, `SELECTION_CLIENT_CLOSE`). It reads text (`UTF8_STRING`, then `STRING`) from each new owner, keeps it when <= 1 MiB, and re-serves it through `Clipboard::set_text` when the owner window or client goes away. An explicit clear (`SetSelectionOwner` None) drops it. Started from `flamewm-wm-x11/src/wm/event_loop.rs` after `become_wm`, via `flamewm_clipboard::spawn_keeper`. Exception entry added in `docs/ARCHITECTURE_EXCEPTIONS.toml` (`keeper.rs`, x11rb-symbol).
- Probe: `scratchpad/cb_probe.py` on Xvfb `:92`, `flamewm` built from `target-swarm-cb`, run under `dbus-run-session`. Owner closed without clearing, then reader: `'keeper-probe é hello'` (exit 0). Owner cleared, then reader: `NONE` (exit 0). WM log: 0 keeper or panic lines.
- Checks: `cargo test -p flamewm-clipboard -p flamewm-wm-x11` exit 0 (17 + 2 + 544 passed, 0 warning lines). Keeper unit tests: 3 (step mapping, 1 MiB cap boundary). `cargo build -p flamewm-wm` exit 0, 0 warnings.
- Limits: the 1 MiB cap applies after receipt. The transfer is bounded only by the 500 ms `get_text` timeout, so a large owner can be transferred in full before it is dropped. The keeper caches all CLIPBOARD text in WM memory until replaced (password-manager copies included). Text already on the clipboard before WM start is not kept until the next copy. WM restart drops the kept text.
- Task B, SKIPPED: `wm/ping.rs` already exposes `PingState::is_unresponsive`, but it is set only after a close-triggered ping misses its pong. Showing it in the pager/task list needs a snapshot field, control-wire encode/decode, and the shell projection, which is over the 3-file cap. No UI change made.

Results: CB2
- Change: `keeper.rs` skips keeping text when CLIPBOARD TARGETS contains `x-kde-passwordManagerHint` (presence, not value) or TARGETS is unanswered. Decision: pure `keepable()`, unit test `password_hint_or_unanswered_targets_are_never_kept`.
- Xvfb `:92`, `flamewm` built from `target-swarm-cb` (`cargo build --bin flamewm`, exit 0, 0 warnings). Probes run with the default Python path, not `-I`, since `Xlib` is installed in user site.
- (1) Regression, normal owner then close: reader `'keeper-probe é hello'`, so text kept and re-served.
- (2) Owner advertising `x-kde-passwordManagerHint` in TARGETS, then close: reader `NONE`, so not kept. The NONE result depends on the TARGETS decode, which is confirmed by this run.
- Unit tests: `cargo test -p flamewm-clipboard -p flamewm-wm-x11` exit 0, 0 warning lines.
- Not done: session-lock drop (no cheap hook). Value check for `secret` not implemented.
- Probe PIDs (Xvfb, flamewm) killed. No commit.

Results: SP
- Task: snap/move preview invisible. Status: UNVERIFIED (no code change; root cause for the reported default-config case not reproduced).
- Live repro (Xvfb :150, flamewm + flamewm-desktop, xterm, python-xlib xtest, screenshots under the scratchpad `base/shots`):
  - Default outline mode, titlebar drag to top-left corner, top-centre and screen centre, and Meta+drag to corner: ring window mapped, accent, 2px, rounded, at the zone (797x425 / 1596x852) or frame size for a free move. Visible in screenshots.
  - showContentWhileDragging=true, titlebar drag to top edge: ring is visible on arrival, then unmapped after the 220 ms edge dwell. Debug trace: the dwell fires a workspace switch (`move_native` to the bottom row, pointer warped to the bottom edge), and `workspace.rs` hides the preview.
  - Outline mode, same edge dwell: switch is refused by `drag_edge.rs::defer_drag_arrival_paint` (commit carries a non-move effect), so no hide. Edge workspace switching is effectively dead in outline mode (separate defect).
- Not changed: `snap_preview.rs`, `pointer.rs` (temporary instrumentation reverted, `git diff` empty for flamewm-wm-x11), `workspace.rs`.
- Build: `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-swarm-sp CARGO_BUILD_JOBS=3 cargo build -p flamewm-wm -p flamewm-wm-x11` exit 0.
- Open: which exact visual the user lost (fill vs border, or a hidden-on-dwell case) is not confirmed. Wayland `crates/flamewm-wm-wayland/src/snap_preview.rs` not tested.
- No commit.

Results: SP2
- Status: UNVERIFIED for the user-reported Wayland case. No product code changed (`git diff` of snap_preview.rs, input/*.rs empty; render/mod.rs diff is another worker's 25 lines). Debug hooks were reverted.
- Harness: debug build `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-swarm-sp2`, nested winit `WINIT_UNIX_BACKEND=x11` on Xvfb `:161`, `FLAMEWM_WAYLAND_SOCKET=sp2-flame`, probe client `examples/pointer_probe`. Host GL: Mesa Intel (hardware), same as the user log.
- Snap ring on Wayland: titlebar drag to left edge, held: ring drawn at LeftHalf (accent 2px, 8px radius). Also with forced buffer age 1 (temporary env hack, reverted). After release the window snaps and no stale ring remains.
- Meta+drag: works. Earlier "did not move" was a harness artifact: the nested window needed keyboard focus (`XSetInputFocus`) before XTest Super key events reached the compositor. With focus, `state.logo` is true and Meta+drag moves the window and shows the ring. No code bug found in `input/buttons.rs:129` or the modifier path.
- Not reproduced: no partial-repaint (age>0) defect in show/move/hide.
- Open: the user's "no preview at all" on release Intel hardware is not reproduced. Untested: winit under a Wayland host (user log shows `FLAMEWM_GL_RENDERER backend=winit`, host session is Wayland).
- Side observation: the ring was red in one run and blue in others; sampled pixels 0xef4048 vs 0x3daee9. Likely the shared accent setting changed between runs (other worker's tests write settings). Not investigated.
- X11 items (dwell hide, outline refusal): not started.
- Checks: `cargo test -p flamewm-wm-wayland -p flamewm-wm-x11` EXIT 0 (12+253+544 passed, 0 warnings); `python3 -I tools/architecture_guard.py` EXIT 0 (77 ratified violations, no new).
- Screenshots: `/tmp/claude-1000/sp2/dbg_edge_hold.png`, `/tmp/claude-1000/sp2/age1_edge_hold.png`, `/tmp/claude-1000/sp2/meta_focus_hold2.png`, `/tmp/claude-1000/sp2/after_release_meta_age1.png`.
- No commit.

Results: SP2 (update 2)
- Root cause (Wayland, verified nested): a free move never moved the window or drew any outline. The Move grab only drew the snap ring on snap targets. The window committed only on release, in both modes.
- Fix: `showContentWhileDragging` is read in `control/settings.rs` (same key and default as wm-x11 `live_settings.rs`) and mirrored into `State`. `input/move_feedback.rs` is a pure function: content mode sets the live client rect, and rings only snap zones; outline mode keeps the window and rings the snap zone or else the free frame. `input/pointer.rs` Move branch applies it and reuses `SnapPreview`. Motion calls `request_paint()` when the ring or rect changes, so the DRM cached scene is rebuilt (`needs_scene_rebuild`, unit-tested).
- Verified nested (Xvfb :162, debug build, Intel GL): outline mode titlebar drag shows accent ring at would-be frame, window stays; release commits to new place, ring gone. Content mode (temporary forced default, reverted) moves the window live; left-edge drag shows the snap ring with the window following.
- Not verified: content mode through the real settings file. Writing `showContentWhileDragging=true` (with `windowsCustom=true`) to a `FLAMEWM_SETTINGS_PATH` file did not change behaviour in the nested run. Cause unknown; the sync runs in `control/service.rs`.
- Not verified: Meta+drag in outline mode (the key path is the same `Move` grab).
- Known gap: Wayland ignores `snapPreviewEnabled`, which X11 honours. Not changed.
- Checks: `cargo test -p flamewm-wm-wayland` exit 0 (12, 258, 0 passed, 0 warnings). `cargo check -p flamewm-wm-wayland --features drm` exit 0, 0 warnings. `python3 -I tools/architecture_guard.py` exit 0 (77 ratified). `flamewm-platform` not touched; `flamewm-wm-x11` not touched.
- Screenshots: `/tmp/claude-1000/sp2/outline_hold.png`, `/tmp/claude-1000/sp2/outline_after_release.png`, `/tmp/claude-1000/sp2/content_hold.png`, `/tmp/claude-1000/sp2/content_edge_hold.png`, `/tmp/claude-1000/sp2/live162.png`, `/tmp/claude-1000/sp2/live162_after.png`.
- No commit. Debug hooks reverted; `rg SP2DBG|SP2TEMP` is empty.

### Results: NR (CODER, X11 not-responding task mark)

- Scope: `_NET_WM_PING` liveness shown on the shell task list. `WindowSnapshot.unresponsive: bool` (default false) is set from wm-x11 ping state, encoded as `unresponsive` in `flamewm-control-wire` (missing key decodes false), and drives the task status line tint (`--c-ef4048`, existing token; no layout change).
- Root cause of "stale" marks: the ping expiry only changed WM state; it did not mark windows dirty, so the snapshot and shell were not refreshed until another event. Fixed: `Wm::expire_pings` runs before `take_changes`, and a pong marks windows dirty.
- Live check (Xvfb :181, isolated session, fresh binaries from `target-swarm-nr`; harness `/tmp/claude-1000/nr-live/`):
  - HungApp (never answers `_NET_WM_PING`): close request, WM sent ping and WM_DELETE; after the 5 s timeout its task underline turned red. Screenshot `/tmp/claude-1000/nr-live/after_timeout.png`, crop `taskbar_after.png`.
  - HungTwo (control, timed): 1.6 s after close request, slot 3 red pixels 0 (`hung2_t1.png`); after timeout, red (`hung2_late.png`, crop `hung2_late_tb.png`).
  - GoodApp (answers ping, ignores WM_DELETE): pong received; after timeout its slot shows no tint (red 0; accent/idle colour only).
  - No panics in `wm.log`, `shell.log`, `desktop.log`.
- Checks: `cargo test -p flamewm-api -p flamewm-control-wire -p flamewm-control-dbus -p flamewm-shell -p flamewm-shell-core -p flamewm-wm-x11 -p flamewm-platform` EXIT 0 (1670 passed, 0 failed, 0 warnings). `cargo build -p flamewm-wm -p flamewm-desktop -p flamewm-shell` EXIT 0. `cargo check -p flamewm-wm-wayland` EXIT 0. `python3 -I tools/architecture_guard.py` EXIT 0 (77 ratified, no new).
- Not verified: Wayland (out of scope); the tint at the exact timeout instant (screenshots at 1.6 s and after the timeout only); D-Bus/control consumers of `unresponsive`; the chosen red is distinct from the focus accent only in this theme.
- Processes: all own PIDs (Xvfb :181, dbus session, flamewm, flamewm-shell, flamewm-desktop, python clients) killed; `/proc` check shows none with `DISPLAY=:181`.
- No commit.

Results: S3 (CODER, Wayland drag feedback and X11 edge dwell; not committed)
- Task 1, `snapPreviewEnabled` on Wayland: done. `control/settings.rs::snap_preview_enabled` (default on, same as X11 `live_settings`), mirrored in `control/mirror.rs`; `input/pointer.rs` Move drops the zone ring when off. Release snapping is unchanged.
- Task 2, root cause: `state.rs` initialised `show_content_while_dragging: false` and only refreshed it in `Service::turn` (bus-driven, `control/mirror.rs` `sync_chrome_settings`). `titlebars`, `windowMargin` and `focusFollowsMouse` are read from the file at startup, but this key was not, so a file value stayed unapplied until a bus turn. `windowsCustom` gating is correct (`effective()` resolves members). Key names are exact-match; a mis-cased key is kept as an unknown key and ignored. Fix: `input/settings.rs::saved_drag_feedback` read at startup for both keys.
- Task 3 (X11), fix: `wm/pointer.rs` snap-zone branch sets `edge_drag = None` instead of arming the dwell while a zone is held. The zone owns the edge, so no workspace switch hides the preview.
- Task 4 (Meta+drag, outline, nested Wayland): verified. The window stays and the ring follows the pointer at the would-be frame; titlebar to left edge shows the zone ring. Focus is needed first (`XSetInputFocus`).
- Checks (actual exits): `cargo test -p flamewm-wm-wayland -p flamewm-wm-x11` exit 0 (12 + 262 + 545 passed, 0 warnings); `cargo check -p flamewm-wm-wayland --features drm` exit 0, 0 warnings; `python3 -I tools/architecture_guard.py` exit 0 (77 ratified).
- Live evidence, `/tmp/claude-1000/s3/`: `content_mid_c.png` (file-only content drag moves the window live), `nopreview_edge_c.png` (snapPreviewEnabled=false, no ring), `outline_meta_c.png`, `outline_edge_c.png`, `x11_before_late.png` (pre-fix: xterm jumps to bottom row, ring gone), `x11_edge_late.png` (fixed: ring kept, no switch).
- Side change outside task scope: another worker added `WindowSnapshot.unresponsive`; `control/mirror.rs` and `control/tests.rs` set `unresponsive: false` (3 sites) so the Wayland crate compiles.
- Not done: no unit test for the X11 edge-dwell change (`Wm` needs an X connection); covered by the live Xvfb run only. Pre-fix nested Wayland baseline for task 2 not re-run.

### Results: QC (CODER, quick-control popup soak and cold-open timing; no source change; not committed)
- Build: `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-swarm-qc CARGO_BUILD_JOBS=3 cargo build -p flamewm-wm -p flamewm-shell -p flamewm-desktop --bins` exit 0, 9m12s, 0 warnings (dev profile, host load ~40).
- Harness: Xvfb :94 (own PID), `dbus-run-session`, flamewm, flamewm-shell, flamewm-desktop; state `/tmp/claude-1000/fw-qc/`. Trigger: xtest click on panel buttons (1600x44 panel at y=856) from a fresh screenshot. No dbus open method exists in `flamewm-control-wire`. Script: `/tmp/claude-1000/fw-qc/soak.py 50`; samples `/tmp/claude-1000/fw-qc/soak.jsonl`; summary `/tmp/claude-1000/fw-qc/soak_summary.json`.
- Soak: 50 open/close cycles each for weather, network, battery, calendar. Open confirmed by window title in all 200 cycles. Close = outside click on the desktop.
- Not exercised: audio. No PulseAudio in the harness (`pulseaudio` not installed), so the tray volume item is absent. Panel button at x=1404 opens "FlameWM Assistant", not audio.
- Samples (shell main pid, start -> end): RSS 27460 -> 26892 kB, PSS 13114 -> 12534 kB, fd 20 -> 20, threads 4 -> 4 (one transient 5 mid-run).
- Samples (quick-control host child, PPid = shell, start -> end): RSS 27760 -> 28740 kB, PSS 12981 -> 13953 kB, fd 12 -> 12, threads 2 -> 2. Mid-kind samples fluctuate by about +/-0.9 MB.
- Slope: about +4.9 kB per open/close cycle on the host (about +1 MB over 200 cycles, noisy, single run). Below the 2 MB threshold. No fd or thread growth. No fix made.
- Cold-open timing: `slow open` lines (threshold 50 ms, hard-coded in `quick_controls/host/timings.rs`, not env-gated). 191 lines logged, including one smoke-run cycle per kind. Opens under 50 ms are not logged.
  - Median open.total (p90, max): weather 300 ms (721, 1154); network 1100 ms (2057, 3236); battery 141 ms (307, 373); calendar 781 ms (1598, 2327).
  - Median stages (ms): `present` 272 / 1063 / 122 / 704; `settings_refresh` 24 / 24 / 31 / 26; `measure` under 1; `project` under 1; `ensure` about 1 µs when cached (9 to 248 ms on first use).
- Culprit for the >30 ms open: `shell.quick.present` (`quick_controls/popup.rs:98` -> `flamewm-ui-x11` `surface_runtime.rs:195` -> `flamewm-render-x11` `surface_controller.rs:440` `show()`). It repaints the popup on every open. Outside this task's scope, so not changed. Not profiled, so the cause is inferred from stage timings.
- Cold-open fix: not applied. `settings_refresh` (about 24 ms) is a fresh `ControlClient::connect` plus `GetSettings` per open, deliberately live (`host/scoped.rs:83`). Caching the connection is a lifecycle change with no measured gain, so I did not make it.
- Checks: `cargo test -p flamewm-shell` (same target dir, CARGO_BUILD_JOBS=3, RUST_TEST_THREADS=2) exit 0; 574 passed in the lib, 0 failed; 0 warnings. `python3 -I tools/architecture_guard.py` exit 0 (77 ratified violations, 3 WARN TODO lines). `python3 -I -m unittest discover -s tools -p test_architecture_guard.py` exit 0 (6 tests).
- Changed paths: none. `git diff -- crates/flamewm-shell/src/quick_controls` is empty.
- Risks and gaps: debug build under load ~40, so absolute latencies are inflated. No release-profile measurement. Audio popup not soaked. Single soak run; slope not statistically established. Harness processes stopped; Xvfb :94 gone.

### Results: IDLE (CODER, scoped idle audit; no code changed)
Probe: `scratchpad/idle_probe.py` (own Xvfb, own dbus, binaries `target-swarm-id/debug`, debug profile). Per-thread `/proc` utime+stime and voluntary/nonvoluntary ctxt switches over the window.
- STEADY (settle 30 s, window 15 s): total 0.0% CPU, 2.9 wakes/s. Shell and desktop at 0. Only WM main thread: 2.7 voluntary wakes/s (WM is outside this worker's scope). Evidence: `scratchpad/steady_before/idle_samples.json`.
- SPEC PROBE (settle 10 s, window 15 s): total 31.5% CPU, 195.7 wakes/s. Top rows: desktop `flamewm-wallpaper-loader` 25.0% CPU, 142.9 wakes/s (vol 0.07/s); shell main 4.8% CPU, 34.1/s (vol 3.3/s); desktop main 1.3% CPU, 13.4/s; WM main 0.3% CPU, 3.1/s. Evidence: `scratchpad/timeline_a/idle_samples.json`.
- Wallpaper worker is a one-time startup cost, not an idle loop. strace shows one `openat` of `assets/wallpapers/fitz-roy-dawn.webp` (2560x1707 WebP) per desktop start. It was idle at the 20-25 s sample. The 5-50 s window totals 4.9 s CPU (`scratchpad/sample_run/idle_samples.json`). A live /proc sample at 20-25 s showed it in `futex_do_wait` with no CPU.
- Wake counts are mostly nonvoluntary preemptions. Host load average was 46-54 on 12 cores during the runs, so use the voluntary column as the true wake metric.
- `strace -p` attach is blocked (`/proc/sys/kernel/yama/ptrace_scope=1`, EPERM). Launch-mode `strace -f -c` covers startup as well as idle, so its counts are not idle-only. Evidence: `scratchpad/strace_run/strace_*.txt`.
- Culprit ranking: (1) desktop wallpaper decode at startup, one-time, debug-build cost; (2) shell startup-phase wakes, about 3 voluntary/s at 10-25 s, 0 by 30-45 s; source not isolated.
- FIXED: nothing. No steady-state idle culprit reproduced in the in-scope crates, so no root fix was made. A fix without before/after movement would be unproven.
- Checks: `cargo build -p flamewm-wm -p flamewm-shell -p flamewm-desktop --bins` exit 0, 0 warnings (`CARGO_TARGET_DIR=target-swarm-id`, jobs 3). `python3 -I tools/architecture_guard.py` exit 0. No crate edited, so no crate tests were run.
- Risk: debug profile inflates decode CPU. Release probe under low host load is the next step. Other agents' uncommitted edits are present in the tree and were not touched.
