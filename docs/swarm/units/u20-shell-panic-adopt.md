# u20 — Panic-to-record adoption in flamewm-shell entry

**Status: UNBUILT.** The edit in this slice was not compiled, tested, committed or pushed: the
worker contract forbids `git`, `cargo`, builds and tests. The orchestrator must run
`cargo build -p flamewm-shell` (or the workspace check) after the batch lands and flip this
marker to verified.

Scope: one file, the shell binary entry `crates/flamewm-shell/src/bin/flamewm-shell.rs`, plus this
unit file. No other crate, module or `lib.rs` owner was touched. The binary entry is the one named
by `[[bin]]` in `crates/flamewm-shell/Cargo.toml` (`path = "src/bin/flamewm-shell.rs"`); it calls
`flamewm_shell::entry::main()`, which is the `pub fn main()` in `src/main.rs` re-exported as the
`entry` module. The hook is installed at the binary entry, before `entry::main()` starts any
subsystem, so it is active for the whole process and chains to the previously installed hook.

## Dependency check (read, not changed)

`crates/flamewm-shell/Cargo.toml` already lists both crates this edit needs; no dependency was
added:

- `flamewm-session-core = { path = "../flamewm-session-core" }` (line 27) — provides
  `diagnostics::install_panic_hook`, `session_log_path`, `fallback_log_path`.
- `rustix = { workspace = true }` (line 35) — provides `process::getuid` for the fallback path,
  matching the existing pattern in `crates/flamewm-wm/src/session_diagnostics.rs:40-45`.

## Signatures read before editing

From `crates/flamewm-session-core/src/diagnostics/failure.rs`:

- `install_panic_hook(log_path: impl Into<PathBuf>)` — idempotent, first path wins; appends one
  `FailureRecord` per panic with code `panic` and calls the previous hook.
- `FailureRecord::to_log_line()` — the hook's record serializes the blast radius as the
  `still_works="<...>" degraded="<...>"` fields, so the session-log line carries blast-radius text
  without any extra record from the shell.
- `guard(subsystem, closure) -> Result<T, FailureRecord>` and `RestartPolicy` — read and left to
  their owners; this unit only adopts the hook, not a panic boundary or restart policy, so process
  behaviour is unchanged.

From `crates/flamewm-session-core/src/diagnostics.rs`:

- `session_log_path(xdg_state_home: Option<&OsStr>, home: Option<&Path>, fallback: &Path) -> PathBuf`.
- `fallback_log_path(uid: u32) -> PathBuf`.

## Change

At the binary entry, resolve the same session log path the session uses and install the hook once
before delegating to `entry::main()`:

```rust
use std::env;
use std::path::PathBuf;

use flamewm_session_core::diagnostics::{fallback_log_path, install_panic_hook, session_log_path};
use rustix::process::getuid;

fn main() {
    let fallback = fallback_log_path(getuid().as_raw());
    let home = env::var_os("HOME").map(PathBuf::from);
    install_panic_hook(session_log_path(
        env::var_os("XDG_STATE_HOME").as_deref(),
        home.as_deref(),
        &fallback,
    ));
    flamewm_shell::entry::main();
}
```

No behaviour change otherwise: the hook records and continues (it does not keep the process
alive), chains to the previous hook so the normal panic output is preserved, and the shell's own
execution path is untouched.

## Verification owed by the orchestrator

- `cargo build -p flamewm-shell` to confirm the resolved imports and types.
- A forced panic in a shell subprocess should append one `code=panic` line with quoted
  `still_works`/`degraded` fields to `$XDG_STATE_HOME/flamewm/session.log` (or the fallback), then
  still show the original panic message through the chained hook.

No build, test, `git` or `cargo` command was run under this contract.

PASS: the panic hook is installed at the shell binary entry using only the already-declared
`flamewm-session-core` and `rustix` dependencies; it resolves the session log path and writes a
failure record whose log line carries blast-radius text, chains to the previous hook, and leaves
all other shell behaviour untouched. The edit is UNBUILT by contract, so compilation and the
forced-panic log check are owed by the orchestrator.
