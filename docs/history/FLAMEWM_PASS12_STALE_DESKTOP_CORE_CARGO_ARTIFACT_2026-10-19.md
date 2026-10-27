# FlameWM 0.1.0 — Pass 12: repeated E0432/E0599/E0061 after successful source check

## Symptom and evidence

`./xephyr` prints the successful desktop Convert source-check hashes:

- `lib.rs` `3360f4fee867ac25…`
- `file_actions.rs` `972d9ce202907dac…`
- `conversion.rs` `efe6b9f2f4d58f99…`

and then `cargo build --release` still reports:

- `E0432` unresolved `flamewm_desktop_core::conversion`;
- `E0599` missing `EntryMenuAction::Convert`;
- `E0061` the older one-argument `entry_context_menu` API.

These source files **are already correct in Pass 11**. Identical errors after matching
hashes are not evidence that the source needs the same API patch again.
The Pass 11 archive stored these files with old MTIMEs: `lib.rs` and
`file_actions.rs` at 2026-10-18 14:01 UTC; `conversion.rs` at **1970-01-01**.
If the archive was extracted into a working tree containing newer Cargo build
artifacts, older file modification timestamps are a plausible mechanism for
Cargo reusing an old compiled `flamewm-desktop-core` rlib. **This is a hypothesis,
not yet confirmed by a native Cargo invocation**, since this delivery environment
has no rustc/cargo. An alternative is that a Cargo override/wrapper points at
another workspace or cached output.

## Actual Pass 12 fix

- New `tools/ensure_desktop_conversion_cargo.py`: verifies the complete source API;
  runs `cargo metadata --offline --no-deps` and rejects a different workspace,
  local package, lib.rs target or path dependency; fingerprints the three API
  source files plus both crate manifests by file **content**, not MTIME. On the
  first run, or when content changes, it calls `cargo clean -p
  flamewm-desktop-core --release`. This removes only that crate's release outputs
  via Cargo's configured target directory, not the entire build cache. One
  ignored fingerprint marker under `.target/flamewm` prevents repetition for
  unchanged source. `--force` allows one explicit re-invalidation.
- `scripts/xephyr` calls the new gate immediately before the release build.
- `scripts/check` calls the gate before `cargo check --workspace --all-targets`
  (`--profile dev`) and before the release build (`--profile release`).
- `tools/verify_desktop_conversion_contract.py` now accurately labels its
  earlier check as **source-only** instead of implying Cargo's artifact is valid.
- New test suite `tools/test_desktop_conversion_cargo_gate.py` verifies
  content-based first-run invalidation, idempotence, old MTIMEs, independent
  profiles, changed files, wrong Cargo resolver/workspace, failed clean, source
  mismatch, and metadata failure.

## Fix the EXISTING local checkout without overwriting any source

Extract the accompanying tiny `Pass12` cache repair kit to a separate `/tmp`
folder and execute its `repair-now.sh` from your FlameWM repo. It checks your
actual dependency graph, **forces** one clean of only the outdated desktop-core
package and runs a fresh release build of `flamewm-desktop`. It does not edit
repository source files. Then run `./xephyr` normally.

Alternatively, if your checkout already has this Pass 12 gate:

```sh
python3 -I tools/ensure_desktop_conversion_cargo.py --force
CARGO_BUILD_JOBS=2 cargo build --release -p flamewm-desktop
./xephyr
```

If the compiler still emits E0432/E0599/E0061 **after the package-scoped clean**,
retain exact build logs and run `cargo build -vv --release -p flamewm-desktop`
so the actual rustc `--extern` path and wrappers can be inspected. Don't repeat
blind source patches, remove unrelated Cargo artifacts, or overwrite a live
checkout with this source archive.

## Verification and limitations

- `python3 -I -m unittest discover -s tools -p 'test_desktop_conversion_cargo_gate.py' -q`
  => 10 tests PASS (mocked Cargo metadata/clean, not Rust compilation).
- Source-only conversion contract continues to PASS.
- The actual local user's Cargo build and Xephyr runtime remain **unverified**
  here because Cargo, rustc and the complete checkout assets are unavailable.
- This fixes an observed source-vs-build inconsistency by invalidating only the
  relevant Cargo cache; a successful native compile still must be observed.

- Standalone kit was also exercised end-to-end with a fake Cargo executable:
  source check → `cargo metadata` → `cargo clean -p flamewm-desktop-core --release`
  → `cargo build --release -p flamewm-desktop` succeeded. This **does not**
  establish that a real Rust compiler accepted the project. The `python3 -I`
  execution mode is covered by an explicit unit test to avoid isolated-import
  regressions in the Xephyr harness.
