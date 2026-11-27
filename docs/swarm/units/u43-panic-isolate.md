# u43 — Subsystem panic isolation: restart policy wiring audit

**Status: UNBUILT.** The edit below was not compiled, tested, committed or pushed: this worker
contract forbids `git`, `cargo`, builds and tests. The orchestrator must run
`cargo build -p flamewm-wm` (or the workspace check) plus the `flamewm-session` tests after the
batch lands and flip this marker to verified.

Scope: `crates/flamewm-session-core/src/diagnostics.rs`,
`crates/flamewm-session-core/src/diagnostics/failure.rs`,
`crates/flamewm-wm/src/bin/flamewm-session/supervision.rs` (the only file edited), plus this unit
file. Child-launch sites were located with `rg 'Command::new|spawn' crates/flamewm-session-core
crates/flamewm-wm/src`; no other crate, module or owner was touched.

## Policy read before editing (no change)

- `diagnostics/failure.rs:249-269` `guard` — opt-in panic boundary, converts a panic into
  `FailureRecord` and restores the outer subsystem name.
- `diagnostics/failure.rs:271-278` `RestartDecision` (`Restart { delay_ms }` / `GiveUp`) and
  `280-329` `RestartPolicy` — in-process crash-loop guard; `note_failure_at` delegates to
  `ProcessSupervisorPolicy::child_exited_at`, so both mechanisms share window and backoff.
- `diagnostics/failure.rs:226-243` `install_panic_hook` — logs panics, keeps the process alive only
  where a caller opts into `guard`.
- `session-core/src/lib.rs:588-709` — `SupervisorPolicy` (5 restarts / 60 s window, 500 ms → 8 s
  backoff, 1.5 s graceful stop), `ChildExit`, `SupervisorDecision`, and
  `ProcessSupervisorPolicy::child_exited_at`. The exit kind is deliberately ignored by the policy
  (`_exit`), so a normal desktop/shell exit is still restarted; this was left intact.

`guard` and `RestartPolicy` have no production call sites in this checkout (only their own tests),
so the live "panic → restart → session kept" path for subprocess subsystems is the supervisor's
`ProcessSupervisorPolicy`, audited below.

## Findings

| # | File:line | Gap | Change | Risk | Status |
|---|-----------|-----|--------|------|--------|
| 1 | `flamewm-wm/src/bin/flamewm-session/supervision.rs:135-142` (deleted arm at pre-edit `176-182`) | On `GiveUp` the WM arm (index 0) returned `Err`, so a WM crash loop ended the session (`main` then showed the failure dialog and `exit(1)`); only non-WM roles fell back. This contradicted the required give-up cap that leaves the session alive. | Removed the `if index != 0` guard from the fallback arm and deleted the index-0 `GiveUp => Err` arm. Every managed child now hits the same `restart budget exhausted; starting the fallback instead` path, including the WM. Two small edits. | A WM that crashes 5 times in 60 s is replaced by the `fallback_plan` sleeper (session, desktop and shell stay up) instead of terminating the session. No change on a clean WM exit: `clean_wm_exit` at `131-134` still returns `Ok(())` before the policy is consulted. The fallback WM does not set EWMH, so it is a degraded WM, not a working one. | UNBUILT |
| 2 | `supervision.rs:143-175` | Checked: `RestartAfter(delay)` stops dependents, waits the policy backoff, respawns and re-arms the same supervisor, then checks readiness/startup grace (WM, desktop, shell). Non-zero and signal exits both reach this arm via `status_to_exit` (`305-315`). | None — wiring is correct. | None — observation only. | UNBUILT (no edit) |
| 3 | `supervision.rs:12-43` | Checked: `start_optional` swaps in the fallback plan when the desktop/shell binary is missing, cannot be spawned, or exits during startup grace, so a panic at startup keeps the session alive instead of aborting. | None. | None — observation only. | UNBUILT (no edit) |
| 4 | `session_env.rs:274-302`, `session_app_compat.rs:536-575` | Checked: these `spawn` sites are one-shot commands (D-Bus/systemd activation, PAM KWallet handoff). A non-zero exit is logged/degraded, not ignored as a subsystem crash, and no session child depends on them staying alive. | None. | None — observation only. | UNBUILT (no edit) |
| 5 | `readiness.rs:203-212` with `flamewm-session.rs:200-201` | Residual: a WM that exits non-zero or by signal *before readiness* makes `wait_ready` return `Err`, which `execute()` propagates to `main` (failure dialog, `exit(1)`), bypassing the restart policy. Closing it needs `wait_ready` to report the exit kind plus a restart loop around initial readiness — larger than the three-edit budget and it would touch a second owner. | None. | A WM panic in the first readiness window still ends the session. | UNBUILT (no edit) |
| 6 | `supervision.rs:153,159,168` | Residual: respawns use `spawn(plan)?`. If the binary disappears after startup, the spawn failure aborts the session rather than falling back; feeding `ChildExit::SpawnFailed` back through the policy would need a fallback that skips `wait_ready` for the WM path. | None. | Rare (binary removed while the session runs). | UNBUILT (no edit) |

## Child classes covered

- **Window manager** — covered: startup spawn/readiness, post-startup non-zero and signal exits via
  `child_exited` → `RestartAfter` backoff → respawn, and now `GiveUp` → fallback (session alive).
- **Desktop** — covered: `start_optional` startup fallback, post-startup restart via the policy,
  dependent re-spawn after a WM restart, `GiveUp` → fallback.
- **Shell** — covered: same as desktop, including re-spawn after a desktop restart.
- **Lock screen, notification daemon, task manager** — not started by the session supervisor in
  either audited crate: there is no `Command::new`/`spawn` for them in
  `crates/flamewm-session-core` or `crates/flamewm-wm/src` (they live in `flamewm-lock`,
  `flamewm-integrations-linux` and `flamewm-task-manager`, or arrive through D-Bus activation).
  They are covered only indirectly as part of the managed desktop/shell child; their own internal
  restart wiring is outside this unit's scope and was not changed.

PARTIAL — The single in-scope abort path is fixed: a give-up now leaves the session alive for every
managed child, while clean exits (notably WM logout) and the existing backoff/cap behaviour are
unchanged. Two residual paths were audited and deliberately left alone: a WM exit before readiness
and a failed respawn still end the session, because fixing either needs a readiness/restart redesign
beyond the three-edit budget. Lock, notification daemon and task manager are not supervisor children
in these crates, so no direct coverage was claimed for them. All edits are UNBUILT: no `git`,
`cargo`, build or test was run under this contract; the orchestrator must verify the build and tests
after the batch lands.
