# u118-shutdown-paths

Area: shutdown and restart of explorer, shell, task manager and terminal — worker threads,
child processes, staging output, and state writes at exit.

Audit result per app (current source, this checkout):

- Task manager (`crates/flamewm-task-manager/src/lib.rs`): already complete. The window loop
  returns, `running` is cleared, `SampleGate::wake()` releases a sampler parked on the condvar,
  and the sampler is joined. The loop's own sleeps are bounded at one second. No change needed.
- Shell (`crates/flamewm-shell/src/main.rs`): the reactor loop returns, then the polkit and
  bluetooth agents unregister. The quick-controls helper child is reaped by
  `Drop for QuickControlSupervisor` (best-effort Shutdown line, kill, wait). No staging files,
  no runtime sockets, no state file is written at exit. No change needed.
- Explorer and terminal carried two defects; both are fixed below.

## Changes

- `crates/flamewm-terminal/src/pty.rs`: closing a terminal window killed the shell with
  `Child::kill()` (SIGKILL) and only then let the PTY master drop. A shell saves nothing from
  SIGKILL, so `$HISTFILE` (and any other shell-owned state written on hangup) was lost on every
  window close. Teardown now sends an explicit SIGHUP, polls `try_wait` for up to 250 ms while
  the child exits, and only SIGKILLs a child that ignored the hangup; the child is always reaped
  so no zombie is left in a still-running terminal. This is the Terminator sequence and its
  documented reason (`.vendor/terminator-master/terminatorlib/terminal.py`, `_wait_for_shell_exit`:
  the kernel's post-`close(master)` SIGHUP arrives as a second terminating signal and bash 5.2+
  aborts a history flush that it interrupts). `Drop for PtyProcess` now always goes through that
  path, so it reaps an already-exited child instead of leaking it. The 5 ms poll interval makes a
  normal shell exit cost one poll, not the full grace.
- `crates/flamewm-explorer/src/main.rs`: `close()` (cancel the running transfer, wait for its
  staging registry to drain, sweep staging entries) ran only after a successful event loop,
  because the loop call ended in `?`. A projector failure therefore exited through
  `std::process::exit(1)` with staging output still on disk. The loop result is now stored, the
  teardown runs on every path, and the stored result is returned.
- `crates/flamewm-explorer/src/main.rs`, `crates/flamewm-explorer/src/app/lifecycle.rs`: removed
  the `closing` field. It was written in `close()` and never read; teardown state is already the
  cancel token plus the staging registry.

State-write audit (no change needed): explorer preferences are written by `save_prefs` on the
user's toggle and by the resize commit only, each through one atomic temp-file rename;
`save_remote_recent` runs when a remote location is used, not per frame. Terminal settings are
written only when the value differs (`ThemeStore::save` from the preferences path, the settings
cache only on a changed snapshot). Neither app rewrites state on exit, so exit cannot double-save.

## Expected composition shift

Shutdown, not steady state: one bounded 250 ms worst case (5 ms typical) added per closed tab
whose child ignores the hangup, in exchange for zero SIGKILLed interactive shells, hence no lost
shell history and no orphaned zombie after a tab close. Explorer cleanup now runs before every
process exit, so a failed window loop no longer leaves staging directories in the destination.
The removed field drops one word from `ExplorerApp`, and with it one dead write on every close.

No `cargo`/`git` was run in this shared checkout. Compile correctness verified by reading the
current call paths; the rustix APIs used (`process::Pid::from_child`, `process::Signal::HUP`,
`process::kill_process`) are in the workspace's rustix 1.1.4 features (`std`, `process`).
