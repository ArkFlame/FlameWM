# Unit u36c — DRM fatal-init fallback fault-injection test

**Status: UNBUILT.** Static source edits only in a shared checkout. No `git`, `cargo`, build, test,
Python, `ctx_execute` or sleep loop was run here; the orchestrator builds, tests and commits after
the batch lands.

Route evidence: workspace version `0.1.7` was read directly from `Cargo.toml` (`scripts/version`
invokes Python, so it was not executed). Read `AGENTS.md`, `.opencode/HARNESS.md`,
`.opencode/ROUTES.md`, `.agents/README.md`, `.agents/CONTRACTS.md`, `.agents/JOBS.md` and the u36
audit (`docs/swarm/units/u36-drm-fallback.md`). Routed skills: `flamewm-source-first` (current
source, callers and error paths first) and `flamewm-rust-systems` (pure ownership of the state
transition, error and teardown boundaries). Shared checkout: no branch, stash, reset, rebase or
commit operation; only `main.rs`, the new test file and this report were touched.

## Scope and method

Read the fallback chain in `crates/flamewm-wm-wayland/src/main.rs`: `select_backend` (backend
choice), `run` (native attempt and fallthrough), `start_native` (`BackendInitError`),
`start_nested_or_headless` (winit attempt and headless tail), `stage_line` /
`record_backend_failure` (typed `FailureRecord` append) and `fill_backend_stages`. Read
`crates/flamewm-session-core/src/diagnostics/failure.rs` for `FailureRecord::at_now`,
`append_record` and the `to_log_line` key=value contract, and the crate manifest to confirm there
is no `tempfile` dev-dependency and that the crate denies unsafe code. Source and current call
paths outrank the u36 prose: the decision was still inline in `run`, so it was extracted.

## Extracted pure decision

`BackendFallbackOutcome` (`NativeOk`, `NativeFailed`, `NestedOk`, `NestedFailed`) is the input enum;
`NextBackend` (`Keep`, `Nested`, `Headless`) is the output enum. `next_backend` is a total, pure
mapping with no I/O and no state mutation:

| outcome in | next backend out |
| --- | --- |
| `NativeOk` | `Keep` — the tty backend is running |
| `NativeFailed` | `Nested` — try the winit window |
| `NestedOk` | `Keep` — the nested backend is running |
| `NestedFailed` | `Headless` — keep the socket-only compositor alive |

Production is routed through the same mapping: `run` asks the rule what to do after
`NativeFailed`, and `start_nested_or_headless` asks it what to do after `NestedFailed`. The emitted
stage lines, error messages, record writes and `state.backend` teardown are byte-for-byte the
actions of u36; the rule adds no new I/O and no new early exit. `record_backend_failure` now
delegates to `append_backend_failure_record(path, ...)`, the same record construction with an
injected log path, so the write can be fault-injected without changing the production path or
touching session environment variables (edition 2024 forbids unsafe `set_var`, and the crate denies
unsafe code).

## Added tests — `crates/flamewm-wm-wayland/src/backend_fallback_tests.rs`

Declared as `#[cfg(test)] mod backend_fallback_tests;` in `main.rs` only after the file existed.

| test | fault injected | assertion |
| --- | --- | --- |
| `native_ok_keeps_the_native_backend` | none (`NativeOk`) | next step is `Keep`; no fallback |
| `native_fails_nested_ok_keeps_the_nested_backend` | `NativeFailed`, then `NestedOk` | first step is `Nested`, final step is `Keep` |
| `native_fails_nested_fails_goes_headless` | `NativeFailed`, then `NestedFailed` | first step is `Nested`, final step is `Headless` |
| `each_failure_appends_a_record` | two failures appended to a pid-unique temp log | two lines, both `code=subsystem_error` and `fallback_taken=true`, with `stage="drm"` and `stage="nested_winit"` |

The first three tests are pure enum assertions: no DRM device, seat, winit surface or event loop.
The fourth exercises the real `FailureRecord::at_now` / `append_record` write against an injected
path and cleans up its file.

## Verification

Not run by this unit (see Status). Static read-back confirmed the new module declaration follows
the existing `mod` block, the pure mapping is used at both production decision points, the record
helper has the same body as the u36 helper with only the path parameter added, and no existing test
module name was reused. The orchestrator should run the crate test suite with and without the
`drm` feature after the batch lands; the three source edits are marked UNBUILT.

PARTIAL — the inline native → nested → headless decision is now a small pure enum-to-enum function
with four fault-injection tests covering native success, native failure with nested success, native
failure with nested failure and the failure-record append, while production keeps the same stage
lines, records and teardown. Compilation and test execution were explicitly out of scope, so this
slice is UNBUILT and reported as PARTIAL rather than PASS.
