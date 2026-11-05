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
