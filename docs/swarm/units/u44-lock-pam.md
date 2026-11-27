# u44 — Lock screen PAM / auth backend / session-lock failure fallback audit

Status: **UNBUILT** — source edits are complete by inspection only. The worker contract forbids
`git`, `cargo`, builds, tests, Python and ad-hoc scripts; the orchestrator builds, tests and commits
after the shared-checkout batch lands. No build, test or runtime evidence is claimed here.

## Route and evidence

- Skills loaded: `flamewm-source-first`, `flamewm-failure-resolution`, `flamewm-rust-systems`,
  `no-adhoc-scripts`. Governance read: `AGENTS.md`, workspace version `0.1.7` (`Cargo.toml`),
  `.opencode/{HARNESS,ROUTES}.md`, `.agents/{README,CONTRACTS,JOBS}.md`. Git-state inspection was
  skipped: the worker contract forbids `git`.
- Search: `rg -n -i 'pam|authenticate|ext_session_lock|session_lock|Outcome::|unavailable|Denied'`
  over `crates/flamewm-lock/src` and `tests` (per-file counts: `pam.rs` 103, `wayland.rs` 37,
  `state.rs` 4, `run.rs` 4, `main.rs` 2, `lib.rs` 2, `run/input.rs` 2, `run/tick.rs` 2,
  `run/live.rs` 1, `secret.rs` 1). `rg` for hard failures found no production `panic!` on these
  paths; the risks are swallowed causes, a permanent `Authenticating` state and silent `Ok`.
- Read in full/part: `pam.rs`, `state.rs`, `wayland.rs`, `run/input.rs`, `run/tick.rs`, `main.rs`,
  `lib.rs`, `view.rs`, `document.rs` (status-line sink only), `Cargo.toml`, plus the typed failure
  owner `crates/flamewm-session-core/src/diagnostics/failure.rs` (`FailureRecord`, `BlastRadius`,
  `append_record`, `session_log_path`) and precedents `crates/flamewm-platform/src/host.rs:93-119`
  and `crates/flamewm-wm-wayland/src/main.rs:413-450`. `run.rs`, `document.rs`, `view.rs` were
  treated as read-only because other workers own them.
- Guardrail preserved: `state::LockState::finish` (`state.rs:108-125`) returns `true` only for
  `Outcome::Unlocked`; `Denied` and `Unavailable` stay locked. Every added record uses
  `fallback_taken=false`, so no weaker unlock path is introduced. Passwords are never put in a
  record; only PAM codes and static causes are.

## Failure-path audit and changes

Rows marked "read only" are audited but unchanged because of the three-file edit cap.

| file:line | failure | now | change | risk |
| --- | --- | --- | --- | --- |
| `crates/flamewm-lock/Cargo.toml:19` | No typed failure-record sink in this crate | Existing `FailureRecord` owner lives in `flamewm-session-core`; lock could only `eprintln!` | Added `flamewm-session-core = { path = "../flamewm-session-core" }` | New build edge; no cycle (`flamewm-session-core` does not depend on `flamewm-lock`). UNBUILT. |
| `crates/flamewm-lock/src/pam.rs:88-111` | Any PAM backend failure had no durable record | Cause discarded at each `Outcome::Unavailable`/`Denied` return | Added `record_failure`: one typed `FailureRecord` (`stage=pam`, `SubsystemError`, `still_works=session,shell,wm`, `degraded=lock_auth,lock_surface`, `fallback_taken=false`) appended via `append_record` to the shared session log; write errors only print stderr | Uses `XDG_STATE_HOME`/`HOME` with a fixed `/tmp` fallback, as in `host.rs`; a failed append never changes the lock decision. |
| `crates/flamewm-lock/src/pam.rs:145-151` | `libpam.so.0`/`libpam.so` missing or a required symbol absent | `load_from` error dropped; `Outcome::Unavailable`, no cause | Logs `PAM library unavailable: <dlopen/dlerror detail>` then the same `Unavailable` | Detail contains library/symbol names only, never a secret. |
| `crates/flamewm-lock/src/pam.rs:152-158` | PAM session setup fails (e.g. invalid user string) | Session error dropped; `Unavailable` | Logs `PAM session setup failed: <cause>` then `Unavailable` | Same fail-closed outcome. |
| `crates/flamewm-lock/src/pam.rs:115-118` | `pam_start` fails: service `flamewm-lock` misconfigured / PAM absent | `session.start().is_err()` → bare `Unavailable` | Logs `pam_start failed with code <n>`; still `Unavailable`, `end` not called as before | Mock tests now append a record; harmless, no state change. |
| `crates/flamewm-lock/src/pam.rs:119-133` | `pam_authenticate` returns a non-PAM_SUCCESS/error code other than AUTH_ERR/USER_UNKNOWN (conversation error, system error) | Bare `Unavailable`; on-screen "Authentication unavailable" | Logs `pam_authenticate failed with code <n>`; outcome unchanged | Wrong password / unknown user still map to plain `Denied` with no record (normal user error). |
| `crates/flamewm-lock/src/pam.rs:121-127` | `pam_acct_mgmt` fails after a valid password (expired/new token required, account policy) | Bare `Denied` → "Authentication failed" plus backoff | Logs `pam_acct_mgmt failed with code <n>`; still `Denied`, never `Unlocked` | One record per attempt; backoff unchanged. |
| `crates/flamewm-lock/src/state.rs:28-38,108-125` (read only) | Backend failure must show a clear line and keep the lock | `Unavailable` → "Authentication unavailable", `Denied` → "Authentication failed"; only `Unlocked` releases | Unchanged; the new repaint in `wayland.rs` makes this line actually reach the screen | The X11 loop repaints every tick, so it was already visible there. |
| `crates/flamewm-lock/src/run/input.rs:72-91` (read only) | X11 PAM worker cannot be spawned | Sends `Outcome::Unavailable` and wakes the loop; no failure record | Unchanged (fourth file would exceed the 3-edit cap) | No record for this rare X11-only path; it stays locked and shows the error line. |
| `crates/flamewm-lock/src/run/tick.rs:67-75` (read only) | X11 auth worker dies without a verdict | `while let Ok(outcome) = try_recv()` silently ignores the loss; loop holds its own sender, so no disconnect is observable → `Authenticating` forever | Unchanged; a timeout/watchdog needs new state in `run.rs`, owned by another worker | Residual X11 lockout risk; not introduced by this change. |
| `crates/flamewm-lock/src/wayland.rs:546-566,569` | Session-lock protocol unavailable (no `WAYLAND_DISPLAY`, connect/registry/global/bind failure) or paint failure | `run_with` returned `LockError` via `?`; caller printed and exited, but no failure record | Wrapped the protocol loop in `run_with` → `run_with_inner`; every `Err` now emits exactly one `stage=lock-protocol` record with the same blast radius before returning | Caller still exits `FAILURE`; no lock surface exists to paint an on-screen line, so stderr is the only line here. |
| `crates/flamewm-lock/src/wayland.rs:614-617` | Compositor sends `finished` (refused/ended lock) or a surface paint fails and sets `finished` | Loop returned `Ok(())`: a silent successful exit with no lock held | Returns `Err(LockError::Refused)`; the wrapper records it and `main.rs:171-175` prints `flamewm-lock: compositor refused the session lock` with a failure exit | Refusal cause is generic; the specific paint error is still printed at `wayland.rs:511-513`. No `unlock_and_destroy` is sent, so the client never unlocks itself. |
| `crates/flamewm-lock/src/wayland.rs:746-754` | Auth helper thread cannot be spawned | Applied `Unavailable` without a record | Records `cannot spawn the PAM authentication worker`, keeps `Unavailable` and forces a repaint | Screen stays locked; no retry that could bypass authentication. |
| `crates/flamewm-lock/src/wayland.rs:762-773` | Auth worker exits/panics before sending any outcome ("auth helper fails") | `try_recv()` error ignored → `Status::Authenticating` forever; `accepts_input` false → permanent lockout/hang | Matches `TryRecvError::Disconnected`, records one `pam` failure, prints `PAM worker exited without an outcome; screen stays locked`, applies `Outcome::Unavailable` | `Empty` still returns quietly while the helper runs; the failure path always ends as locked with an error line. |
| `crates/flamewm-lock/src/wayland.rs:782-784` | Outcome applied but the status line never repainted | `dirty` was only set on submit; after the result the surface kept showing "Authenticating…" | Sets `dirty = true` for every applied outcome, so `view.rs:56-61`/`document.rs:541` show "Authentication unavailable" / "Authentication failed" | One extra repaint per attempt; no effect on the authentication decision. |
| `crates/flamewm-lock/src/main.rs:78-87,118-176` (read only) | Caller handling of `LockError` and exit code | Prints `flamewm-lock: {error}` and returns `ExitCode::FAILURE`; no record | Unchanged; the new records are emitted before the error returns | Keeps the existing non-zero exit for unavailable protocol and missing `wayland` feature. |

## Verification

- Not run by contract: `git`, `cargo`, builds, tests, Python, ad-hoc scripts, runtime compositors.
- Static checks only: re-read the three edited files; confirmed the typed API signatures against
  `failure.rs`, the status-line path through `view.rs`/`document.rs`, and that `tests/wayland_unlock.rs`
  calls the unchanged public `run_with` signature. No build or runtime claim is made.

PARTIAL — all named failure paths now either record a typed failure with blast radius and keep the
screen locked, or are explicitly listed as read-only residual risks; the Wayland lost-helper hang
and stale status line are fixed, but no build/test/runtime verification was permitted, so the unit
is UNBUILT and the X11 lost-helper watchdog remains out of scope.
