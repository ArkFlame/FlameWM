# u15b — Wrapper missing-library preflight fault-injection test

**Status: VERIFIED (script level).** `bash -n scripts/test-wrapper-faults` and
`bash scripts/test-wrapper-faults` were both executed; the real output is below. No cargo build,
session launch, git operation or python was used in this unit. Only the new script and this document
are owned; `scripts/flamewm-session-wrapper` was read, not edited.

Owner evidence: `scripts/flamewm-session-wrapper` lines 222-273 — `session_preflight_blast`,
`session_preflight_record` and `preflight_libs`. `preflight_libs` calls `ldd "$bin"`, dedups the
`not found` lines with awk, writes one `FLAMEWM-SESSION-CHAIN stage=preflight status=fail
cause=missing_lib` record per library to `${XDG_STATE_HOME}/flamewm/session.log`, and returns
non-zero. With no `not found` line it returns 0 before writing anything.

Routes loaded: `flamewm-source-first`, `flamewm-runtime-evidence`, `no-adhoc-scripts`. Owned paths:
`scripts/test-wrapper-faults`, this document. No Rust source, manifest or public API changed.

## Test design

`scripts/test-wrapper-faults` uses a `mktemp -d` sandbox under `${TMPDIR:-/tmp}` with an `EXIT` trap.
It extracts the three preflight functions from the wrapper with awk (from
`session_preflight_blast()` to the closing brace of `preflight_libs()`) and sources them, so the
wrapper's main tail never runs. The timing/blast helpers (`chain_now_ms`, `chain_ms`, `chain_works`,
`chain_degraded`) are pinned stubs; they carry no behavior under test. A fake executable `fake-app`
is paired with a PATH-shadowed `ldd` stub that prints a preset file (`FAKE_LDD_OUT`), so no real
`ldd` or binary inspection happens. Each case gets a clean `XDG_STATE_HOME` and session log.

| Case | Injected ldd output | Assertions |
| --- | --- | --- |
| `missing-library` | `libfoo.so.1 => not found` | non-zero return; session log has a `stage=preflight status=fail cause=missing_lib` record with `detail=lib=libfoo.so.1` |
| `all-resolved` | `linux-vdso.so.1 (...)`, `libfoo.so.1 => /usr/lib/libfoo.so.1 (...)`, `libc.so.6 => ...` | zero return; session log absent or empty |

Each case prints `ok`/`FAIL` lines and a `PASS`/`FAIL` summary line; the script ends with
`SUMMARY cases=... passed=... failed=...` and exits non-zero on any failed case.

## Verification

`bash -n scripts/test-wrapper-faults` — syntax OK. `bash scripts/test-wrapper-faults` — real output:

```
CASE missing-library
  ok   non-zero return when a NEEDED library is unresolved
  ok   failure record naming libfoo.so.1 in session.log
PASS missing-library
CASE all-resolved
  ok   zero return when all NEEDED libraries resolve
  ok   no session log entry written
PASS all-resolved
SUMMARY cases=2 passed=2 failed=0
exit=0
```

## Follow-ups

The test exercises `preflight_libs` in-process only; it does not drive the wrapper's supervisor or
the `session_preflight_missing_libs` fallback branch, which remain covered by
`scripts/test-session-fallbacks`. Re-run `bash scripts/test-wrapper-faults` after any change to the
preflight functions. No build, commit or push was performed here; the orchestrator owns those steps.
