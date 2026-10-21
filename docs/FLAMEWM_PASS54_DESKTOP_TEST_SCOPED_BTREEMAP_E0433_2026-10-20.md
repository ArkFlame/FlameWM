# FlameWM Pass 54 — desktop test E0433 fix

## Source of the reported failure

`cargo check --workspace --all-targets` compiles `#[cfg(test)]` code in the desktop binary. The `work_area_tests` module in `crates/flamewm-desktop/src/main.rs` calls both `BTreeMap::new()` and `BTreeMap::from(...)` but does not import that type into its own module scope. `use std::collections::{BTreeMap, BTreeSet};` in the parent module is not automatically in scope in Rust child modules. As a result, the compiler reports two `E0433` errors.

## Repair

One line added inside `work_area_tests`: `use std::collections::BTreeMap;`. No production source logic, monitor layout, settings, screenshots, or packaging contracts changed. A focused static regression verifies the scoped import and original call sites.

## Installation

Extract **only Pass 54** over the complete local checkout; it includes all of the Pass 53 source overlay. Do **not** re-extract Pass 41 or any earlier source archive afterward. The overlay requires the preexisting runtime assets from the complete checkout and is not a standalone source release.

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS54_DESKTOP_TEST_BTREEMAP_E0433_FIX_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
  tar --touch -xzf "$ARCHIVE" -C . && \
  python3 tools/test_flamewm_pass54_btreemap_scope.py && \
  CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
  CARGO_BUILD_JOBS=2 cargo test -p flamewm-desktop && \
  CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

## Evidence / limit

The defect was identified from the user's native compiler output. The archive regression passes on patched source and fails on the prior Pass 53 source. Native Rust and graphical testing cannot be performed in this container; further errors, if any, require a new compiler log. Existing warnings are not compiler failures.
