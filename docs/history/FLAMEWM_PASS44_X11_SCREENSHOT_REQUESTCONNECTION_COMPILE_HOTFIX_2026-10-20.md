# FlameWM Pass 44 — Screenshot X11 `RequestConnection` compilation repair

## Observed failure

Native `cargo check --workspace --all-targets` compiled FlameWM Shell after Pass 43 and failed on `flamewm-screenshot`:

```text
error[E0599]: no method named `maximum_request_bytes` found for struct `RustConnection<S>`
  --> crates/flamewm-screenshot/src/clipboard.rs:57:20
help: trait `RequestConnection` ... is implemented but not in scope
```

`maximum_request_bytes()` is supplied by the `x11rb::connection::RequestConnection` trait. The caller has a concrete `RustConnection`; the trait must be in scope for method lookup. Existing `crates/flamewm-x11-canary/src/scenarios/clipboard.rs` uses the same import pattern.

## Minimal edit

- `crates/flamewm-screenshot/src/clipboard.rs`: change `use x11rb::connection::Connection;` to `use x11rb::connection::{Connection, RequestConnection};`. No algorithm or protocol behavior changes.
- `tools/test_flamewm_screenshot_integration.py`: add a static regression that detects a call to `maximum_request_bytes()` without importing its defining trait. Confirmed RED before the import and GREEN after.
- This report only; all other source paths remain byte-identical to the Pass 43 input.

## Verification in this execution environment

- Confirmed the logged Rust E0599 diagnosis matches the shipped code and the equivalent import in the X11 canary.
- Demonstrated new test RED before fix and GREEN after.
- `python3 -m unittest tools.test_flamewm_screenshot_integration tools.test_flamewm_package_pipeline tools.test_package_source_archive tools.test_flamewm_pass35_xephyr_panel_bootstrap tools.test_flamewm_pass33_merge_contract tools.test_flamewm_pass30_helper_migration tools.test_flamewm_pass29_cargo_targets`: 39/39 PASS (22.632 seconds).
- Independent tar extraction / checksums: see final archive and in-session verification.
- **NOT RUN**: native Rust compilation (no `cargo`/`rustc` in this container), Debian production build, X11 screenshot interaction, Xephyr, SDDM.

## Install and real native verification

Extract the Pass 44 **source overlay** over an existing complete FlameWM checkout containing assets; **do not re-extract Pass 41, 42, or 43 afterward**:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS44_SCREENSHOT_X11_REQUESTCONNECTION_BUILD_FIX_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null &&
tar --touch -xzf "$ARCHIVE" -C . &&
CARGO_BUILD_JOBS=2 cargo check -p flamewm-screenshot --all-targets &&
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets &&
CARGO_BUILD_JOBS=2 cargo test -p flamewm-screenshot &&
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Run `./scripts/xephyr` only if the above succeeds. This archive **is not** a complete isolated source release: previous uploaded Pass 43 source omits `assets/` and `.vendor/`. No `.deb` is built or certified here. The previously reported nonblocking warnings remain unless addressed by another pass.
