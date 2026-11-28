# z06 Lock fail-closed with PAM absent: fault-injection design

STATUS: DESIGN DELIVERED; step 1 implemented UNBUILT.

Scope: `crates/flamewm-lock/src/{pam.rs,state.rs,fail_safe.rs,wayland.rs,run/input.rs}` and this
document. No new dependency, no visual change, no pixel change. The only additive production
surface is the typed loader seam `pam::authenticate_with`.

## Current structure (file:line, after step 1)

- `crates/flamewm-lock/src/pam.rs:51-58` — `Outcome` is the typed verdict (`Unlocked`, `Denied`,
  `Unavailable`). Only `Unlocked` can release the lock.
- `crates/flamewm-lock/src/pam.rs:122-148` — `run_flow()` is the single decision gate: a
  `pam_start` error yields `Unavailable`; `PAM_AUTH_ERR` and `PAM_USER_UNKNOWN` yield `Denied`;
  other authentication errors yield `Unavailable`; an account-management failure yields `Denied`;
  only authentication success followed by account success yields `Unlocked`.
- `crates/flamewm-lock/src/pam.rs:150-177` — the typed facade `authenticate()` delegates to
  `authenticate_with(&LIBRARY_NAMES, ...)`, and the additive `authenticate_with(names, ...)`
  seam accepts injected library names. Both map every backend failure to `Unavailable`.
- `crates/flamewm-lock/src/pam.rs:439-464` — mock-driven cases inject start failure, a wrong
  password, and a conversation error; `crates/flamewm-lock/src/pam.rs:494-502` injects account
  failures.
- `crates/flamewm-lock/src/pam.rs:504-507` and `:521-538` — a dlopen miss is asserted at the
  loader and, separately, through the reducer.
- `crates/flamewm-lock/src/pam.rs:541-559` (step 1) — an absent backend is injected through the
  typed facade for three credential shapes; every case must be `Unavailable` and must leave the
  reducer locked.
- `crates/flamewm-lock/src/pam.rs:561-568` (step 1) — the `PAM_USER_UNKNOWN` branch injects an
  unknown account with a permissive account double and proves a denial, not a success.
- `crates/flamewm-lock/src/state.rs:108-125` — `LockState::finish()` returns `true` only for
  `Outcome::Unlocked`; `Denied` adds backoff and `Unavailable` sets a visible status.
- `crates/flamewm-lock/src/fail_safe.rs:15-24,33-40` — `decide()` always returns
  `show_lock: true` and `unlock: false`; `pam` is one of three counted faults.
- `crates/flamewm-lock/src/wayland.rs:758-788` — `poll_outcome()` consumes the single verdict.
  A disconnected receiver (the PAM worker exited without a verdict) resolves to `Unavailable`,
  forces a repaint, and sets `unlock` only from `state.finish()`.
- `crates/flamewm-lock/src/wayland.rs:994-1005` — an explicit `Unavailable` verdict keeps the cover
  up with `Status::Unavailable`.
- `crates/flamewm-lock/src/wayland.rs:1007-1023` (step 1) — the absent-worker branch is faulted by
  dropping the sender before `poll_outcome()`; the lock stays locked, the status becomes
  `Unavailable`, and the repaint flag is set. No `Unlocked` verdict is fabricated.
- `crates/flamewm-lock/src/wayland.rs:739-754` and
  `crates/flamewm-lock/src/run/input.rs:77-90` — worker spawn failure also resolves to
  `Unavailable`; neither path has an injected fault yet.

## Counted problems

1. `authenticate()` had no injection seam, so an absent PAM backend could only be asserted at
   `DynamicPam::load_from`, never through the real entry point used by both presenters.
2. The `PAM_USER_UNKNOWN` branch was covered only by implication from the `PAM_AUTH_ERR` case; an
   account that may not log in had no dedicated proof that it stays locked even when the account
   double accepts it.
3. `poll_outcome()` handled a dead PAM worker with `Outcome::Unavailable`, but that branch had no
   test until step 1; a regression could leave the screen in `Authenticating` or fabricate an unlock.
4. Worker spawn failure (`wayland.rs:746-753`, `run/input.rs:87-89`) has neither an injection seam
   nor a fault test, so the "no worker at all" path is unverified.
5. The pure gates (`fail_safe::decide()`, `state::finish()`) are covered by local tests, but the
   chain from a missing backend to those gates needed explicit end-to-end cases for each branch.

## Target design (typed facade remastering)

`pam::Outcome` and `LockState::finish()` remain the only authorities that may release the lock.
`authenticate()` stays the public facade; `authenticate_with(names, ...)` is an additive typed seam
whose `&[&str]` parameter is the only injected fault. All failure mapping stays in `pam.rs`, behind
the same `PamSession` trait used by the mock doubles. Fault tests inject only data: library names,
PAM return codes, and a dropped channel. No test touches process state, a real PAM library, a
display, or a new dependency. Visuals are untouched: tests assert only the verdict, the status
token and the repaint flag; no colour, geometry or pixel value is changed, and no gate is loosened.

## Steps (each compiles alone)

1. **Implemented (step 1, UNBUILT).** Add the `authenticate_with` seam in `pam.rs` and the
   absent-backend and unknown-user fault tests in the same file; add the disconnected-worker fault
   test in `wayland.rs`. All items are private or additive, and use only imports already present in
   the modules, so this step compiles alone.
2. Add a private typed `spawn_authenticator` closure seam to `LockClient::submit()`
   (`wayland.rs:739-746`) and a fault test that injects a spawn failure, asserting `Unavailable`,
   a covered surface and no unlock. No pixel path changes.
3. Mirror the same seam into the X11 path (`run/input.rs:77-90`) and add its spawn-failure fault
   test; the `Unavailable` send at `:88` stays the sole failure verdict.
4. Add one table-driven test over every `run_flow` branch (start, denial, unknown user, other
   authentication error, account failure, absent library), pairing each injected outcome with
   `LockState::finish()` and asserting that none releases the lock. Keep the named step 1 tests as
   the per-branch cases.
5. Thread the absent-PAM fault into `fail_safe::Faults::pam` in the presentation path with a typed
   status token. `decide()` keeps `unlock: false`; no layout or colour value changes.

## Ported reference (path + choice)

- Path: `.vendor/lightdm-main/src/session-child.c:303-307` (`pam_start` failure logs and returns
  `EXIT_FAILURE`, so authentication never proceeds), `:378-382` (account management runs only
  after `pam_authenticate` succeeded), and `:388-397` (a successful PAM result still fails when
  the user does not exist).
- Path: `.vendor/lightdm-main/src/greeter.c:746-760` (a session request is ignored unless the
  authentication session is authenticated; the caller is told it is not authorized).
- Path: `.vendor/sddm-develop/src/helper/backend/PamHandle.cpp:147-159` (`start()` returns false
  on any non-success `pam_start`) and `:85-93` (`authenticate()` returns true only for
  `PAM_SUCCESS`).
- Choice ported: the exact-success-only verdict and the fail-closed start rule. FlameWM keeps the
  cover up and reports `Unavailable` or `Denied` instead of terminating the session, so the port is
  the decision rule, not upstream code, process exit, layout or dependencies. The reference
  material is read-only evidence and is never a build dependency or runtime input.

## Rollback

Remove the `wayland.rs:1007-1023` test. If the shared-checkout PAM seam is also rolled back,
restore `pam.rs:152-154` to call `DynamicPam::load_from(&LIBRARY_NAMES)` inline and delete
`pam.rs:156-177` and `:541-568`. No other source file, Cargo manifest, public signature or pixel
value is touched by step 1, so this restores the prior behaviour exactly. Later steps revert
independently in reverse order.

## Verification limit

UNBUILT by handoff: no `cargo`, test run, or runtime observation was performed. The fault-injection
assertions are written but unexecuted.
