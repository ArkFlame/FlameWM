# u19 — Panic-to-record guard adoption in the Wayland WM process entry

**Status: UNBUILT.** The edit in this slice was not compiled, tested, committed or pushed: the
worker contract forbids `git`, `cargo`, builds and tests. The orchestrator must run
`cargo build -p flamewm-wm-wayland` (or the workspace check) after the batch lands and flip this
marker to verified.

Scope: one production file, `crates/flamewm-wm-wayland/src/main.rs`, plus this unit file. No other
crate, module owner, `Cargo.toml`, architecture exception or binary was touched. The panic hook
installation at the top of `main()` is already present from `u19-wl-panic-hook.md` and was left
intact — this slice adds the opt-in panic boundary that turns the recorded panic into a degraded
session instead of a process exit.

## Signatures read before editing

From `crates/flamewm-session-core/src/diagnostics/failure.rs`:

- `install_panic_hook(log_path: impl Into<PathBuf>)` — idempotent, first path wins; appends one
  `FailureRecord` with code `panic` for every panic and chains to the previous hook. Already
  installed in `main()` with the resolved session log path; not duplicated here.
- `guard<F, T>(subsystem: impl Into<String>, operation: F) -> Result<T, FailureRecord> where F:
  FnOnce() -> T` — opt-in unwind boundary. It sets the thread-local subsystem used by the hook,
  catches the panic and restores the outer name on every path, so the hook's record and the
  guard's `Err` describe the same subsystem.
- `FailureRecord { stage, code, cause, blast_radius, fallback_taken, timestamp }` — the `cause` and
  `stage` fields are read for the stderr degrade message. `to_log_line()` and `append_record()` are
  deliberately **not** called by the adopter: the installed hook already wrote the `code=panic`
  line, so a second append would double-record the same panic.
- `RestartPolicy` / `RestartDecision` — read and left to their owner. Startup panics here degrade
  in place rather than respawning a subsystem, so no crash-loop policy was introduced.

From `crates/flamewm-wm-wayland/src/main.rs`:

- `control::install(state: &mut State) -> Result<(), Box<dyn Error>>` (`control/mod.rs:206`).
- `xwayland::spawn_xwayland(dh: &DisplayHandle, handle: &LoopHandle<'static, State>) ->
  Result<Client, Box<dyn Error>>` (`xwayland.rs:325-328`).
- The `debug_outputs`, `virtual_output`, `profile` and `cursor` helpers and `report_render_acceleration`
  are unit-returning startup calls with no existing panic boundary.

## Change

One small helper, `guard_startup<T>(subsystem: &'static str, operation: impl FnOnce() -> T) ->
Option<T>` (`main.rs:705`), calls `guard` and maps `Err(FailureRecord)` to `None` after printing
`flamewm-wayland: <subsystem> panicked (<cause>); continuing without it`. The hook has already
appended the machine-readable record; the helper only makes the degrade visible on stderr. Callers
that ignore the result bind it to `let _` so the `#[must_use]` `Option` stays explicit.

The wrapped non-essential startup subsystems, all inside `run()`:

| Subsystem | Call | Degrade on panic |
| --- | --- | --- |
| `render-acceleration` | `report_render_acceleration()` | skip the render-acceleration report |
| `cursor-theme` | `cursor::saved_appearance()` + `export_session_theme` | skip exporting the theme |
| `profiling` | `profile::init()` + `install_compose_dump` | run without profiling/dump support |
| `session-protocols` | `state.init_session_lock()` + `init_screencopy()` | run without those extra globals |
| `control` | `control::install(&mut state)` | same as an `Err`: compositor runs without the shell API |
| `debug-outputs` | `debug_outputs::install_from_env(&mut state)` | skip debug outputs |
| `virtual-outputs` | `virtual_output::install_from_env(&mut state)` | skip extra headless outputs |
| `xwayland` | `xwayland::spawn_xwayland(&xwayland_dh, &handle)` | same as a spawn `Err`: report `stage=xwayland status=fail`, clear autostart, continue without X11 clients |

The event loop core is intentionally **not** wrapped: `EventLoop::try_new`, `Display::new`, the
socket and dispatch source insertion, `event_loop.run(None, ...)` (`main.rs:1004`) and backend init
keep their existing fatal behaviour, since there is no session to degrade to if they panic. The
existing `Err` paths and stage-line reporting are unchanged.

## Verification owed by the orchestrator

- `cargo build -p flamewm-wm-wayland` (or the workspace check) to confirm the import, the `FnOnce`
  closures over `&mut state` / `&handle`, and the `Option<Result<_, _>>` match arms.
- A forced panic in e.g. `control::install` should append one `code=panic` line whose `stage` is the
  subsystem name, print the stderr degrade message, and leave the compositor running; a panic in
  `event_loop.run` should still end the process.

No build, test, `git` or `cargo` command was run under this contract.

PASS: `main()` installs the panic hook from the earlier slice and `run()` now wraps eight
non-essential startup subsystems in `guard` through one `guard_startup` helper, degrading on panic
without a duplicate record; the event loop core stays fatal, only `main.rs` and this unit file were
touched, and the edit is UNBUILT by contract, so compilation and the forced-panic log checks are
owed by the orchestrator.
