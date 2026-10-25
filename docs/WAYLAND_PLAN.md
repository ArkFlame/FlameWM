# FlameWM Wayland Plan

Status: the compositor runs as a nested window (winit) and as a real tty compositor (crate feature
`drm`) that takes its seat from systemd-logind over D-Bus. The nested backend and the build, test and
wrapper chain are exercised on this host. The native backend compiles and is unit-tested; the
recorded hardware attempt (a LightDM login) did not come up - a text cursor, then back to the greeter
- and is still under investigation. No successful hardware start is on record, so the session ships
as preliminary and the X11 session stays the default.

Claims carry their evidence: **code** marks something readable in the sources with no runtime
evidence behind it; commands that actually produced a result on this host are listed under "Proven
in this session"; everything that needs real hardware is under "Unverified / required hardware
validation".

## What exists

`crates/flamewm-wm-wayland`, binary `flamewm-wayland`, smithay 0.7.

- `model.rs`: pure window model (stacking, workspaces, cascade, min/max/fullscreen, grabs), no smithay.
- `state.rs`: `State`, plus the GPU-free frame-scheduling rules (`plan_frame`, `arm_retry`,
  `OutputFrames`) so the tty backend's pacing is testable without DRM.
- `handlers.rs`: xdg-shell, server-side decorations, layer-shell surfaces, seat, data device.
- `input.rs`: keyboard and pointer; chords resolved through the shared shortcut tables.
- `render.rs`: flat-colour titlebars plus client surfaces through the glow renderer; `dump_offscreen`
  renders a readback into an offscreen texture (copying from the window surface lost the EGL context
  on the next swap).
- `xwayland.rs`: XWayland spawn, X11 WM integration, unmanaged-window mapping and struts.
- `control.rs`: the shell/desktop control service (`flamewm-desktop-services` peer).
- `logind.rs`: the logind D-Bus session (feature `drm` for the real bus, `Session` impl and
  `TakeDevice` plumbing; the state machine compiles and is tested in every build).
- `drm.rs`: the tty backend (feature `drm`).
- `probe.rs`: `--probe`.

Also: session wrapper `scripts/flamewm-wayland-session`, session entry
`packaging/flamewm-wayland.desktop`, X11 entry renamed to `FlameWM (X11)`.

Shared with the X11 session: `flamewm-api` (shortcut action names, workspace grid),
`flamewm-platform` (shortcut registry, open-files/terminal actions), and the session bridge
`flamewm-desktop-services`. The shell (`flamewm-shell`) and desktop (`flamewm-desktop`) are still
the same X11 clients as under X11; they run on the XWayland display the compositor publishes.

## Backend selection and startup order

- `FLAMEWM_WAYLAND_BACKEND` accepts `drm`/`native` and `winit`/`nested`. Unset or unknown means the
  compiled default: native when the `drm` feature is on, nested otherwise. There is no inference
  from `DISPLAY`/`WAYLAND_DISPLAY` anywhere in the crate (**code**). A native request against a
  build without `drm` exits 2 with a message.
- `packaging/flamewm-wayland.desktop` runs
  `Exec=/usr/libexec/flamewm/flamewm-wayland-session --backend=drm`. Nested is a developer mode
  only: `--backend=nested` on that wrapper. The wrapper resolves the backend itself (explicit flag,
  then `FLAMEWM_WAYLAND_BACKEND`, then `drm`) and hard-gates the native path on
  `--capabilities` reporting `drm_compiled=true`; `--probe` never gates it.
- Startup order in `main.rs`: backend and seat first (a failure must not leave a half-started
  compositor holding the session), then the Wayland listening socket, then the XWayland spawn, then
  `ready`. `--capabilities` and `--probe` are handled before any of it.
- Teardown drops the backend handle first (releases DRM master and the logind session), then the
  display, then the XWayland client.

## Seat and session access

There is no libseat and no seatd in this stack: not in the crate feature list, not in the build
dependencies, not in the package `Depends`. `logind.rs` implements seat access directly over the
system bus (**code**, plus unit tests):

- Session lookup: `GetSession` from `XDG_SESSION_ID`, falling back to `GetSessionByPID` for our own
  pid, because the variable is absent or stale under login managers, wrappers and `su`.
- Validation before anything is taken: `Class` must be `user`, `Active` must be true, and `Seat`
  must match `XDG_SEAT` when set. A greeter, an inactive session or another seat's session is
  rejected with a named reason (`greeter`, `inactive`, `wrong-seat`, `no-session`) and
  `TakeControl` is never called.
- `TakeControl(false)` (no device pause), then one `TakeDevice` per device when smithay asks to open
  it; fds are released through `ReleaseDevice`. `ReleaseControl` runs on the explicit shutdown path
  and from `Drop`, so an error path or a panic still returns the session.
- Signals (`PauseDevice`, `ResumeDevice`, `Activate`) are read by a 20 ms poll timer that only
  enqueues; the compositor consumes them outside the session borrow.
- `PauseDeviceComplete` is sent only for `pause` (never for `force`/`gone`, which logind has already
  revoked), and only after the compositor quiesces: libinput suspend, KMS pause, in-flight frames
  parked, queued commits stopped. Each paused device is acknowledged exactly once, with a retry
  path for a failed call.
- `ResumeDevice` consumes the descriptor logind hands back: the kept-fd path re-activates master,
  resumes libinput, resets scanout and forces a full redraw; if that fails, the device, GBM, EGL,
  renderer and outputs are rebuilt from the new fd before the old ones are dropped, so a failed
  rebuild leaves the compositor paused rather than half rebuilt. A resume for a device this backend
  does not own is released and logged, never leaked or silently ignored.

Consequence for packaging: the binary links only desktop-stock libraries, so the seat library is not
a runtime or build dependency.

## Feature `drm` and the tty backend

`drm` = `dep:dbus` + `smithay/{backend_gbm,backend_libinput,backend_session,backend_udev}`. Smithay
is used with `backend_session` only (the session-crate alternative is not enabled). The crate lint is
`unsafe_code = "deny"`; the `drm` module and the logind `system` submodule allow it locally for
smithay's EGL constructors and for the dbus fd hand-off.

`drm.rs` (**code**): primary GPU from the seat via udev, `DrmDevice` with master, `GbmDevice`,
EGL/GLES `GlowRenderer`, one `DrmCompositor` per connector, vblank-paced frames, libinput on the
session, Ctrl+Alt+F1..F12 VT switch through `SwitchTo`, connector rescan on udev change.

Scope limits: the first connected connector drives `State::output` and further connectors show the
same scene. The cursor is a software square, not a hardware plane. No output layout, no per-output
scale, no gamma or DPMS.

## Rendering and frame scheduling

**code**, with the per-output rules unit-tested without a GPU:

- The redraw wake is backend-neutral: `State::request_redraw`/`request_full_redraw` set a coalescing
  dirty flag and ping a calloop `Ping` on the tty backend (the nested path keeps its
  `window().request_redraw()`). A request never waits for an unrelated event to be painted.
- Every output carries `OutputFrames { dirty, queued, needs_retry }`. `plan_frame` returns `Idle`,
  `Draw` or `Deferred`: a request that arrives while a frame is queued is deferred to that vblank,
  never dropped. Idle means zero periodic paints.
- The old unconditional 16 ms idle retry is gone. A timer is armed only for a real sync or buffer
  error, at most 3 times, and never while the output is dirty or the session is paused. An empty
  render result arms nothing.
- A resume forces a full redraw (`OutputFrames::wake` plus `request_full_redraw`).
- `FLAMEWM_WAYLAND_DUMP` works on the tty backend too: the pending dump is taken once inside the
  paint pass and rendered offscreen from the primary output.

## Diagnostic entry points

`flamewm-wayland --capabilities` is compile-time only: no bus, no `/dev`, no `PATH`, exit 0, exactly
three lines (`flamewm-capabilities=1`, `drm_compiled=true|false`, `backend=native|nested`). The
packaging and wrapper gates grep `drm_compiled=true` literally.

`flamewm-wayland --probe` is runtime and **advisory**: the wrapper records its exit code and
`[FAIL]` lines in the log and never lets it change the session exit code or gate startup. It reads
connector state from sysfs, queries logind through `busctl` without ever calling `TakeControl`,
checks libraries in the `ldconfig -p` cache, and never opens `/dev/dri` unless this process resolved
its own active user-class session. Its required set follows the selected backend (native: EGL,
GLESv2, gbm, libdrm, libinput, libudev; nested: EGL, GLESv2); the legacy seat-library soname is
reported as informational only and is never required.

## Diagnostics and logging

Structured stderr lines from the compositor:
`FLAMEWM-WAYLAND-STARTUP stage=<seat|drm|outputs|libinput|socket|xwayland|ready> status=<ok|fail|skip>
code=<0..6> detail=<short tokens> t=<ms>ms`. The stage code is the stage order, so a log reader can
order stages without parsing names; `t` is monotonic milliseconds since process start; detail is
reduced to short tokens (no fds, addresses, cookies or environment dumps). A session manager may
start the compositor before its VT/seat is active. After `TakeControl` the compositor calls
`org.freedesktop.login1.Session.Activate` (and `Seat.SwitchTo(XDG_VTNR)` when the variable is set),
polls the `Active` property, and fails closed (`reason=inactive_after_activate`) rather than taking
DRM master on a session that is not ours. Once the event loop starts, a 5 s first-frame watchdog
stops the loop, releases the session and exits non-zero if no frame was queued, so a taken-but-black
VT cannot persist. `drm::init` also refuses to open the GPU and modeset while the session is
inactive.

The wrapper appends its own phases to the same shape (`session`, `runtime`, `log`, `capabilities`,
`probe`, `socket`, `xwayland`, `ready`, `exit`, each with `ms=`) and writes everything to
`$XDG_CACHE_HOME|$HOME/.cache/flamewm/wayland-session.log` (mode 0600, exactly one `.1` rotation).
On failure it also prints a bounded summary to `/dev/tty` (so a VT failure is readable without a
second machine) and rewrites `$XDG_CACHE_HOME|$HOME/.cache/flamewm/wayland-session-last-failure.log`
(0600) with the reason, exit code, signal and the last stage lines. If `stage=ready status=ok` does
not appear within 15 s (`FLAMEWM_WAYLAND_READY_TIMEOUT_MS`), the wrapper terminates the compositor
(TERM, then KILL after a grace period) and reports `reason=no_ready_timeout`. A log that cannot be
created never prevents startup; the inherited streams are used instead. The D-Bus bus address is
never logged.

## Packaging

`scripts/build-deb` builds
`cargo build --release --workspace --locked --features flamewm-wm-wayland/drm`, so the shipped
compositor always has the native backend. Two fail-closed gates:

- Preflight before Cargo: `pkg-config --exists libinput gbm libdrm libudev` and an `Xwayland`
  binary on `PATH`. Nothing is installed by the script.
- Post-build capability gate: `flamewm-wayland --capabilities` must print `drm_compiled=true`,
  otherwise the build fails instead of packaging a Wayland login entry that cannot open a tty.

`scripts/dev-deb` uses the same feature list with the `fast` profile.

`packaging/debian/control.in` `Depends` now carries the Wayland runtime set on top of the existing
X11/python deps: `libgbm1, libdrm2, libinput10, libudev1, libegl1, libgles2, xwayland`. No
seat-library runtime package is pulled in.

## Proven when this record was written

The numbers below are what the recorded runs produced. Re-run the gates instead of quoting them;
they drift as the tree changes.

- `cargo check --features drm` passes; the `drm`-feature unit tests (logind state machine, frame
  planner, capability/backend selection, probe tables) pass.
- `cargo test` (default features) passes, 48 tests at the time of writing.
- Wrapper matrix `scripts/tests/wayland-session-matrix.sh`: 18 cases, 89 checks at the time of
  writing.
- Packaging pytest: 75 cases at the time of writing.
- **Not** proven: the native binary did not link on this host, because `libinput-dev`, `libgbm-dev`
  and `libdrm-dev` (the latter also needed by the preflight) were not installed there. That is a
  missing build dependency on that machine, not a code defect.

## Unverified / required hardware validation

No successful real hardware start, VT switch or LightDM login has been validated. A LightDM login
attempt reached a text cursor and returned to the greeter; the cause is still under investigation,
and nothing below has been observed working. The required procedure is:

1. Install the built `.deb` while logged into an X11 session, so a working session stays available
   for recovery.
2. `flamewm-wayland --capabilities` must show `drm_compiled=true`.
3. Log out and select `FlameWM (Wayland)` in LightDM.
4. On failure, read `~/.cache/flamewm/wayland-session.log` (and its `.1` rotation) plus
   `journalctl -b` for the logind and DRM side; the `FLAMEWM-WAYLAND-STARTUP` lines name the stage
   that failed.
5. On success, verify: first frame on the connector, panel and shell visible, a terminal opens and
   accepts input, pointer and keyboard work, Ctrl+Alt+Fn VT switch away and back (pause, ack,
   resume), and a second login after a VT return.
6. Recover by choosing `FlameWM` (X11) at the login screen; the package still ships that session and
   the Wayland wrapper does not fall back to it by itself.

## Remaining work

1. Layer-shell support exists in the compositor, but the shipped shell, desktop and launcher are
   still X11 clients on XWayland. Native Wayland shell clients are not built.
2. Screenshot and portal integration.
3. Session lock.
4. Native event sources: the compositor drives smithay's `EventLoop<State>` directly.
   `flamewm-reactor` wraps an `EventLoop<()>` whose callbacks take no state, so smithay's sources
   cannot be routed through it without a reactor change (owner decision). No
   `ARCHITECTURE_EXCEPTIONS.toml` entry was added: the guard only admits exceptions for capabilities
   it checks, so a calloop entry would be reported as stale. The control bus pump and the logind
   poll are timer-driven for this reason.
5. Known limitation, owned by the render crate: the terminal body stays black at the default 80%
   opacity under Wayland, because the translucent fill blends over a backdrop captured from the X
   root, which never holds the composited desktop or wallpaper. At `background_opacity = 100` the
   same binary renders its prompt. Either the terminal skips the root backdrop under Wayland or the
   compositor feeds a wallpaper into the XWayland root.
6. Launcher settings parity with X11 is partial: the user's accent is applied, but the selection row
   composes with the translucent accent tokens and reads darker than the X11 reference. Not
   compared pixel-for-pixel.
7. Resize parity (edge-specific `Grab::Resize` edges, Ctrl+Super+drag) and taskbar
   middle-click have no automated coverage in the current harness.
8. Native Wayland clients are shared-memory only: `renderer.bind_wl_display` and a
   `zwp_linux_dmabuf_v1` global are not wired (`smithay/use_system_lib` is not enabled), so
   EGL/dmabuf clients fall back to software or SHM. X11 clients through XWayland are unaffected.
