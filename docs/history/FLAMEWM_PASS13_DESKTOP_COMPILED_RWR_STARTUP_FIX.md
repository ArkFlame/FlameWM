# FlameWM 0.1.0 Pass 13 — desktop RWR startup reliability

## Observed failure

The user's **native** `./xephyr` build reached step 4 of 5 and `flamewm-desktop` quit with `Error: "no node with id 'desktop-convert-wheel'"`. This log demonstrates the old desktop-core API errors have stopped, but does not prove *why* the embedded RWR was obsolete. The source HTML contains `desktop-convert-wheel` and all eight `convert-row-*` nodes. The Render compiler walks ordinary `section` descendants; the baseline `flamewm-desktop/build.rs` validates only text children, not these popup nodes. The prior Cargo source-content preflight invalidated `flamewm-desktop-core` only, **not** the desktop crate that produces and embeds `flamewm-desktop.rwr`. Old archive mtimes are a plausible cause; this remains an inference until verified on the live checkout.

## Repair in canonical owners

1. `crates/flamewm-desktop/ui_contract.rs` is the single desktop compiled UI node inventory shared by the build script and executable. It includes the popup root, Convert action, eight target buttons, and all first-paint/transient roots. Tests include a negative fixture that removes the exact failing `desktop-convert-wheel` ID.
2. `crates/flamewm-desktop/build.rs` validates these IDs on **compiled nodes and the decoded round-trip of the exact encoded RWR**, and checks each required ID is unique. A missing wheel fails the build with its ID rather than allowing Xephyr to fail on startup. A new `rerun-if-changed` includes the shared contract.
3. `crates/flamewm-ui-x11/src/template.rs` offers a read-only, allocation-free `UiDocument::has_node` check; no feature-local renderer bypass. `flamewm-desktop/src/main.rs` validates the **actual embedded decoded document** immediately after loading it, before layer assignment or painting. Includes a Rust embedded-document regression test.
4. `tools/ensure_desktop_conversion_cargo.py --include-ui` compares source-content hashes of the desktop UI compiler inputs and build script, and performs **only** `cargo clean -p flamewm-desktop --release` (or corresponding dev profile) when the content fingerprint has not been invalidated. It continues the existing desktop-core cache check. No full `cargo clean`, and a second unchanged run makes no additional changes.
5. `scripts/xephyr` and `scripts/check` opt into the UI gate for their respective profiles. The existing Cargo path metadata gate stays in place; source validation precedes all clean commands.

## Local verification

Run on the user's **complete working checkout**; do not replace concurrent edits:

```bash
python3 -I -m unittest discover -s tools -p test_desktop_conversion_cargo_gate.py -q
python3 -I tools/verify_desktop_conversion_contract.py
CARGO_BUILD_JOBS=2 cargo check -p flamewm-desktop --all-targets
RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test -p flamewm-desktop
CARGO_BUILD_JOBS=2 cargo build --release -p flamewm-desktop -p flamewm-wm -p flamewm-shell -p flamewm-settings -p flamewm-x11-canary
./xephyr
```

For X11 isolation choose an unused `FLAMEWM_XEPHYR_DISPLAY=:NN`, retain all `.target/flamewm/<run>/` logs. Required native gate: desktop remains alive and the Convert popup is usable; confirm no `no node with id` error, then exercise workspace-switch context menu dismissal. None of these native gates were executed in the archive-only environment.

**Important limitations:** This export lacks `assets/`, which `flamewm-desktop/build.rs` requires (`assets/raster/wallpaper.ppm`, `assets/raster/watermark.png`, `assets/wallpapers/fitz-roy-dawn.webp`, and `assets/web/breeze/`). The tool environment has no `cargo`/`rustc` and no `Xephyr`; therefore Rust compilation, RWR binary inspection, runtime screenshot and native menu acceptance are explicitly **UNVERIFIED**. Existing UI contract warnings are unrelated to this change. No Wayland backend is claimed.

## Safe integration into the live checkout

Use the accompanying `pass12_to_pass13` patch with `git apply --check` first. If other agents have changed any of its paths, reconcile the hunks manually: never overwrite the live Git tree wholesale or discard untracked/dirty files. To avoid old timestamp/cache traps, the new Xephyr preflight will detect the missing UI invalidation marker on its first run and rebuild only the desktop's release outputs. New conversion functionality is not introduced here; this pass closes the build/embedded-UI consistency gap.
