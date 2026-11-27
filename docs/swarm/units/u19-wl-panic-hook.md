# u19 — Panic-to-record adoption in the Wayland and X11 WM binary mains

**Status: UNBUILT.** Neither edit was compiled, tested, committed nor pushed: the worker contract
forbids `git`, `cargo`, builds and tests. The orchestrator must run
`cargo build -p flamewm-wm-wayland -p flamewm-wm` (or the workspace check) after the batch lands and
flip this marker to verified.

Scope: two binary entries and this unit file. No library, module owner or `Cargo.toml` was changed.

- `crates/flamewm-wm-wayland/src/main.rs` — the `flamewm-wayland` binary named by `[[bin]]` in
  `crates/flamewm-wm-wayland/Cargo.toml` (`path = "src/main.rs"`). The hook is installed as the first
  statement of `main()`, before the startup timer, `tune_malloc()`, the logger, the `--capabilities` /
  `--probe` exits and `run()`, so it is active for the whole process.
- `crates/flamewm-wm/src/bin/flamewm.rs` — the X11 WM's `flamewm` binary named by `[[bin]]` in
  `crates/flamewm-wm/Cargo.toml`. `crates/flamewm-wm-x11` is a library-only crate (native X11
  authority) with no binary; its facade `flamewm_wm::entry::main()` is what this binary runs, matched
  by the shell's `flamewm_shell::entry::main()` pattern in u20. The hook is installed before the
  delegation, so it covers entry too. The separate `flamewm-session` supervisor binary was not
  touched.

## Dependency check (read, not changed)

- `crates/flamewm-wm-wayland/Cargo.toml` already declares
  `flamewm-session-core = { path = "../flamewm-session-core" }` (line 34) and
  `rustix = { workspace = true, features = ["process"] }` (line 53; the workspace definition adds
  `std`). Both were already direct dependencies, so no path dependency was added.
- `crates/flamewm-wm/Cargo.toml` already declares
  `flamewm-session-core = { path = "../flamewm-session-core" }` (line 15) and
  `rustix = { workspace = true, features = ["fs", "process", "std"] }` (line 21).
- No architecture-gate exception was needed: `docs/ARCHITECTURE_EXCEPTIONS.toml` has only an
  `x11rb-dependency` ratchet for `crates/flamewm-wm/Cargo.toml` (line 242), which this edit neither
  uses nor changes. The typed diagnostics owner is `flamewm-session-core`; no new cross-owner
  capability is introduced.

## Signatures read before editing

From `crates/flamewm-session-core/src/diagnostics/failure.rs`:

- `install_panic_hook(log_path: impl Into<PathBuf>)` — idempotent, first path wins; appends one
  `FailureRecord` with code `panic` per panic and calls the previously installed hook.
- `FailureRecord::to_log_line()` serializes the blast radius into the log line, so no extra record
  is written here.

From `crates/flamewm-session-core/src/diagnostics.rs`:

- `session_log_path(xdg_state_home: Option<&OsStr>, home: Option<&Path>, fallback: &Path) -> PathBuf`.
- `fallback_log_path(uid: u32) -> PathBuf`.

`guard`, `RestartPolicy` and `append_record` were read and left to their owner: this unit adopts the
hook only, not a panic boundary or restart policy, so process behaviour is otherwise unchanged.

## Change

Both entries now resolve the same path the session uses and install the hook once, following
`crates/flamewm-shell/src/bin/flamewm-shell.rs`:

```rust
let fallback = fallback_log_path(getuid().as_raw());
let home = env::var_os("HOME").map(PathBuf::from);
install_panic_hook(session_log_path(
    env::var_os("XDG_STATE_HOME").as_deref(),
    home.as_deref(),
    &fallback,
));
```

The hook records and continues (it does not keep the process alive) and chains to the previous hook,
so stderr panic output is preserved. The Wayland compositor's stage lines, capability/probe exits and
error exits remain untouched; the X11 binary still delegates straight to `flamewm_wm::entry::main()`.

## Verification owed by the orchestrator

- `cargo build -p flamewm-wm-wayland -p flamewm-wm` to confirm the imports, types and features.
- A forced panic in each binary should append one `code=panic` line to
  `$XDG_STATE_HOME/flamewm/session.log` (or the fallback) while still showing the original panic
  message through the chained hook.

No build, test, `git` or `cargo` command was run under this contract.

PASS: both binary mains install `flamewm-session-core`'s panic hook with the resolved session log
path using only already-declared `flamewm-session-core` and `rustix` dependencies; no `Cargo.toml`,
library or unrelated binary was touched, and the edits are UNBUILT by contract, so compilation and
the forced-panic log checks are owed by the orchestrator.
