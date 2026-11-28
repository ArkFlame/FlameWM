# w32: what the user sees when the supervisor restart budget is exhausted

## Answer

**A near-black screen, not a visible result.** After 5 restarts inside 60 s the supervisor returns
`GiveUp` and replaces the failed role with a fallback plan. The fallback sets a `#1e1e1e` root
background, writes the reason to the session log, then runs `sleep`. There is no VT switch, no
minimal window manager, and — for the window-manager role — **no on-screen message**. The typed
`restart_limit` record and one log line exist, but neither is visible without opening the log.
Desktop-role exhaustion does show an `xmessage`; window-manager and shell exhaustion do not.

## Gap analysis (source evidence)

| file:line | finding |
| --- | --- |
| `crates/flamewm-session-core/src/lib.rs:598-607` | Policy is exactly 5 restarts / 60 000 ms, backoff 500 ms -> 8 000 ms; graceful stop 1 500 ms. |
| `crates/flamewm-session-core/src/lib.rs:656-663`, `flamewm-session-core/src/.../failure.rs` tests | `gave_up()` / `restarts()` expose the exhausted state; the 6th exit in the window yields `SupervisorDecision::GiveUp` (proved by `crates/flamewm-wm/src/bin/flamewm-session/tests.rs:410-443`). |
| `crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:201-217` | On `GiveUp`: `abort_recovery(..., "restart budget exhausted; starting the fallback instead", GiveUp, true, now)`, `record_failure`, one `session_diagnostics::log` line, then `spawn(fallback_plan(...))?`. The function returns `Ok(())` and the loop continues — no error reaches `main()`. |
| `crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:49-55` | `record_failure` is best effort: with no writable log location the record is silently dropped. |
| `crates/flamewm-wm/src/bin/flamewm-session.rs:385-408` | Startup WM readiness failures take the same `GiveUp -> fallback_plan` path; startup exhaustion is equally silent. |
| `crates/flamewm-wm/src/bin/flamewm-session.rs:69-81`, `crates/flamewm-wm/src/session_diagnostics.rs:241-254` | `show_failure_dialog` (the only `xmessage` notice owned by the session binary) is called **only** when `execute()` returns `Err`. The `GiveUp` path never returns `Err`, so this dialog never fires for budget exhaustion. |
| `crates/flamewm-session-core/src/diagnostics.rs:363-382` (pre-fix 367) | `fallback_plan` chose `dialog = false` for `SessionRole::WindowManager`; only `Desktop` got `true`. |
| `crates/flamewm-session-core/src/diagnostics.rs:342-359` | Fallback script: `xsetroot -solid '#1e1e1e'`, `echo "$msg" >&2`, optional `xmessage -center "$msg" &`, `exec sleep 2147483647`. "Minimal mode" is a sleeping shell on a flat dark root — no WM and no recovery UI. |
| `crates/flamewm-wm/src/session_diagnostics.rs:185-194` | Preflight reports missing `xmessage` as `"fallback notices go only to the session log"`: the visible notice is conditional on `xmessage` being installed. |
| `crates/flamewm-session-core/src/diagnostics.rs:343-345` (pre-fix) | The shared notice text said "The window manager is running without it" even for the WM role, where no WM remains — misleading if shown. |

## One fix spec (implemented)

Make the window-manager fallback notice visible and truthful, matching the desktop fallback and the
Plasma precedent:

- `crates/flamewm-session-core/src/diagnostics.rs:367`: `SessionRole::WindowManager` fallback now
  requests `dialog = true`, so the same `fallback_script` emits
  `command -v xmessage ... && xmessage -center "$msg" ... &`.
- `crates/flamewm-session-core/src/diagnostics.rs:343-350`: for role `"window manager"` the second
  sentence becomes `"A plain background is shown instead."` instead of the false
  `"The window manager is running without it."` Other roles keep their exact text.
- `crates/flamewm-session-core/src/diagnostics.rs:338-340`: doc comment updated to match.
- `crates/flamewm-session-core/src/diagnostics.rs` tests: new
  `window_manager_fallback_shows_a_truthful_notice` asserts the WM fallback script contains
  `xmessage -center`, the truthful background sentence, and the log path.

This is a notice/record-path change only. No new dependency, no supervisor gate is loosened, and
normal WM rendering is untouched (pixel-identical outside the fallback path). The flat `#1e1e1e`
fallback background is unchanged; the only added pixels are the intentional failure dialog.

Reference precedent: `.vendor/LingmoOS-main/core32/startshell/startplasma.cpp:50-53` defines
`messageBox()` that logs and runs `xmessage`; lines 695 and 713 call it when the Plasma session
cannot start. Read-only reference evidence; not a dependency or runtime input.

## Status of edits

All four edits (3 production lines/regions + 1 unit test) are **UNBUILT** and **UNTESTED**. This
worker may not run `git`, `cargo`, or builds; the orchestrator runs checks after the batch lands.

Residual risks: `xmessage` must be installed for the notice to appear (otherwise log-only, as
preflight already reports); the backgrounded `xmessage` can outlive the session because `stop_role`
signals the shell, not the detached notice — identical to the existing desktop fallback behavior.
No VT/minimal-WM fallback was added; that would exceed a small notice change.
