# Build Times

Measurements for the workspace dev build and the local lint gate. Follows `docs/BUILD_TIME_2026-11-05.md`, which holds the earlier investigation (duplicate-version causes, linker survey, test-harness notes). This file records what was measured after that.

## Environment and rules

- Host: 12 cores, 38 GiB RAM, shared with other workers. The cargo lock `/tmp/opencode/flamewm-terra-cargo.lock` was held by other jobs for most of the run, so wall times include queueing and are not clean.
- Rules: `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=2`, all cargo runs under the shared flock.
- Toolchains: system `rustc 1.93.x` for dev builds; Rust `1.89.0` (`/run/media/linsaftw/SDD/rust189/bin`) for the lint gate, matching CI.
- Linkers: only `ld.bfd`. `mold`, `lld` and `clang` are not installed.

## Cold dev build (`cargo build --workspace --timings`)

| Item | Value |
|---|---|
| Command | `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-timing cargo build --workspace --timings` |
| Result | exit 0, `Finished dev profile in 16m 04s` (wall, includes lock wait) |
| Report | `/run/media/linsaftw/SDD/target-timing/cargo-timings/cargo-timing.html` (433 units) |

Slowest units (unit duration, under contention):

| # | Unit | Seconds |
|---|---|---|
| 1 | `syn 2.0.119` | 156.5 |
| 2 | `flamewm-wm-wayland 0.1.7` | 62.8 |
| 3 | `x11rb-protocol 0.13.2` | 62.1 |
| 4 | `smithay 0.7.0` | 57.3 |
| 5 | `usvg 0.48.1` | 53.9 |
| 6 | `syn 3.0.5` | 53.4 |
| 7 | `thiserror-impl 2.0.21` | 42.0 |
| 8 | `wayland-protocols 0.32.13` | 36.9 |
| 9 | `rustix 1.1.4` | 36.3 |
| 10 | `winit 0.30.13` | 35.1 |

Critical path: `syn 2.0.119` is the single longest unit and sits at the head of the chain for the proc-macro crates. The full chain was not extracted from the report.

## Duplicate versions (`cargo tree -d`)

Duplicates: `calloop` 0.13/0.14, `x11rb` 0.13/0.14, `rustix` 0.38/1.1, `syn` 2/3, `thiserror` 1/2, `getrandom` 0.3/0.4, `hashbrown` 0.15/0.17, `miniz_oxide` 0.8/0.9, `linux-raw-sys` 0.4/0.12, `block-buffer`, `crypto-common`, `digest`, `cpufeatures`.

Every duplicate traces back to `smithay 0.7` or `winit 0.30` (via `flamewm-wm-wayland`), or to `fontdue`/`png` for `hashbrown` and `miniz_oxide`. Unifying them needs upstream upgrades, so no `Cargo.lock` change was made.

## Changes in this pass

- `scripts/lint` (rewritten): runs the CI gates in CI order: architecture guard and its unit tests, `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`. Toolchain: `FLAMEWM_TOOLCHAIN_BIN`, else rustup 1.89.0, else system with a warning.
- `CONTRIBUTING.md`: points at `scripts/lint`.
- `.githooks/pre-push`: stays light (fmt and guard), notes `scripts/lint` for the full gate.

No `Cargo.toml`, profile, linker or `.cargo/config.toml` changes. Reasons: duplicates need upstream upgrades; no faster linker is installed and a committed linker path would be host-specific; the profiles are already tuned in `Cargo.toml`.

## Not measured

- Incremental rebuild times and profile experiments (`opt-level` for deps, `split-debuginfo`). They were not run in this pass, so no before/after claim is made.
- Dependency audit: a text-based scan found no unused non-dev dependencies. Dev-dependencies were not scanned.

## Lint gate status

`scripts/lint` ran on the shared tree. The architecture guard passed (76 ratified violations, 0 new). `cargo fmt --check` failed on other workers' uncommitted files, so the run stopped before `check` and `clippy`. Clean-tree verification is recorded below once run.

## sccache evaluation (2026-11-08): not adopted

Setup: sccache 0.10.0 (official release, sha256 verified) in `~/.cargo/bin`, toolchain 1.89.0, private `CARGO_TARGET_DIR` per run, `cargo build -p flamewm-wm-wayland`, shared flock, `CARGO_BUILD_JOBS=2`.

| Run | Wrapper | Cargo `Finished` time | Cache hits / misses |
|---|---|---|---|
| plain cold | none | 4m 47s | n/a |
| sccache cold (empty cache) | sccache | 4m 51s | 0 / 175 |
| sccache warm (new target dir, populated cache) | sccache | 7m 18s | 0 / 175 |

- Cold overhead of sccache versus plain: about +1%, so no win. The threshold to adopt was >30%.
- The warm run got no cache hits. Cause not identified; the cache size did not grow on the second run. Under contention the warm time is not usable.
- Decision: `scripts/gate` does not export `RUSTC_WRAPPER`. Revisit only with a warm run that shows real hits.
- Measurement used cargo's own `Finished` time; wall times include flock queueing and are not comparable.
