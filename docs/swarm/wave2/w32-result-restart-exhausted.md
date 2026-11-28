# w32: result when the session restart budget is exhausted

Status: UNBUILT (audit + one small notice edit; no build or test run in this checkout)

## Question

With the default supervisor policy (5 restarts per 60 s, 500 ms to 8 s backoff), what does the
user see after the budget is exhausted: a visible result or a black screen?

## Findings (file:line)

- `crates/flamewm-session-core/src/lib.rs:598-607` — default `SupervisorPolicy`: 5 restarts in a
  60 s window, 500 ms initial and 8 s maximum backoff. `:688-690` sets `gave_up` and returns
  `SupervisorDecision::GiveUp` once `window_restarts >= maximum_restarts`.
- `crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:201-217` — the `GiveUp` arm is not a
  bare exit: `abort_recovery` (`:17-47`) builds a `FailureRecord` with code
  `FailureCode::RestartLimit` and `fallback_taken = true`, `:211` appends it to the session log,
  `:212-215` logs "restart budget exhausted; starting the fallback instead", and `:216` replaces
  the crashed child with a fallback plan. No `Err` is returned, so the session keeps running.
- `crates/flamewm-wm/src/session_diagnostics.rs:77-97` — `log` writes to stderr and the session
  log file; supervised children's stdout/stderr are redirected into that same file. The notice
  line is therefore persisted and visible to a display manager capturing stderr.
- `crates/flamewm-session-core/src/diagnostics.rs:342-364` (`fallback_script`) — the replacement
  runs `/bin/sh -c`: paints a flat `#1e1e1e` root background **if** `xsetroot` exists, echoes the
  reason to stderr (session log), shows an `xmessage -center` notice **if** `xmessage` exists and
  the role allows a dialog, then `exec sleep 2147483647` to hold the supervisor role slot.
  `:381-399` (`fallback_plan`) sets `dialog = true` for window manager and desktop, `false` for
  shell. No VT switch and no minimal window-manager mode exist on this path.
- `crates/flamewm-wm/src/session_diagnostics.rs:185-194` — preflight flags missing `xsetroot` and
  `xmessage` only as `Degraded` ("fallback notices go only to the session log"); exhaustion still
  proceeds when both are absent.
- `crates/flamewm-wm/src/bin/flamewm-session.rs:73-80` — `show_failure_dialog` is called only when
  `execute()` returns `Err`; a supervisor `GiveUp` never reaches it.
- Reference check: `.vendor/sddm-develop/src/helper/UserSession.cpp:48-50,148-160` ends a session
  when its process finishes and returns to the greeter; neither `.vendor/kwin-master/src` nor
  `.vendor/smithay-master` contains a restart-exhaustion notice policy. There is no vendored
  precedent for a VT or minimal-mode handoff, so no such mechanism was introduced.

## Answer

Not a black screen by design: the user normally gets a flat dark root background plus an
`xmessage` notice for WM/desktop, a typed `RestartLimit` record, and a persisted log line, while
the supervisor keeps the role alive on a sleeping fallback. The real gaps are (1) the on-screen
notice said "is missing or failed", so crash-loop exhaustion was indistinguishable from a
missing binary; (2) the visible notice and background are best-effort — without `xmessage` there
is no on-screen text at all and without `xsetroot` the old root image stays; (3) the shell role
never gets a dialog; (4) no VT or minimal-WM handoff exists.

## One fix spec (implemented — small notice change with a unit test)

Pass the concrete give-up cause into the fallback notice instead of the generic missing-binary
wording, without changing any existing script geometry, colors, dialog placement, or sleep.

- `crates/flamewm-session-core/src/diagnostics.rs:342` — `fallback_script` now delegates to new
  `fallback_script_with_cause` (`:349`) with the original "is missing or failed" clause, keeping
  all existing callers and output byte-identical. New `fallback_plan_with_cause` (`:387`) mirrors
  `fallback_plan` (`:381`) with a cause clause.
- `crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:216-220` — the `GiveUp` arm now calls
  `fallback_plan_with_cause(..., "kept crashing and exhausted the restart budget")`.
- `crates/flamewm-session-core/src/diagnostics.rs:648-665` — unit test asserts the default script
  is unchanged, the exhausted script names the cause, drops the generic wording, and keeps
  `xsetroot -solid '#1e1e1e'`, `xmessage -center`, the plain-background sentence, and the sleep.

Not changed (documented residual risk): optional-tool dependency of the visible notice, shell
role dialog, VT/minimal-WM handoff. All edits are UNBUILT; no cargo or tests were run in this
shared checkout.
