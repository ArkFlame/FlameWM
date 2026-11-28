# p103 lock-screen fail-safe gaps

STATUS: OPEN

Audit only. No crate edits, no git, no cargo. Evidence is static (source reads and indexed search), so every "runtime" column is UNVERIFIED until a capture or a run confirms it.

## Fault matrix

| Fault | User sees today (per code) | Fail-closed | Recoverable | Failure record with blast radius | Fault test |
| --- | --- | --- | --- | --- | --- |
| Lock client crash, Wayland (ext-session-lock) | Compositor drops the dead surface and paints an opaque placeholder on that output (`session_lock.rs:23` `cover_dead_locker`). Stays locked. Respawn: none found on the Wayland path (UNVERIFIED). | Yes, by code. | Not verified: no respawn means the user may need a VT switch or a restart of the lock client. | No record found on this path. | Unit tests `session_lock.rs:38`, `:39` (placeholder seeding only). No crash test. |
| Lock client crash, X11 | Supervisor thread respawns after a delay (`x11-desktop/src/lock.rs:102`), crash-loop backoff at `lock.rs:53`. | Yes while respawns succeed. | Yes, the respawn restores the prompt. | Message to stderr only (`lock.rs:102`), no failure record. | Only `decide()` backoff unit test `lock.rs:432`. |
| Lock client respawn fails, X11 | Prints error, runs `loginctl lock-session`, then returns (`lock.rs:111-115`). No further supervision. The screen depends on the logind lock hint being handled by something else. | Not proven. If nothing handles the logind lock hint, the desktop can be visible with no lock client. | Unclear. A VT switch and a relogin are the fallback. | stderr only, no record. | None found. The `loginctl` fallback has no test. |
| Lock client fails to start: missing font | `FontSlot::Missing` exists in the settings or font loader (file not pinned in this audit). Presentation failure is meant to fall back to a solid colour. | Intended yes. | Yes if the lock paints. | Not verified. | Settings tests only (`crates/flamewm-lock/src/settings.rs:370`). |
| Lock client fails to start: missing or undecodable wallpaper | `fail_safe.rs:3` says it must draw the solid theme colour. The decision is `fail_safe.rs:33` `decide()`. **No runtime call site.** `decide` is referenced only inside `fail_safe.rs` tests (`:58`, `:91`, `:104`). The real paint path does not consult it (grep of `crates/flamewm-lock/src` finds no other caller). | Test-only proof. The runtime path is not wired. | Not verified. | None. | `fail_safe.rs:45-72` checks the pure table, not the wiring. |
| Lock client fails to start: missing GPU or render fault | `fail_safe.rs:34` `degraded` flags `render`, but no producer sets `Faults.render` at runtime. Compositor side: `render/gl_probe.rs:65` `is_software_renderer` picks a class. No lock-specific GPU fallback was found in `session_lock.rs`, `lock_cover.rs` or `render/mod.rs:105,132`. | Not verified. Software fallback while locked is UNVERIFIED. | Not verified. | None. | `renderer_class_tests.rs:30` covers the classifier, not the lock. `docs/FAULT_TESTS.md` has no lock rows (zero matches for "lock"). |
| PAM or auth helper missing | `pam.rs:117` and `:132` return `Outcome::Unavailable`. `state.rs:120` handles it. The lock stays up; no unlock. `pam.rs` test `missing_libpam_stays_locked`. Record: `pam.rs` `record_failure("pam", ...)` is called on the authenticate and acct paths, not on the dlopen-missing path (UNVERIFIED for the load path). | Yes, by test and code (`state.rs:224`). | No way to unlock if PAM is absent. The user needs a VT switch or a restart. This is a deliberate fail-closed trade, but no recovery is documented. | Partial: `record_failure` is not reached on the load-failure path (UNVERIFIED). | `pam.rs` unit test only. `state.rs:224` checks Denied and Unavailable do not unlock. |
| Compositor GPU fallback while locked | Not found. Locked compositor paints only lock surfaces and the placeholder (`session_lock.rs` header). Whether a GPU reset or fallback re-enters the desktop while locked is UNVERIFIED. | UNVERIFIED. | UNVERIFIED. | None. | None in `FAULT_TESTS.md`. |

## Gap table (file:line)

| # | Gap | File:line | Severity |
| --- | --- | --- | --- |
| G1 | `fail_safe::decide` is not called by the runtime lock paint path. Presentation faults (font, wallpaper, render, PAM) are untested at runtime. | `crates/flamewm-lock/src/fail_safe.rs:33`, `:58` (test only) | High |
| G2 | X11 respawn failure ends supervision and relies on `loginctl lock-session` with no lock client. Possible unlocked desktop. | `crates/flamewm-x11-desktop/src/lock.rs:111-115` | High |
| G3 | No failure record on the X11 crash, respawn or `loginctl` paths. Stderr only. | `crates/flamewm-x11-desktop/src/lock.rs:102`, `:111` | Medium |
| G4 | Wayland lock client crash has no respawn or recovery path found. Placeholder keeps it locked but there is no way back in without a VT switch. | `crates/flamewm-wm-wayland/src/session_lock.rs:23` | Medium |
| G5 | PAM load failure (`dlopen` miss) does not write a failure record; `record_failure` covers only authenticate and acct. | `crates/flamewm-lock/src/pam.rs:117`, `:132` | Medium |
| G6 | `docs/FAULT_TESTS.md` has no lock rows. No fault test covers any lock failure. | `docs/FAULT_TESTS.md` (whole file) | High |
| G7 | Compositor GPU fallback while locked is not analysed or tested. | `crates/flamewm-wm-wayland/src/render/gl_probe.rs:65`, `src/render/mod.rs:105,132` | High (UNVERIFIED) |

## Fix specs (max 3)

1. **Wire the fail-safe.** In the lock paint path (`crates/flamewm-lock/src/wayland.rs` and the paint module that consumes the wallpaper and font), set `Faults.render`, `.pam` and `.background` from the real results, call `fail_safe::decide`, and draw `solid_colour` when `solid_colour` is true. Add a fault test that forces each fault and asserts `show_lock` and `!unlock` on the paint output. Add the matching rows to `docs/FAULT_TESTS.md` (G1, G6).
2. **Close the X11 respawn gap.** In `crates/flamewm-x11-desktop/src/lock.rs:111-115`, replace the bare `loginctl lock-session` with a check that a lock client is running after the call. If none, keep retrying with backoff and write a failure record (file, reason, blast radius "session visible unlocked"). Add a test for the respawn-failure branch (G2, G3).
3. **Record and test the PAM and render-fault paths.** In `crates/flamewm-lock/src/pam.rs` call `record_failure("pam", ...)` on the dlopen-miss path (`:117`). In `crates/flamewm-wm-wayland`, add a lock-specific GPU fallback test that asserts the session stays covered with the placeholder. Add both to `docs/FAULT_TESTS.md` and the `CASES` table of `scripts/run-fault-tests` (G4, G5, G7).

## Not verified

- Runtime behaviour of every row. This audit is static. Needs a capture run of the lock with each fault injected.
- Wayland respawn path, GPU reset while locked, and the exact font and wallpaper files.

## Orchestrator review (read fail_safe.rs)
- G1 is downgraded: `decide` returns a constant (`show_lock=true`, `unlock=false`; only `solid_colour` varies). Wiring it into the paint path would add no safety. The real checks are runtime ones: a wallpaper/font/PAM fault must still paint a cover (p01 covers wallpaper) and never release. Replace the G1 fix with one runtime fault test per fault (render, PAM dlopen miss, wallpaper) asserting the lock stays covered.
- G2 fixed UNBUILT in `crates/flamewm-x11-desktop/src/lock.rs` (retry the respawn every LOCK_CRASH_LOOP_DELAY instead of ending supervision). A test needs an injectable spawn function.

## w12: Wayland lock-client unclean exit — respawn with bounded backoff

Read-only audit of `crates/flamewm-wm-wayland/src/session_lock.rs` at workspace version 0.1.7. Evidence is static source reads, so all runtime claims below are UNVERIFIED. No crate edits were made by this unit.

### What happens today (file:line)

- `session_lock.rs:171-193` `new_surface` is the only path that installs a live lock surface and marks its output covered (`:187`).
- `render/mod.rs:840` calls `prune_dead_locks(state)` every frame. `session_lock.rs:280-309` finds surfaces whose client is gone (`!l.surface.alive()`, `:285`), calls `cover_dead_locker` (`:258-261`: `uncover` then `cover_placeholder`) for each dead output, drops the dead entries (`:291`) and refocuses only when no live surface remains (`:304-307`).
- `session_lock.rs:215-221` `screen_for(true, None)` returns `Screen::Blank`; `render/mod.rs:850` paints `solid_lock_cover`, so the desktop is never revealed. `session_locked` is set only by `enter_lock` (`:199-203`) and cleared only by `unlock` (`:160-169`); `prune_dead_locks` never clears it. An unclean exit is therefore fail-closed: opaque placeholder, session still locked, keyboard shortcuts and window pointer reach still disabled (`:226-234`).
- There is **no respawn** on this path. `docs/backlog/wayland-shared.md:112` records the design: "a dead locker leaves the session locked ... no respawn; WM owns no lock process." A grep of `crates/flamewm-wm-wayland/src` finds no lock-client `Command::new`/`spawn` or lock supervisor. The only crash supervision in the repo is X11 (`crates/flamewm-x11-desktop/src/lock.rs:54-66` `decide`, `:80-121` `supervise_lock`, constants `:35,38,41,42`). The user sees a permanently blank/placeholder screen with no password prompt until a VT switch or a manual client restart. This is gap G4 (`p103-lock-failsafe-gaps.md:27`) with no failure record.

### Vendor precedents (`.vendor/**`, read-only evidence)

- `smithay-master/src/wayland/session_lock/lock.rs:136-143`: on `destroyed` (client disconnected without unlocking) smithay sets `LockStatus::Defunct` and says it is "up to the compositor's policy to decide whether it is allowed for another client to connect and take over the session-locker responsibility." Smithay does not respawn. This matches the active `alive()` pruning: a dead lock stops being authoritative, so a respawned client may issue a fresh `lock()`.
- `cosmic-epoch/cosmic-comp/src/wayland/handlers/session_lock.rs:19-43`: `lock()` rejects a second lock while the current one's client is still alive (`:22-31`), confirms immediately and seeds surfaces (`:33-38`). No respawn in the compositor; the lock client comes from the session (`cosmic-epoch/pop-launcher/scripts/session/session-lock.sh:10` `xdg-screensaver lock`).
- `kwin-master/src/wayland_server.cpp:613-651`: KWin delegates lock-process lifecycle to `ScreenLocker::KSldApp` through a dedicated Wayland client connection (`createScreenLockerConnection`, `:672-680`) and recreates that connection on `aboutToStartGreeter` (`:620-631`). Respawn/backoff lives in kscreenlocker, not in KWin. kscreenlocker itself is not vendored (only a dependency reference in `kwin-master/.kde-ci.yml:35,48`), so its exact backoff is UNVERIFIED here — do not transplant it.

### Precise fix spec

Ownership: put the supervisor in the process that owns the Wayland `flamewm-lock` client, outside `SessionLockHandler`, mirroring X11 `supervise_lock` — `docs/backlog/wayland-shared.md:112` establishes the compositor owns no lock process. Leave the compositor protocol handler unchanged; it keeps painting the placeholder while no live surface exists.

1. **Respawn on unclean exit.** Own the spawned client child. On any non-success exit (non-zero status or a signal, which also drops the `ext-session-lock` connection) respawn; on a success exit treat it as an unlock and stop. Never call `unlock()` on the compositor's behalf from this path.
2. **Bounded backoff.** Mirror X11 `decide()` (`lock.rs:54-66`): short `LOCK_RESPAWN_DELAY` initially, escalate to `LOCK_CRASH_LOOP_DELAY` after `LOCK_RESPAWN_CAP` restarts in a rolling `LOCK_RESPAWN_WINDOW` (`:35,38,41,42`). Cap the escalation (doubling with a fixed maximum) so a crash loop neither hot-spins nor gives up: respawns are unbounded in count, bounded in rate. Inject `spawn`/`wait`/`sleep` closures so policy is testable without real processes, as X11 `respawn_until_started<T>` does at `:125-146`. No new dependencies.
3. **Placeholder remains, visuals pixel-identical.** Keep `session_lock.rs:280-309`, `:215-221` and `render/mod.rs:840,850` exactly as-is. While the client is down the output stays covered by `cover_dead_locker` (`:258-261`) and `Screen::Blank`, identical to today's paint. When the respawned client connects, smithay's `Defunct` status (`vendor .../lock.rs:142`) lets its fresh `lock()` be accepted; `lock()` (`session_lock.rs:144-158`) re-seeds the cover and `new_surface` (`:171-193`) replaces placeholders with live surfaces. Do not weaken `shortcuts_active`/`windows_reachable` (`:226-234`) during the gap.
4. **Record and test.** Emit one failure record per failed respawn with blast radius "session locked, prompt unavailable" (X11 precedent `lock.rs:135-137`), and add a pure unit test beside the backoff function asserting the ramp, the cap and that a success exit stops supervision (mirroring X11 `decide()` tests at `lock.rs:494-520`). Mark all resulting code UNBUILT until a build/test run is allowed.

Risks: (a) if two lock clients ever race, cosmic-comp's alive-check (`handlers/session_lock.rs:22-31`) suggests rejecting the second, while the active `lock()` clears and re-seeds unconditionally (`session_lock.rs:150-151`); verify takeover semantics before enabling multi-client respawn. (b) kscreenlocker's exact backoff is absent from the corpus, so the delay values above are proposals copied from the X11 path, not proven optimal. (c) all of this is static evidence; no runtime capture confirms the observable placeholder paint or respawn timing.

## Result

UNBUILT. No cargo, tests or gates were run; evidence is source only.
- X11 respawn (G2, G3): `respawn_until_started<T>` in `crates/flamewm-x11-desktop/src/lock.rs` takes spawn, loginctl and sleep closures; `supervise_lock` calls it with the same logic. Tests `failed_respawns_retry_at_crash_loop_delay_and_ask_loginctl_once` (3 failures then success: 3 sleeps of LOCK_CRASH_LOOP_DELAY, loginctl once) and `immediate_respawn_skips_sleep_and_loginctl`.
- Runtime-level tests in `crates/flamewm-lock` (no GUI): `render_fault_keeps_a_cover_and_the_reducer_stays_locked` (fail_safe.rs), `dlopen_miss_leaves_the_reducer_locked_with_a_cover` (pam.rs), `undecodable_wallpaper_still_yields_an_opaque_full_size_cover` (document.rs, garbage file and missing file both give an opaque 64x48 navy cover).
- Not covered without GUI or side effects: the dlopen miss through `authenticate()` itself (it selects libpam by fixed name, and a miss there writes a real failure record to the session log), so the test asserts `DynamicPam::load_from` failing plus the `Unavailable` reducer outcome. Render fault is a flag only: no lock producer sets `Faults.render` at runtime, so the test proves the decision, not a real GPU failure (G7 stays open). G1 stays downgraded per the review above.
- Re-verify: `flamewm-x11-desktop` lock supervision tests and `flamewm-lock` tests, each with `echo $?`, after a build is allowed.
