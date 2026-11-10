# BREAK reliability scenarios backlog

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
| s4 xrandr 1280x800 -> 1024x768 -> back | PASS | newmode/addmode/size exit 0; WM and shell/desktop alive; dock position UNVERIFIED (screenshots only) |
| s5 read-only config dir | UNVERIFIED | no save fired at startup, so the save-error path was not exercised |
| s6 garbage / non-UTF-8 settings.toml | PASS (after fix 1) | before: WM exited (`IoFailure: stream did not contain valid UTF-8`), session failed; after: logged, defaults, session up |
| s6b truncated settings.toml | PASS (3 of 4 runs) | one FAIL in the final suite (B2), passes on reruns |
| s7 no session bus | FAIL (after fix 2) | WM and desktop up; shell exits (B1); session reports shell fallback |

## Backlog rows

| # | Scenario | file:line | Symptom -> cause | Proposed fix | Size |
| --- | --- | --- | --- | --- | --- |
| B1 | s7 | `crates/flamewm-shell/src/main.rs:594` | Shell exits on `ControlClient::connect(BusKind::Session)` failure (`connect control session: D-Bus connection failed`), so the session shows the shell-missing fallback and no dock/panel. Evidence: `session.out` "shell: Shell ... exited during startup: exit status: 1". | Retry the control connect with backoff instead of exiting, or add a degraded shell mode. Touches the shell entry, not `dock.rs`. | M |
| B1b | s7 | `crates/flamewm-wm/src/bin/flamewm-session/readiness.rs:96` (`READY_TIMEOUT` 30 s) | Without a bus, desktop and shell start only after the 30 s control-bootstrap timeout. Fix 2 makes this a degrade instead of an abort. | Lower the timeout when the bus socket is absent (no socket file, immediate ECONNREFUSED). | S |
| B2 | s6b (1 of about 6 starts) | `crates/flamewm-wm/src/main.rs:165` and `crates/flamewm-xsettings/src/x11.rs:218` | WM logs `XSettings X11 request failed: failed to read whole buffer`, then `failed to read whole buffer`, and exits about 1 s after start. The X connection was closed during startup. Not reproduced on demand; the harness now waits for an X round-trip before starting the session. | Needs a soak: loop WM start against a fresh Xvfb N times and capture the first X error. Suspect an X server startup race or an early request. | M (investigation) |
| B3 | s2 | `crates/flamewm-wm-x11/src/runtime.rs` (not isolated) | WM RSS steps up 8.2 MB at the first 150 churn cycles and plateaus (40 windows +136 KB, after close +8 KB). Not a per-cycle leak at this size. | Soak 1000+ cycles to rule out slow growth; no code change yet. | S (measure) |
| B4 | s5 | `crates/flamewm-platform/src/services/settings_persist.rs:56` | Save-error path is unexercised. Startup does not write settings, so the read-only config dir produced no save failure. | Drive a settings change through the control path under the read-only dir and check for a logged error. | S |
| B5 | s3 | n/a | Label flicker (map count during hover) not measured; no map-count probe in the harness. | Add an xprop/xwininfo map-count sampler during the sweep. | S |
| B6 | s4 | n/a | Dock/panel reposition after resize only checked by screenshot; no geometry assertion. | Compare panel/dock window geometry from `xwininfo -root -tree` before and after `xrandr -s`. | S |

## Fixed in this change

1. **Malformed or non-UTF-8 settings aborted the WM** (s6 FAIL before the fix). `crates/flamewm-platform/src/host.rs` `start()`: a parse error or `InvalidData` now logs `flamewm: settings <path> ignored, using defaults` and keeps defaults. Parse runs before any state change. Test: `crates/flamewm-platform/src/host/tests.rs` `start_falls_back_to_defaults_for_non_utf8_settings_file`, `start_falls_back_to_defaults_for_malformed_settings_value`.
2. **Missing control bus aborted the whole session** (s7). `crates/flamewm-wm/src/bin/flamewm-session/readiness.rs` `wait_ready`: once EWMH reports ready, the control-bootstrap timeout logs `continuing without session control` and returns Ok instead of failing the session. Test: `readiness_timeout_is_fatal_only_before_ewmh_is_ready`.

Not fixed (log only): B1, B1b (shell and timeout, cross-crate), B2 (needs repro), B3-B6 (measurement gaps).
