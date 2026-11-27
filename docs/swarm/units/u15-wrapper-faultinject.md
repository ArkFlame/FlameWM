# u15 — Session wrapper fallback-chain fault injection

**Status: UNBUILT.** This slice was not compiled, executed or committed: the worker contract forbids
`git`, `cargo`, builds and test execution. The orchestrator runs `bash -n` on the two wrappers and the
new script after the batch lands. No production behavior changed in this slice.

Owner evidence: `scripts/flamewm-session-wrapper` (`chain_record`, `chain_cause`, `chain_supervisor`,
`run_safe_mode`) and `scripts/flamewm-wayland-session` (the same chain plus the `drm` and `nested`
stages and the crash-loop guard) already implement the fallback chain and its
`FLAMEWM-SESSION-CHAIN stage=... status=... cause=... code=... blast_radius=works:...|degraded:...`
records. Both wrappers expose the hooks needed for injection — `FLAMEWM_WAYLAND_BINARY`,
`FLAMEWM_SESSION_BINARY`, `FLAMEWM_SAFE_WM`, `FLAMEWM_SAFE_TERMINAL`, `FLAMEWM_SERVICES_BINARY`, the
timeout/guard knobs and `FLAMEWM_SESSION_ATTEMPT` — so no smallest env override was necessary.

Routes loaded: `flamewm-source-first`, `flamewm-parallel-execution`, `no-adhoc-scripts`. Owned paths:
`scripts/test-session-fallbacks`, `scripts/flamewm-session-wrapper`,
`scripts/flamewm-wayland-session`, this document. Only the new script was added; the wrappers were
read, not edited.

## Test design

`scripts/test-session-fallbacks` copies both wrappers into a `mktemp -d` sandbox, prefixes `PATH`
with stub binaries there, and gives each case its own `HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` and
`XDG_RUNTIME_DIR`. The temp tree is removed by an `EXIT` trap. The compositor stub fails the DRM
capability gate and exits 9 for the nested backend; the X11 session stub exits 7; the safe-mode WM
and terminal stubs exit 0 and the terminal writes a marker proving safe mode ran. `ldd`, `id`,
`Xvfb`, `systemctl`, `dbus-update-activation-environment` and `dbus-run-session` are stubbed so no
host tool, bus, seat or real session is reached. A stub `id -u` returns a UID whose `/run/user/<uid>`
does not exist, so an unset `XDG_RUNTIME_DIR` cannot recover on the test host.

| Case | Injection | Assertion |
| --- | --- | --- |
| `missing-runtime-dir` | `XDG_RUNTIME_DIR` unset | wayland log has `stage=runtime status=fail`; DRM record cause contains `runtime_dir` |
| `drm-fail` | stub reports `drm_compiled=false` | DRM record has `stage`, `cause` (`capability_gate`) and `blast_radius` |
| `nested-fail` | nested stub exits 9 | nested record has all three fields and a `stage_exit` cause |
| `x11-fail` | X11 stub exits 7 | X11 record has all three fields and a `session_exit` cause |
| `unwritable-log-dir` | `XDG_CACHE_HOME` under a regular file | no chain log is created; the same records are asserted on stderr |
| `crash-loop` | guard state seeded at 2 attempts, max 1 | `stage=guard ... cause=crash_loop` record; no stage launch; safe mode still runs |

Every case also asserts exit 0 (no stage's non-zero code ends the chain early), the safe-mode marker
and, except for the guard case, one `status=fail` record for each of `drm`, `nested` and `x11` plus
`stage=safe status=ok`. The script prints `SUMMARY checks=<n> passed=<n> failed=<n>` and exits
non-zero on any failed check.

## Verification

`bash -n scripts/test-session-fallbacks scripts/flamewm-session-wrapper
scripts/flamewm-wayland-session` — syntax only, as required by the brief. The script was not
executed; its runtime behavior remains unverified until the orchestrator runs it in a sandbox.

## Follow-ups

The orchestrator may run `bash scripts/test-session-fallbacks` (it never starts a real session) to
turn this unit from UNBUILT to verified. No Rust code, manifest or public API was touched.
