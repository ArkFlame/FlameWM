# Unit u08-startup-robust

Status: **UNBUILT** — static audit and script edits only. No cargo, build, test or runtime session was run in this unit; verification and commit belong to the orchestrator.

Area: Session startup robustness on the laptop case where neither X11 nor Wayland comes up.
Owned paths: `scripts/flamewm-wayland-session`, `scripts/flamewm-session-wrapper`, this file.
Task: find hard fails in the two session entries, add an automatic fallback chain with one failure record per stage and a crash-loop guard, and keep the existing capability contract.

## Audit findings (current source)

| Hard fail | Where | Behaviour before this change | Consequence |
| --- | --- | --- | --- |
| Missing or bad `XDG_RUNTIME_DIR` | `flamewm-wayland-session`: `stage=runtime` | Recovers `/run/user/$uid` when present; otherwise logs an ACTION line and continues with `code=0` | No Wayland socket and no XWayland lock file; the compositor later fails at `stage=socket` / XWayland, so the user sees a black VT |
| No seat or logind | `XDG_SEAT` is only logged; `--probe` failures are advisory | No gate; a compositor that cannot take the seat exits fast and the wrapper exits non-zero | Session returns to the greeter with only a durable failure file to explain why |
| DRM open failure, no GPU or no EGL | Capability gate checks only `drm_compiled=true`; `--probe` is advisory | Device and renderer failures surface as a fast non-zero compositor exit or a presentation timeout (exit `124`) | Black panel; no automatic attempt with a different backend |
| Missing binaries | Compositor: exit `127` missing, `126` not executable, `3` XWayland absent; the services bridge is silently skipped | Hard exit with that code | No fallback to the X11 session or to a usable shell |
| No D-Bus session bus | Private `dbus-run-session` is started only when neither an address nor `$XDG_RUNTIME_DIR/bus` exists; if that binary is absent, `dbus_session_bus=absent` is logged | Continues without activation environment sync | Clients can inherit greeter `DISPLAY`; portals and wallet activation degrade |
| Stale socket | Socket wait tests only `-e "$XDG_RUNTIME_DIR/wayland-flame"` | A regular file left by a crashed run is read as a live socket | Compositor cannot bind, wait burns its budget, session fails |
| Unwritable log directory | Log setup falls back to inherited streams and leaves `LOG_FILE` empty | Presentation gate and compositor diagnostics are skipped (`stage=presented skip 0 no_log`) | Failure evidence disappears from the durable report |

Residual risk: an orphaned *live-looking* socket at our private name is not probed for a peer; removing it could kill a still-running session, so the cleanup is deliberately limited to non-socket files.

## Implemented fallback chain

- Entry points: the Wayland session drives `drm -> nested winit -> X11 session -> minimal safe mode`; the X11 session drives `x11 -> minimal safe mode` (a display manager that selected X11 has no compositor to fall back from).
- Activation: only for a display-manager launch, detected by `XDG_SESSION_ID`, `DESKTOP_SESSION`, `XDG_SESSION_CLASS` or `XDG_SESSION_TYPE`, or forced with `FLAMEWM_SESSION_FALLBACK=1`. `FLAMEWM_SESSION_FALLBACK=0` keeps the historical single-attempt behaviour. Every stage re-enters a wrapper with `FLAMEWM_SESSION_ATTEMPT=<stage>`, so the capability gate, its `code=` values and its exit codes are untouched for diagnostics and the existing session matrix.
- Failure record, written to stderr and to `~/.cache/flamewm/session-chain.log` (mode 0600, append-only): `FLAMEWM-SESSION-CHAIN stage=<s> status=fail cause=<tokens> code=<n> blast_radius=works:<tokens>|degraded:<tokens> ms=<n> detail=next=<s>`. Causes combine the stage exit code with observed evidence (`runtime_dir`, `no_seat`, `no_gpu`, `dbus_absent`, `stale_socket`, `no_first_frame`, `socket_not_detected`, `preflight_fatal`). Blast radius names what still works and what is degraded for each stage; a non-zero exit is returned only after the last stage fails. A forwarded shutdown signal ends the chain as a requested logout, not a failure.
- Crash-loop guard: a shared `session-chain.state` file records the chain start time and attempt count for both entry points. More than `FLAMEWM_CHAIN_GUARD_MAX` (default 3) chains inside `FLAMEWM_CHAIN_GUARD_WINDOW_S` (default 60 s) emits a `stage=guard cause=crash_loop` record and goes straight to safe mode. Any successful stage removes the state file; safe mode entered through the guard keeps the count while it still fails, so a display manager cannot bounce between the two entries and reset the guard.
- Minimal safe mode: reuses an inherited display or an existing `/tmp/.X11-unix` socket, otherwise starts `Xvfb`; starts any installed light window manager and one terminal (or `xmessage`), and waits on it. With no display at all it opens a recovery shell on the VT and points to the chain log.
- Capability contract preserved: `--capabilities` still prints the stable `key=value` set, exits 0, touches no seat; the drm gate still requires `drm_compiled=true` and still reports `stage=capabilities status=fail code=2 detail=drm_feature_missing` / `capabilities_unsupported` inside the attempt. The chain changes only what happens *after* that attempt fails.
- Added hardening: before the first chain attempt, a non-socket file occupying our private Wayland socket name is removed so the `-e` socket wait cannot be fooled by a crashed run. The single-attempt diagnostic path is left untouched.

## Reference precedent

`.vendor/sddm-develop/src/daemon/Seat.cpp:58-70` falls back from a failed Wayland or rootful X11 display server to `x11-user` "since the alternative is a black screen", and `:80-94` bounds retries to three attempts with a delay before giving up. This unit follows the same choice: keep a usable stage even when degraded, and bound repeated starts. `.vendor/lightdm-main/src/session-child.c:568` shows the logind session id (`XDG_SESSION_ID`) as the normal source of session identity, which is why the chain activation uses display-manager environment markers rather than inferring a backend from a leaked `DISPLAY`.

## Verification

Not run, by task rule (no cargo, no git, no builds, no tests). Static `bash -n` parse of both edited scripts passed. Runtime claims (fallback actually reaching safe mode on the laptop, socket cleanup under a real compositor) remain **UNVERIFIED** until the orchestrator runs `scripts/tests/wayland-session-matrix.sh` and a display-manager session on the affected hardware.
