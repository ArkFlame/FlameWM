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
