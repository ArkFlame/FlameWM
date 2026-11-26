# u125 CI parity

Area: build/loop tooling. Which `ci.yml` gates the perf loop reproduces, and the cheapest local
command for each one it misses. No product code touched; no runtime composition change.

Composition before (build time spent per batch, by gate): `scripts/perf-loop check` was the only
gate the loop ran, and it is one `cargo check --workspace --all-targets` at **default features**.
That covered CI step 6 of 7 and nothing else. Formatting, `clippy -D warnings`, the architecture
guard (and its unit test) and `--all-features` compilation were discovered only after a push, so a
lint-only or feature-only break cost a second full CI round instead of one warm local pass. The
feature gap is concrete: `flamewm-wm-wayland/drm`, `flamewm-integrations-linux/pulse-audio` and
`flamewm-ui-x11/test-support` are `#[cfg(feature = ...)]` code that the default-feature check never
compiles, while CI's clippy step compiles them all.

Composition after (same batch, no extra compile): unchanged for runtime; for the gate, one warm
all-features lint pass replaces a cold CI round. `scripts/gate <crate>` (guard, fmt, clippy
`--all-targets -D warnings`, then tests for the touched crates, all under one cargo-lock slot) does
the per-slice work, and `scripts/lint` — already an exact 4-step mirror of `ci.yml` in CI order —
does the once-per-30-minutes gate. `scripts/perf-loop check` stays the sub-minute compile probe.

Changes:
1. New `docs/perf-loop/CI-PARITY.md`: full command inventory for `ci.yml`, `arch.yml`, `dist.yml`
   and `release.yml` (including the fact that CI runs no `cargo test`); the gap table against
   `scripts/perf-loop check`; and a cost-ordered local-equivalent table naming the scripts that
   already exist, plus the release-only checks (makepkg/`--printsrcinfo`, `build-deb`, `build-cpak`,
   `sha256sum`, tag↔version) that must never run inside the loop.
2. Recommendation recorded, not implemented (owned paths are docs only): per slice `scripts/gate
   <crate>`; at the 30-minute gate `scripts/lint` instead of `scripts/perf-loop check`; keep
   `scripts/perf-loop check` as a compile-only probe and never read its green as CI parity.

Evidence: read `.github/workflows/{ci,arch,dist,release}.yml`, `scripts/perf-loop`,
`scripts/lint`, `scripts/check`, `scripts/gate`, `scripts/check-cargo`, `scripts/dev-check.sh`,
`scripts/build-arch`, `scripts/dist-checksums`, `crates/*/Cargo.toml` `[features]` sections.
No command was executed (unit forbids cargo/git); every claim is a static reading of those files.
