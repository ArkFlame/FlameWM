# FlameWM 0.1.0 — Pass 30: E0601 durable source-layout correction

## Scope

Fix repeated `error[E0601]: main function not found in crate desktop_compat` while preserving Pass 29 application associations, XRandR refresh handling, and Brave/KWallet code. No behavior change to those integrations.

## Root cause and overlay failure

Cargo discovers Rust files directly under `crates/flamewm-wm/src/bin/` as independent executable targets unless that package's installed manifest disables auto-discovery. Pass 29 set `autobins = false`, but the distributed overlay still shipped the 239-line helper at the Cargo auto-binary path. A future manifest overwrite or partially applied overlay could expose that file again as a binary lacking `main`. The repeated E0601 is consistent with this state; without the user's live Cargo metadata, the precise overlay/manifest mismatch is not directly observed.

## Code changes

- Moved the complete original helper implementation to `crates/flamewm-wm/src/session_app_compat.rs`, outside `src/bin/`.
- `crates/flamewm-wm/src/bin/flamewm-session.rs` now explicitly includes the module with `#[path = "../session_app_compat.rs"] mod session_app_compat;` and uses its real functions.
- Kept `autobins = false` and exactly two explicit `[[bin]]` entries in `crates/flamewm-wm/Cargo.toml`.
- The source overlay intentionally overwrites any existing `src/bin/desktop_compat.rs` with a harmless compatibility stub containing `fn main() {}`; even if an old manifest briefly rediscovers this path, E0601 cannot occur. `scripts/apply-pass30-cargo-layout` then **deletes** that obsolete helper file safely and idempotently.
- Updated source-contract tests to inspect the new module path and added a migration regression covering an exact old 239-line helper without `main`, repeated application, and invalid-manifest fail-closed behavior.

## Correct installation over Pass 27, 28, or 29

Run from the full repository; do not re-extract older archives afterward.

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS30_RENAMED_HELPER_E0601_FIXED.tar.gz"
test -f "$ARCHIVE" || { echo "Missing Pass30 archive: $ARCHIVE"; exit 1; }
tar -tzf "$ARCHIVE" >/dev/null || exit 1
tar -xzf "$ARCHIVE" -C . || exit 1
./scripts/apply-pass30-cargo-layout || exit 1
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets &&
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Expected output from the migration:

```text
REMOVED obsolete src/bin/desktop_compat.rs (Cargo E0601 trigger)
PASS: Pass 30 cargo targets and session module layout validated
PASS: Cargo metadata only reports flamewm + flamewm-session
```

## Verification and limitations

- Targeted source-level regression suite including actual stale-helper cleanup and manifest mutation: passed in the packaging environment.
- Archive integrity and overlay onto Pass 29 baseline: checked.
- Native `cargo check --workspace --all-targets`: not run in packaging environment (no Rust toolchain). Must run on the target host; this report does not claim successful compilation.
- No runtime KDE/MIME/Brave/monitor behavior was exercised as part of this E0601 patch.
