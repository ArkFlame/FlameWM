# LOG u74-unwrap-audit-apps

Area: Reliability audit of application crates — explorer, terminal, task-manager, shell.
Scope: non-test runtime code only (test modules and `*tests.rs` excluded), `unwrap()`/`expect()`/
`panic!`/`unreachable!`/`todo!`, plus byte-slice and fixed-array indexing on hot paths.

Method: `rg` over the four crates, then an `awk` pass that stops at the first `#[cfg(test)]`
so only shipped code was reviewed. Panic sites were checked against their callers instead of
being assumed unreachable. No cargo, no git, no absolute perf numbers.

## Panics removed (kept behaviour identical on the success path)

- `flamewm-shell/src/icon_loader.rs`: `IconLoader::spawn` is now fallible (`Option<Self>`).
  Wake pipe creation, `set_nonblocking` and worker-thread spawn no longer `expect`/panic; an
  fd or thread-exhaustion failure disables icons and the desktop keeps running.
- `flamewm-shell/src/app/icons.rs`: `icon_wake_fd` returns `Option<RawFd>`; `ensure_loader`
  stores the fallible spawn result.
- `flamewm-shell/src/main.rs`: the reactor registers the icon wake FD only when it exists;
  no invalid FD is registered.
- `flamewm-shell/src/launch_feedback.rs`: removed `visible().expect("checked above")`, which
  was only guaranteed inside the `concealed` branch. A missing sequence now hides the overlay
  and returns the expiry deadline, as the top-of-function path already does.
- `flamewm-explorer/src/trash.rs`: `unique_destination` no longer ends in `unreachable!`;
  the search is bounded to `u32::MAX` and falls back to a process-unique candidate that
  `restore` reports as an ordinary `io::Error` if it still collides.
- `flamewm-explorer/src/jobs.rs`: `Conflict::Duplicate` in the non-duplicate branch no longer
  `unreachable!`s; it reuses `duplicate_destination`, which is what the caller would do.
- `flamewm-terminal/src/themes.rs`: `builtin_themes` skips a malformed table row instead of
  panicking at startup; `store.rs` no longer indexes `builtin_themes()[0]` but uses `first()`
  with a real error.
- `flamewm-shell` projection code: 16 `write!(String).expect(...)` sites became
  `let _ = write!(...)`; the writes stay allocation-identical and the panic surface is gone.

## Checked, left unchanged

- `terminal/parser.rs` byte slicing: indices are advanced only over ASCII bytes; safe.
- `terminal/links.rs`, `screen.rs` run printing, `view.rs` band extension: bounds are derived
  from `len`/`cols` invariants and checked before indexing.
- `task-manager` `snapshot.cores[i]`, `hits[index]`, explorer menu slot arrays: loops are
  bounded by the same slices/constants that size the arrays.
- No `assert!`/`panic!` remains in non-test code of these crates.

## Composition expectation

No CPU/GPU/RAM delta: these are cold failure paths. The change removes whole-session loss
(panic aborting the shell/explorer/terminal) on resource exhaustion and malformed state, i.e.
it protects the reliability slice without adding work to any per-frame path.
