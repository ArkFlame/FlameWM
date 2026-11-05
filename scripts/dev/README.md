# Fast dev checks

Shared machine rule: one Cargo target dir for every worker, so dependencies compile once.

- `scripts/dev/check.sh <crate>...`: `cargo check --profile fast` for named crates.
- `scripts/dev/test-fast.sh <crate>... [-- filter]`: `cargo test --profile fast` for named crates.

Defaults set by `scripts/dev/env.sh` (override via env vars):

| Var | Default |
| --- | --- |
| `FLAME_TARGET_DIR` / `CARGO_TARGET_DIR` | `/run/media/linsaftw/SDD/target-shared` |
| `CARGO_BUILD_JOBS` | `2` |
| `RUST_TEST_THREADS` | `2` |

Commands run under `nice -n 10 ionice -c3`.

Profile `fast` (Cargo.toml): `opt-level = 1`, `debug` off, `incremental = true`, no LTO,
`codegen-units = 16`. `release` is unchanged.

## Build-time measurements (not adopted)

Measured on a loaded machine (load 16-20, 12 cores), private target dir, `CARGO_BUILD_JOBS=2`.
Incremental = touch `src/lib.rs`, then `cargo test -p <crate> --no-run`, steady-state run.

| Crate | Incremental |
| --- | --- |
| flamewm-render-compiler | 3.8s |
| flamewm-shell | 15.8s |
| flamewm-wm-wayland | 11.7s |

Rejected: `[[example]] test = false` for the three `flamewm-wm-wayland` probe examples. A/B on the
wm-wayland test rebuild: before 9.8s / 8.1s, after 11.0s / 9.4s. No win within noise, so reverted.
Duplicate crate versions (`cargo tree -d`) come from smithay's transitive deps and are not fixable here.
No mold/lld/sccache is installed; `rust-lld` is not in the rustup sysroot. Installing one needs a system
package, so it is not done.
Global `~/.cargo/config.toml` sets `incremental = false` and dev `debug = 0`; account for that when comparing timings.
