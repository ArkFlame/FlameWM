# Build Time Investigation 2026-11-05

Scope: why `cargo check --workspace` and `cargo test --workspace` are slow on this host. No Rust source behaviour changed.

## Environment

- Host: 12 cores, 38 GiB RAM, load average 17-23 during all runs (other workers building concurrently). Absolute numbers are inflated; use ratios.
- rustc/cargo 1.93.1. Linkers: only `ld.bfd` 2.46. `mold`, `ld.lld`, `lld`, `clang`, `sccache`, `gold` are not installed.
- Rules in force: `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=2`, private `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-buildtime`.
- Worktree: `/run/media/linsaftw/SDD/wt-buildtime` at origin/main `6d0caa60`.

## Measurements

| Scenario | Command | Result | Log |
|---|---|---|---|
| Cold (empty target) | `cargo check --workspace --all-targets --timings` | exit 0, 878 s wall | `.agents/runtime/buildtime/cold_check.log` |
| Incremental leaf | touch `crates/flamewm-screenshot/src/ui.rs`, `cargo check -p flamewm-screenshot --all-targets` | exit 0, 19 s | `.agents/runtime/buildtime/inc_leaf.log` |
| Incremental core | touch `crates/flamewm-render-core/src/lib.rs`, `cargo check --workspace --all-targets` | exit 0, 71 s (21 units re-checked) | `.agents/runtime/buildtime/inc_core.log` |

Timing data: `target-buildtime/cargo-timings/cargo-timing.html` (511 units).

### Cold build analysis

- Sum of unit durations is 1750 s over 878 s wall, about 2.0x. The build is throughput-bound by `CARGO_BUILD_JOBS=2`, not by a long serial chain. The graph's longest single unit is 125 s (`syn`).
- Top units by duration (wall, under load): `syn 2.0.119` 125 s, `syn 3.0.5` 89 s, `x11rb-protocol 0.13.2` 71 s, `tracing-attributes 0.1.31` 51 s, `thiserror-impl 2.0.21` 50 s, `smithay 0.7.0` 42 s, `thiserror-impl 1.0.69` 38 s, `tiny-skia 0.12.0` 33 s, `wayland-protocols 0.32.13` 30 s, `usvg 0.48.1` 30 s.
- Workspace units total 391 s across 208 units. Largest: `flamewm-image-core` 27 s, `flamewm-render-core` 26 s, `flamewm-render-x11` 16 s (check-test), `flamewm-render-compiler` 13 s.
- Duplicate versions (`cargo tree -d`): `syn` 2/3, `x11rb` 0.13/0.14, `rustix` 0.38/1.1, `calloop` 0.13/0.14, `thiserror` 1/2, `getrandom` 0.3/0.4, `hashbrown` 0.15/0.17, `miniz_oxide` 0.8/0.9, `linux-raw-sys` 0.4/0.12. Every duplicate is pulled in by `smithay 0.7` or `winit 0.30` through `flamewm-wm-wayland`. Removing them needs upgrading those dependencies, which is out of scope here. `syn` 2 and 3 alone cost about 214 s of unit time.
- Shared external subgraph: `resvg`, `usvg`, `tiny-skia`, `fontdue`, `png`, `image-webp` reach about 25 workspace crates through `flamewm-render-core` and `flamewm-image-core`. Built once, so this costs cold time only.
- Dependency features are already trimmed (`default-features = false` on resvg, zip, gif, zune-jpeg, bzip2, chrono, rustix, x11rb, smithay).
- Profiles already tuned: `[profile.dev] debug = "line-tables-only"`, `[profile.dev.package."*"] debug = 0`, `[profile.dev.build-override] opt-level = 2`.

### Why the workspace is slow

1. Cold build cost is dominated by about 290 external crates and the duplicated `syn`/`thiserror`/`x11rb` stacks. None is fixable by a Cargo or config edit alone.
2. `CARGO_BUILD_JOBS=2` is the largest single factor. A default-jobs build on the same host would use all 12 cores.
3. Incremental cost is driven by fan-out. A touch to `flamewm-render-core` re-checks 21 units (71 s). `scripts/dev-check.sh` shows its reverse-dependency closure is 20 crates.
4. Test cost, not measured here: `flamewm-render-x11` builds 4 integration-test binaries plus a bin and an example, each linking the full crate. Four test files spawn a private `Xvfb`: `flamewm-clipboard/tests/two_instances.rs:30`, `flamewm-render-x11/tests/xdnd_source_gone.rs:46`, `foreign_event_livelock.rs:41`, `xdnd_two_windows.rs:49`. Fixed sleeps exist in `flamewm-integrations-linux/tests/pulse_stress.rs` (100-400 ms) and `flamewm-render-x11/tests/xdnd_source_gone.rs:226` (800 ms).

## Changes made

- `scripts/dev-check.sh` (new): maps `git diff` (vs `origin/main`, plus working tree and untracked files) to changed workspace crates, adds reverse dependencies, runs `cargo check -p ... --all-targets` (optional tests with `DEV_CHECK_TESTS=1`), runs the architecture guard. Root `Cargo.toml`, `Cargo.lock`, or `.cargo/**` changes fall back to the full workspace check.
  - Verified: `bash -n` passes. With no crate changes it prints the no-crate path and exits 0. With a probe in `flamewm-render-core` it maps to 20 crates and exits 0 (`devcheck_probe.log`).
- No Cargo.toml, profile, linker, or test-harness edits. Reason: no measured win was obtained. Dedupe needs dependency upgrades; no faster linker is installed; profiles are already tuned. Unmeasured gains were not claimed.

## Tests

| Crate | Command | Exit | Result |
|---|---|---|---|
| flamewm-ribbons | `cargo test -p flamewm-ribbons` | 0 | all test binaries ok |
| flamewm-desktop-core | `cargo test -p flamewm-desktop-core` | 0 | 65 passed |
| flamewm-control-core | `cargo test -p flamewm-control-core` | 101 | 12 passed, 1 failed: `settings_control_transaction_persists_only_after_native_shortcut_commit` (`tests/platform_control.rs:433`, `fontBold` is `false`, expected `true`) |

The `flamewm-control-core` failure is in a test file this change did not touch. Cause not investigated (suspect host-state or ordering; not proven). It is recorded as an unrelated failure.

## Guard

`python3 -I tools/architecture_guard.py` exit 0 (77 ratified violations, 3 warnings).

## Next steps (not done)

1. Replace `Xvfb` spawns with a shared fixture per binary, or move the four Xvfb tests behind a feature that CI still runs. Coverage must be kept.
2. Consolidate the four `flamewm-render-x11` integration-test binaries into fewer binaries. Each one links the full crate.
3. Upgrade `smithay` and `winit` together when a release removes the `syn`/`x11rb`/`rustix`/`calloop` duplicates.
4. Measure a default-jobs cold build on an idle host to set expectations for local dev.
