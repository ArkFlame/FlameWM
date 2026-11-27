# Fault-injection test index

**Status: UNBUILT.** The index script and this document were written by static reading only.
No `git`, `cargo`, build, test, Python or sleep loop was run in this unit. The orchestrator owns
compilation, test execution and the commit after the batch lands.

## Inventory

Discovery started from `rg -l 'fault' crates --glob '*tests*.rs'`. That bare substring also
matches `default`, so it returns 88 mostly unrelated files; the list below is curated to the
fault-injection modules added this session (standalone `fault`, `fault_tests`, or a documented
fault-injection intent). `backend_fallback_tests.rs` (u36c) and `load_tests.rs` (u34) are included
even though their file names have no `fault`; both declare a documented fault-injection intent.

| Module (file:line of `mod` declaration) | Package | Cargo filter | Fault injected | Unit |
| --- | --- | --- | --- | --- |
| `crates/flamewm-render-x11/src/accel/mod.rs:3` -> `src/accel/fault_tests.rs` | `flamewm-render-x11` | `fault_tests` | missing helper binary, helper exit at start, mid-session timeout/death; hardware -> software -> CPU chain | u09c |
| `crates/flamewm-platform/src/settings.rs:258` -> `src/settings/load_tests.rs` | `flamewm-platform` | `load_tests` | corrupt settings file quarantined, missing file, legacy keys, atomic persist/load round-trip | u34 |
| `crates/flamewm-wm-x11/src/wm/mod.rs:593` -> `src/wm/startup_fault_tests.rs` | `flamewm-wm-x11` | `startup_fault_tests` | no extensions, no RandR, second WM on root `SubstructureRedirect` | u45 |
| `crates/flamewm-wm-wayland/src/main.rs:121` -> `src/backend_fallback_tests.rs` | `flamewm-wm-wayland` | `backend_fallback_tests` | native DRM init failure, nested winit init failure, failure-record append | u36c |
| `scripts/test-wrapper-faults:14` (shell script, no cargo) | n/a | n/a | missing-library `ldd` preflight vs all-resolved | u15b |

The Wayland suite is invoked twice, with default features and with `--features drm`, because u36c
requires both feature states to be checked.

## Running

```sh
bash scripts/run-fault-tests --list   # inventory only; no build, test or git
bash scripts/run-fault-tests          # orchestrator only: runs cargo test per module, then the wrapper test
bash scripts/test-wrapper-faults      # standalone shell fault test
```

The runner sets `CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=2`, calls `cargo test -p <package> --
<filter>` sequentially (one Cargo owner at a time), and finishes with
`bash scripts/test-wrapper-faults`. It exits 0 only when every case passes and prints a
`SUMMARY fault suites=<n> failed=<n>` tail. A missing module file is a hard failure in both
`--list` and run mode.

`scripts/test-wrapper-faults` already existed from u15b; it was read, not edited. Its two cases
shadow `ldd` with a fake executable in a `mktemp -d` sandbox and source the wrapper's preflight
functions without running the wrapper's main tail. No session, compositor, real `ldd`, network or
build is involved.

## Evidence and precedent

Routes loaded: `flamewm-source-first` (source, declarations and call paths before editing) and
`no-adhoc-scripts` (no Python or temporary scripts; shell inventory in a reusable script).
No vendor-specific harness precedent was needed: the local conventions in
`scripts/test-session-fallbacks` and `scripts/test-wrapper-faults` (documented usage, bounded
exit codes, `mktemp -d` isolation) were followed. `.vendor/**` was not used as a dependency or
runtime input.

Unverified here: `bash -n scripts/run-fault-tests` and `bash scripts/run-fault-tests --list` were
not executed because shell permission was denied in this session after a restart; the script was
checked by static read-back only. Unverified: every `cargo test` invocation and the `--features drm`
variant; no Cargo runner was allowed in this unit.

## Risks

Cargo filters are substring matches on test names: renaming a module requires updating the
`CASES` table and this document. The `--features drm` case needs the DRM development libraries on
the orchestrator host; if unavailable, that case fails while the other four can still pass. The
wrapper test asserts preflight behavior in-process only and does not drive the wrapper supervisor;
`scripts/test-session-fallbacks` covers that path.
