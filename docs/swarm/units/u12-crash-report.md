# u12 — Crash boundary, subsystem restart and failure-report record

**Status: UNBUILT.** The edits in this slice were not compiled, tested, committed or pushed: the
worker contract forbids `git`, `cargo`, builds and tests. The orchestrator must run
`cargo test -p flamewm-session-core` after the batch lands and flip this marker to verified.

Owner evidence: `crates/flamewm-session-core/src/diagnostics.rs` already owns session diagnostics,
the resolved log path and `log_line`; the new code lives at
`crates/flamewm-session-core/src/diagnostics/failure.rs` and is re-exported from `diagnostics.rs`,
so no other crate or `lib.rs` owner was touched. Routes loaded: `flamewm-source-first`,
`flamewm-parallel-execution`, `flamewm-semantic-modularization`, `flamewm-rust-systems`.
No new dependencies, no unsafe code.
Crash-loop policy delegates to the existing `ProcessSupervisorPolicy` rather than creating a
second state machine. All decisions and serialization are pure; the only I/O boundary is
`append_record` and the only global side effect is the idempotent panic hook.

## Stable public API

Call path: `flamewm_session_core::diagnostics::{...}`.

| Item | Contract |
| --- | --- |
| `FailureCode` | `Panic`, `RestartLimit`, `SubsystemError`; `as_str() -> &'static str` yields `panic`, `restart_limit`, `subsystem_error`. Adding a variant must not change an existing string. |
| `BlastRadius { still_works: Vec<String>, degraded: Vec<String> }` | `new(into_iter, into_iter)`, `Default` (both empty). Entries are subsystem ids, not prose. |
| `FailureRecord { stage, code, cause, blast_radius, fallback_taken, timestamp }` | `new(stage, code, cause, blast_radius, fallback_taken, unix_seconds) -> Self`; `at_now(...)` reads the clock once; `to_log_line() -> String` is the wire format. `timestamp` is unix seconds (u64), injected in `new` so tests stay pure. |
| `unix_seconds_now() -> u64` | System clock helper for callers that build records themselves. |
| `append_record(path: &Path, record: &FailureRecord) -> io::Result<()>` | Appends one line to `path`, creating file and parent dir if needed; never rotates or truncates; never panics. |
| `install_panic_hook(log_path: impl Into<PathBuf>)` | Idempotent (first path wins). Records every panic with code `panic`, subsystem from the innermost active `guard` (else `process`), then calls the previous hook. It records and chains; it does not keep the process alive. |
| `guard<F, T>(subsystem: impl Into<String>, operation: F) -> Result<T, FailureRecord> where F: FnOnce() -> T` | Opt-in panic boundary: converts a panic into `Err(FailureRecord)` so the caller can fall back. Sets the thread-local subsystem for the installed hook, supports nesting, and restores the previous name on every path. |
| `RestartPolicy` | `new(maximum_restarts: u8, window_ms: u64)`, `Default` = 5 / 60 000 ms; `note_failure_at(Instant) -> RestartDecision`, `note_failure()` (wall clock), `restarts()`, `gave_up()`. Reuses session supervision semantics. |
| `RestartDecision` | `Restart { delay_ms }` or `GiveUp`; `GiveUp` is the signal to emit a `FailureCode::RestartLimit` record. |

Log contract: one physical line, keys in a fixed order, string values quoted with `\\`, `"`, tab,
CR, LF and other control characters escaped; list fields are comma-separated with embedded commas
escaped. Example:

```text
timestamp=42 stage="wm" code=restart_limit cause="line one\nline \"two\"" still_works="x11,shell" degraded="compositor" fallback_taken=true
```

Call pattern for adopters:

```rust
use flamewm_session_core::diagnostics::{append_record, guard, install_panic_hook,
    session_log_path, FailureCode, FailureRecord, BlastRadius};

install_panic_hook(session_log_path(xdg_state_home, home, &fallback));
let result = guard("shell", || execute());
if let Err(failure) = result {
    let _ = append_record(&log, &failure);
    // start fallback / skip this subsystem instead of exiting the session
}
let _ = append_record(&log, &FailureRecord::at_now("wm", FailureCode::RestartLimit,
    "restart limit reached", BlastRadius::new(["x11"], ["panel"]), true));
```

## Call-site adoption list

Lines below are the current checkout (they will shift after adoption). These are the existing
`exit` candidates on the two main paths; each should record a `FailureRecord` and return/give up
through `guard` and `RestartPolicy` before the process exits. No production `unwrap`/`expect` was
found in either `main.rs`; the `unwrap`/`expect` hits there and in startup helpers are test-only
and are listed separately for completeness.

| File:line | Candidate | Suggested adoption |
| --- | --- | --- |
| `crates/flamewm-shell/src/main.rs:743` | `std::process::exit(1)` in the `--quick-control-host` error branch | `guard("quick-control", || run_quick_control_host(...))`; on `Err` append the record with `fallback_taken=true`, then exit only if the helper cannot fall back. |
| `crates/flamewm-shell/src/main.rs:749` | `std::process::exit(1)` in the shell `execute()` error branch | `guard("shell", execute)`; append `Err` record with `blast_radius.still_works` = running components before exiting. |
| `crates/flamewm-shell/src/quick_controls/host/run.rs:291` | `std::process::exit(quick_control_exit_code(shutdown))` after helper shutdown | Wrap the shutdown run in `guard("quick-control-shutdown", ...)`; record `FailureCode::SubsystemError` on failure before the deliberate exit (the exit itself is intentional teardown). |
| `crates/flamewm-wm-wayland/src/main.rs:522` | `std::process::exit(probe::run())` for `--probe` | Record `FailureCode::SubsystemError` with stage `probe` when the probe reports FATAL; keep the diagnostic exit code. |
| `crates/flamewm-wm-wayland/src/main.rs:528` | `std::process::exit(code)` for a nonzero startup result | `guard("wayland-startup", run)`; on `Err` append the record and route through `RestartPolicy` instead of exiting immediately. |
| `crates/flamewm-wm-wayland/src/main.rs:533` | `std::process::exit(1)` for a startup `Err` | Same: append the guarded failure with `stage="wayland-startup"`; only give up after `RestartDecision::GiveUp`. |
| `crates/flamewm-shell/src/main.rs:1528` | test-only `expect("notification template")` | No production change; keep as a strict test fixture. |
| `crates/flamewm-wm-wayland/src/probe.rs:542`, `:562` | test-only `expect` | No production change. |
| `crates/flamewm-wm-wayland/src/startup.rs:297`, `:366`, `:369` | test-only `unwrap`/`expect` | No production change. |

Process entry for both binaries should call `install_panic_hook(session_log_path(...))` once before
subsystem threads start, so escaped panics are recorded with the subsystem name that `guard` set.

## Verification

Unrun by design: no `cargo`, `git`, builds or tests were executed. Compile and unit-test evidence
must be supplied by the orchestrator; the module's tests cover serialization/escaping, panic
capture, nested guard restoration, restart limits and healthy-window reset.
