# BREAK reliability scenarios backlog

> Rows: `break-b1` (done `d7d1c91e`), `break-s6` (done `501b59cd`), `break-b2`..`break-b6` in `docs/BACKLOG.md`.

Harness: `scripts/break/break.sh [all|s1..s7]` (private Xvfb, private HOME/XDG, copies of fast-profile
binaries, XTest pointer via `scripts/break/pointer.py`). Evidence dir per run:
`.agents/runtime/break/<stamp>/<scenario>/` (ignored by git).
Base: `origin/main` `7aa3156e`. Final suite run: `.agents/runtime/break/20261109-222029/` (binaries built with the fixes below).

## Results

| Scenario | Verdict | Evidence |
| --- | --- | --- |
| s1 kill -9 shell, then desktop | PASS | shell and desktop respawned (new pids); WM ready; 10 windows before and after; panics=0 |
| s2 150 xterm/xeyes cycles + 40 windows | PASS (borderline) | WM RSS 11304 KB -> 19344 after churn -> 19480 with 40 windows -> 19488 after close (+8184 KB, threshold 8192); CPU with 40 windows 0%; no stale focus. Growth is a one-time step, not per-cycle |
| s3 500 hover moves over 10 s | PASS | shell CPU 1%; shell RSS 21488 -> 21472 KB; label flicker UNVERIFIED |
| s4 xrandr 1280x800 -> 1024x768 -> back | FAIL (geometry, b6 assertion) | newmode/addmode/size exit 0; WM and shell/desktop alive; dock window correct (1024x44 at y=724) but its strut is `1024,0,0,0` (left) instead of `0,0,0,44` (bottom). Run `.agents/runtime/break/20261111-120945/s4/panel-after.txt` (before resize: 1280x44 at y=756, strut `0,0,0,44`, OK) |
| s5 read-only config dir | UNVERIFIED | no save fired at startup, so the save-error path was not exercised |
| s6 garbage / non-UTF-8 settings.toml | PASS (after fix 1) | before: WM exited (`IoFailure: stream did not contain valid UTF-8`), session failed; after: logged, defaults, session up |
| s6b truncated settings.toml | PASS (3 of 4 runs) | one FAIL in the final suite (B2), passes on reruns |
| s7 no session bus | PASS (after fix 3) | WM, shell, desktop up; panel visible in `shot-s7.png`; shell logs once and retries; late bus attaches (`late_attach=yes`) |

## Backlog rows

| # | Scenario | file:line | Symptom -> cause | Proposed fix | Size |
| --- | --- | --- | --- | --- | --- |
| B1 **fixed** | s7 | `crates/flamewm-shell/src/main.rs:594` | Shell exits on `ControlClient::connect(BusKind::Session)` failure (`connect control session: D-Bus connection failed`), so the session shows the shell-missing fallback and no dock/panel. Evidence: `session.out` "shell: Shell ... exited during startup: exit status: 1". | Retry the control connect with backoff instead of exiting, or add a degraded shell mode. Touches the shell entry, not `dock.rs`. | M |
| B1b **fixed** | s7 | `crates/flamewm-wm/src/bin/flamewm-session/readiness.rs:96` (`READY_TIMEOUT` 30 s) | Without a bus, desktop and shell start only after the 30 s control-bootstrap timeout. Fix 2 makes this a degrade instead of an abort. | Lower the timeout when the bus socket is absent (no socket file, immediate ECONNREFUSED). | S |
| B2 **fixed (harness)** | s6 / s6b (14 of 30 runs) | `crates/flamewm-wm/src/main.rs:165` and `crates/flamewm-xsettings/src/x11.rs:218` | WM logs `XSettings X11 request failed: failed to read whole buffer`, then `failed to read whole buffer`, and exits about 1 s after start. The X connection was closed during startup. Not reproduced on demand; the harness now waits for an X round-trip before starting the session. | Needs a soak: loop WM start against a fresh Xvfb N times and capture the first X error. Suspect an X server startup race or an early request. | M (investigation) |
| B3 | s2 | `crates/flamewm-wm-x11/src/runtime.rs` (not isolated) | WM RSS steps up 8.2 MB at the first 150 churn cycles and plateaus (40 windows +136 KB, after close +8 KB). Not a per-cycle leak at this size. | Soak 1000+ cycles to rule out slow growth; no code change yet. | S (measure) |
| B4 | s5 | `crates/flamewm-platform/src/services/settings_persist.rs:56` | Save-error path is unexercised. Startup does not write settings, so the read-only config dir produced no save failure. | Drive a settings change through the control path under the read-only dir and check for a logged error. | S |
| B5 | s3 | n/a | Label flicker (map count during hover) not measured; no map-count probe in the harness. | Add an xprop/xwininfo map-count sampler during the sweep. | S |
| B6 **fixed (break-b6)** | s4 | `crates/flamewm-render-x11/src/surface_controller/teardown.rs` `sync_dock_geometry` | After `xrandr -s 1024x768` the panel window was placed correctly (1024x44 at y=724) but `_NET_WM_STRUT_PARTIAL` reads `left=1024` instead of `bottom=44`. Root cause: the strut recompute took the root size from Xlib's cached `XDisplayWidth/Height` (1280x800, not refreshed by a RandR mode change), so the bottom bar was measured against the pre-resize root, and classic inference fell through to the left edge. | Read the root size live via `XGetGeometry` on the root window (`live_root_extent`). Regression test: `config.rs` `bottom_dock_after_xrandr_shrink_reserves_bottom_not_left`. Runtime: `scripts/break/break.sh s4` after fix (see commit). | S (fixed) |

## Fixed in this change

1. **Malformed or non-UTF-8 settings aborted the WM** (s6 FAIL before the fix). `crates/flamewm-platform/src/host.rs` `start()`: a parse error or `InvalidData` now logs `flamewm: settings <path> ignored, using defaults` and keeps defaults. Parse runs before any state change. Test: `crates/flamewm-platform/src/host/tests.rs` `start_falls_back_to_defaults_for_non_utf8_settings_file`, `start_falls_back_to_defaults_for_malformed_settings_value`.
2. **Missing control bus aborted the whole session** (s7). `crates/flamewm-wm/src/bin/flamewm-session/readiness.rs` `wait_ready`: once EWMH reports ready, the control-bootstrap timeout logs `continuing without session control` and returns Ok instead of failing the session. Test: `readiness_timeout_is_fatal_only_before_ewmh_is_ready`.

3. **No session bus: shell exited, no panel** (s7 FAIL before the fix; B1, B1b). Shell: `crates/flamewm-shell/src/main.rs` `load_bus_bootstrap` / `attach_control_bus` / retry timer (30 s, `BUS_RETRY_INTERVAL`); degraded state from `crates/flamewm-shell/src/bus_offline.rs` `offline_snapshot` (one synthetic output at root size, panels from `PanelService`); late attach applies `ShellRuntime::apply_bootstrap` and registers the signal source through `flamewm-reactor` `ReactorHandle`. Logged once at startup; retries silent. Start menu apps and bus-owned domains stay empty until attach. B1b: `readiness.rs` `session_bus_absent` skips the control wait when no `DBUS_SESSION_BUS_ADDRESS` and no `/run/user/UID/bus` exists, or when the address's unix socket path is missing. Guard exception added for that env read (`docs/ARCHITECTURE_EXCEPTIONS.toml`, `j11-g08-prefixed-env`, remove_by 0.2.0). Tests: `bus_offline::tests::*`, `readiness::tests::session_bus_*`.

Not fixed: B2 (below), B3-B6 (measurement gaps).

### B2 root cause and fix (break-b2)

Root cause: the harness Xvfb ran without `-noreset`. The `xprop -root` readiness probe in `sandbox_new` was the last client to disconnect, so the server reset (closed and re-created its listener). A client that connected during the reset (the WM main connection and the XSettings `x11rb::connect(None)`) got EOF in setup, which is `failed to read whole buffer`. Not a product bug; the WM was already non-fatal for XSettings.

Evidence: raw X11 setup probe (`probe.py`, connect/setup/close then immediate connect/setup, 200 pairs): default Xvfb 51/200 WM-side failures (EOF or error); `-noreset` 0/200.
Fix: `scripts/break/lib.sh` `sandbox_new` adds `-noreset` (comment in place). No product code changed.
Soak: `scripts/break/break.sh s6` x30 idle with the fix: 0 of 30 `failed to read whole buffer`; 60/60 s6/s6b verdicts PASS (`/run/media/linsaftw/SDD/gate-targets/b2-run/idle/`). One other session (it8) lost its server during s6 (`XIO: fatal IO error 2 ... X server ":240"`, Xvfb exited with no stderr; the socket vanished, likely an external teardown or kill, not reset): WM `Connection refused`, s6 session failed. Not the reported symptom; not reproduced in the other 29 runs.
Baseline under `stress-ng --cpu 14 --io 2` (no fix): `failed to read whole buffer` / `WM exited before readiness` in the first 3 iterations (s6b FAIL in it1).

## break-b4 coverage (settings save-error path)

Driven through the real `SettingsWriter::spawn` save path in `crates/flamewm-platform/src/services/settings_persist.rs` tests: a config path under a regular file (root-proof, unlike 0o500) returns `IoFailure` from `write_now`, from a debounced `flush`, and leaves no temp file when the rename target is a directory. Defect found: a pending save that fails while the writer shuts down was dropped silently; it now logs `flamewm: settings write on shutdown failed` to stderr. Gate: see commit message.
