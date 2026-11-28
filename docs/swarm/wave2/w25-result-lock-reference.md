# w25: reference-lock — keeping the session covered when the locker crashes

## Answer

Both active FlameWM paths already **fail closed**: a dead lock client never reveals the desktop.

- Wayland (`crates/flamewm-wm-wayland/src/session_lock.rs`) keeps `session_locked` true when the
  client dies, replaces the dead surface with an opaque placeholder, and paints a full-output solid
  cover while no live surface exists (`render/mod.rs:837-850`). It does **not** restart the client.
- X11 (`crates/flamewm-x11-desktop/src/lock.rs`) owns the lock child in a supervisor thread that
  never gives up: every non-success exit is respawned with a 500 ms delay, growing to 5 s after 5
  restarts in a 30 s window.

The referenced vendors all combine the two ideas: keep the credential UI covered **and** restart the
greeter. KWin recreates its lock-client Wayland connection on greeter start and tears it down on
unlock; smithay's protocol marks a disconnected locker `Defunct` (not unlocked) and leaves takeover
policy to the compositor; cosmic-comp keys surfaces per output; sddm and lightdm tear down/restart
the display when the greeter dies, but do not restart on an ordinary auth error.

This is a **read-only audit**: no production edits, no new dependencies, no gate changes, no visual
changes. Reference material under `.vendor/**` is evidence only, never a dependency or runtime input.

## Source evidence (active code)

| file:line | finding |
| --- | --- |
| `crates/flamewm-wm-wayland/src/session_lock.rs:136-158` | `lock()` confirms immediately, clears all outputs, seeds a placeholder per output, enters the locked state and drops any pre-lock move/resize grab and resize cursor. |
| `crates/flamewm-wm-wayland/src/session_lock.rs:171-193` | `new_surface()` configures a full-output surface, focuses the lock client's keyboard, replaces that output's placeholder with the surface, and records `LockedOutput`. |
| `crates/flamewm-wm-wayland/src/session_lock.rs:205-221` | `screen_for`: live surface -> `Lock`; locked with no surface -> `Blank`; unlocked with no surface -> `Desktop`. A dead locker can never produce `Desktop`. |
| `crates/flamewm-wm-wayland/src/session_lock.rs:223-234` | While locked, shortcuts are inactive and windows unreachable, so the user cannot act on the desktop behind the dead client. |
| `crates/flamewm-wm-wayland/src/session_lock.rs:256-261`, `277-309` | `cover_dead_locker` / `prune_dead_locks`: dead surfaces are dropped, their outputs get an opaque placeholder, a newer live surface on the same output wins, and focus is returned when no lock surface remains. |
| `crates/flamewm-wm-wayland/src/render/mod.rs:837-850` | Every frame calls `prune_dead_locks`; `Screen::Blank` paints `solid_lock_cover(Rectangle::from_size(size))` over the whole output. No desktop scene is built while locked. |
| `crates/flamewm-wm-wayland/src/session_lock.rs:312-489` | Pure unit tests cover blank-on-dead-locker, placeholder coverage, every-output seeding, newer-live-wins, and the grab/cursor reset. |
| `crates/flamewm-x11-desktop/src/lock.rs:34-42` | Constants: 500 ms respawn, cap 5 restarts in a 30 s window, 5 s crash-loop delay; comment states supervision never gives up. |
| `crates/flamewm-x11-desktop/src/lock.rs:46-66` | `decide`: success -> `Stop`; failure at/over cap -> `Respawn{5 s}`; otherwise `Respawn{500 ms}`. |
| `crates/flamewm-x11-desktop/src/lock.rs:80-121` | `supervise_lock` waits the child, prunes old restart timestamps, sleeps the decided delay, then respawns; a crash-loop line is logged. |
| `crates/flamewm-x11-desktop/src/lock.rs:125-146` | `respawn_until_started` retries spawn forever at 5 s, logs with an explicit unlocked-until-restart blast radius, and asks `loginctl lock-session` once per retry run. |
| `crates/flamewm-x11-desktop/src/lock.rs:148-170` | `start_lock_screen` spawns the lock binary detached and hands it to a `flamewm-lock-supervisor` thread; the loginctl fallback asks any other locker to cover the session. |
| `crates/flamewm-x11-desktop/src/lock.rs:424-500` | Tests pin success-stops, nonzero/SIGKILL respawn, cap -> long delay, retry-on-spawn-failure, and single loginctl call. |

## Reference evidence (read only)

| path | finding |
| --- | --- |
| `.vendor/smithay-master/src/wayland/session_lock/mod.rs:74-88` | `LockStatus::{Unlocked, Locked, Defunct}`; `Defunct` means the lock client disconnected without unlocking. |
| `.vendor/smithay-master/src/wayland/session_lock/lock.rs:117-125`, `136-144` | Only `UnlockAndDestroy` unlocks. `destroyed()` on client disconnect sets `Defunct` and explicitly leaves takeover policy to the compositor — the compositor must not treat client death as unlock. |
| `.vendor/cosmic-epoch/cosmic-comp/src/wayland/handlers/session_lock.rs:19-73` | Rejects a second live lock, confirms at once, keeps `SessionLock { ext_session_lock, surfaces: HashMap<Output, LockSurface> }`, clears on unlock, configures full output geometry per new surface. Matches FlameWM's per-output surface map and immediate confirm. |
| `.vendor/cosmic-epoch/cosmic-greeter/src/locker.rs:130` | `//TODO: send errors to GUI, restart process` — the greeter itself does not own crash recovery. |
| `.vendor/cosmic-epoch/cosmic-idle/src/main.rs:107-123` | Runs the configured lock command via `sh -c`, waits in a detached thread, and logs non-success; no respawn. Same fail-closed-but-no-restart shape as FlameWM Wayland. |
| `.vendor/kwin-master/src/wayland_server.cpp:613-650` | `initScreenLocker` wires `KSldApp`: on `aboutToStartGreeter` it destroys the old lock-client connection and creates a fresh socket/FD; on `unlocked` it destroys the connection and resets the FD; `lockStateChanged` is forwarded. `.vendor/kwin-master/src/main_wayland.cpp:447-455` exposes `--lockscreen` / `--no-lockscreen`. |
| `.vendor/sddm-develop/src/daemon/Greeter.cpp:133-171`, `269-284` | Greeter is a `QProcess`; start failure returns false; `finished()` cleans up. `.vendor/sddm-develop/src/daemon/Display.cpp:541-549` restarts the display/greeter on helper exit except `HELPER_AUTH_ERROR` — avoid restart loops on ordinary wrong passwords. |
| `.vendor/lightdm-main/src/session.c:454-489` | `session_watch_cb` logs exit/signal, converts death mid-authentication into `PAM_CONV_ERR` ("Authentication stopped before completion"), emits `STOPPED`; the display manager tears down rather than exposing a session. |

## Ported-algorithm specs

### A. Fail-closed coverage (Wayland — already implemented, keep as-is)

1. On `lock`: confirm at once; seed **every** layout output with an opaque placeholder; set
   `session_locked = true`; drop pre-lock grab and resize cursor. Never wait for the client's first
   frame to start covering — reference: cosmic-comp `lock()` and FlameWM `seed_cover`.
2. On `new_surface`: configure the surface to the full logical output size (scale-aware), focus the
   lock client's keyboard, and replace that output's placeholder with the surface.
3. Every render pass: drop surfaces whose client is no longer alive; put an opaque placeholder back
   on each dead output; keep a newer live surface on the same output if one exists; move focus back
   only when no live lock surface remains. Reference: smithay `Defunct` (a disconnected locker is not
   an unlock) and cosmic-comp's per-output surface map.
4. Render decision: `session_locked && no live surface` -> full-output solid cover; `session_locked &&
   live surface` -> lock scene; `!session_locked` -> desktop scene. Shortcuts and window pointer
   reach stay off while locked. This is pixel-identical to the live lock's opaque coverage and is
   the compositor-side equivalent of sddm/lightdm tearing down rather than exposing the session.

### B. Never-give-up locker supervision (X11 — already implemented; candidate for Wayland recovery)

1. Own the lock child in a dedicated supervisor thread; `wait()` it, prune restart timestamps older
   than the 30 s window, then classify: success -> stop supervision (the user unlocked); any other
   exit (non-zero or signal) -> respawn. Reference: X11 `decide()` and sddm's "restart unless auth
   error" rule — treat an unlock as the only legitimate stop.
2. Backoff: 500 ms up to 5 restarts in the window, then 5 s (crash-loop delay). Never give up; a
   crash loop cannot leave the session uncovered. Reference: `LOCK_RESPAWN_*` constants and tests.
3. Spawn failures retry forever at the crash-loop delay; ask `loginctl lock-session` **once per
   retry run** so any other locker can cover the session in the meantime. Reference: X11
   `respawn_until_started`, sddm/lightdm display teardown on greeter death.
4. Keep the decision pure (`decide(exit_status, recent_restarts)`) so success, signal, cap and
   backoff are unit-testable without spawning processes or sleeping, as the existing X11 tests do.
5. Porting note for a future Wayland supervisor: it must **not** unlock or drop the cover when the
   lock client dies — only a real `UnlockAndDestroy` (smithay) may end the lock. Restarting the
   client and keeping the cover opaque are complementary, not alternatives.

### C. Greeter-connection lifecycle (KWin — adapt, do not transplant)

KWin's separate lock-client connection is recreated on `aboutToStartGreeter` and destroyed on
`unlocked`, so a stale connection cannot keep a dead greeter's lock alive. In FlameWM's single-seat
compositor the equivalent is `LockSurface::alive()` plus `prune_dead_locks`: dead resources are
dropped from `locked_outputs` and their outputs recovered with placeholders. No extra Wayland
connection is needed; do not copy KWin's socketpair code or its libkscreenlocker dependency.

### D. Decision boundary

- Keep reference algorithms as specs only in this audit. Any future Wayland locker respawn would be
  a separate change with its own unit tests beside the code; it must not loosen the lock gates or
  alter the lock scene.
- Do not treat a disconnected/dead locker as an unlock in either backend; that is the one invariant
  every reference agrees on.

## Gaps and residual risks

- Wayland recovers **coverage** but not a usable lock screen: if `flamewm-lock` never comes back,
  the user sees a permanent opaque cover with no password prompt (focus returns to a window but
  input is gated). KWin/sddm/lightdm/cosmic-idle all restart the greeter; a future change should add
  a bounded Wayland locker supervisor per Spec B, with the cover staying opaque throughout.
- `crates/flamewm-x11-desktop/src/lock.rs:83-88`: `child.wait()` failure currently logs and returns
  from `supervise_lock`, ending supervision. The desktop is not covered on the X11 path, so this is
  the one place a wait error could leave the session exposed. The reference pattern (sddm `finished`,
  lightdm `session_watch_cb`) treats any child death/errors as a restartable failure. Not changed
  here: an audit-only fix would alter supervision behavior and needs its own test and build.
- `crates/flamewm-x11-desktop/src/lock.rs:160-168`: the `Loginctl` branch relies on an external
  locker actually listening; `respawn_until_started` only asks once. Same as the existing design;
  unchanged.
- No runtime claim is made: this worker ran only `rg`/`Read`/`shell` inspection; no build, no
  runtime launch, no `git`/`cargo`. All findings are static source evidence.

## Status of edits

**No production edits were made** (read-only audit). The only written files are this spec doc and
the `w25-reference-lock.md` unit record. No code change is marked UNBUILT because there is no code
change; any future implementation of Specs B/D must be marked UNBUILT until the orchestrator builds
and tests it. Visuals stay pixel-identical: the opaque lock cover and all lock scenes are untouched.
No gates were loosened and no dependency was added.
