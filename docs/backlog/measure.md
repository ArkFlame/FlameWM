# FlameWM Measurement Backlog

Discovery only. No product code changed. Base: origin/main `c5955ae3`. Build target: `/run/media/linsaftw/SDD/target-disc-measure`, `CARGO_BUILD_JOBS=2`, cargo runs under `flock /tmp/opencode/cargo.lock`. Host: 12 cores, shared with other workers, so timings are not clean.

## Measured metrics

| Metric | Value | Command |
|---|---|---|
| Clean dev workspace build | exit 0, 15m 29s | `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=$T cargo build --workspace --timings` |
| Fast-profile bins build | exit 0, 24m 56s | `cargo build --profile fast --workspace --bins` |
| Incremental rebuild (touch `flamewm-explorer/src/lib.rs`, no content change) | exit 0, 10 s | `touch crates/flamewm-explorer/src/lib.rs && cargo build --workspace` |
| Binary size, fast profile: flamewm-wayland / flamewm-desktop / flamewm-shell | 9.43 / 11.58 / 8.69 MB | `ls -l $T/fast/flamewm-*` |
| Idle RSS, 60 s, nested session: flamewm-wayland | 172.7 MB RSS, 50.2 MB anon, 109.8 MB PSS, 31 threads | `/tmp/claude-1000/disc-measure/idle60.sh $T/fast 60` |
| Idle RSS: Xwayland | 120.8 MB RSS, 61.3 MB PSS | same |
| Idle RSS: flamewm-desktop | 26.7 MB RSS, 16.8 MB PSS | same |
| Idle RSS: flamewm-shell | 21.4 MB RSS, 11.4 MB PSS | same |
| Idle CPU, all FlameWM processes (10 s window at 60 s) | 0.00% | same |
| Startup to first mapped / painted, explorer | 137.9 / 248.1 ms (median of 3) | `scripts/perf/startup.sh --bin-dir $T/fast --runs 3 --warmups 1 explorer` |
| Startup, terminal | 276.4 / 618.1 ms | same |
| Startup, imageviewer | 178.4 / 206.3 ms | same |
| Startup, archiver | 63.9 / 88.8 ms | same |
| Startup, settings | 1774.0 / 1843.5 ms | same |
| Startup, finder | 155.8 / 258.1 ms | same |
| Startup, task-manager | 168.0 / **None** (3 of 3 runs did not paint) | same |
| CPU frame batch, 60 fills + 20 strokes + 2 blurs, paint_batch | median 83.4 to 92.8 ms | `cargo run --profile fast -p flamewm-render-x11 --example pixel_bench` |
| CPU frame batch, per-op | median 89.8 to 108.7 ms | same |
| Blurs only (2 full-frame) | median 77.2 to 83.2 ms | same |
| Fills and strokes only | median 7.3 to 8.0 ms | same |
| Test wall time, all 116 test binaries, sequential, RUST_TEST_THREADS=2 | ~139 s total | per-binary `time` over `cargo test --workspace --no-run --message-format=json` |

Per-crate test wall (sum of binaries, seconds): flamewm-settings 47, flamewm-assistant-core 33, flamewm-integrations-linux 22, flamewm-lock 12, flamewm-ribbons 8, flamewm-image-core 7, flamewm-terminal 3, flamewm-render-x11 1, flamewm-render-core 1, flamewm-render-compiler 1.

Slowest clean-build units (dev, under contention, seconds): syn 2.0.119 111.5; x11rb-protocol 0.13.2 66.0; syn 3.0.5 62.5; x11rb-protocol 0.14.0 53.0; tracing-attributes 0.1.31 43.1; usvg 0.48.1 41.5; thiserror-impl 2.0.21 41.0; smithay 0.7.0 39.4; flamewm-render-compiler 0.1.7 34.5; wayland-protocols 0.32.13 32.4. Full timing report: `$T/cargo-timings/cargo-timing.html`.

Failing test binaries (exit 101):
- `flamewm-render-core` `layout_snapshot`: `layout_matches_golden_for_every_ui_page` FAILED. Cause: `ui/desktop/assets/wallpaper.ppm` and `ui/assets/branding/flamewm-wordmark.png` are missing in this worktree.
- `flamewm-render-compiler`: `utilities::tests::settings_hotkeys_document_is_unchanged_by_utility_migration` FAILED (80 passed, 1 failed, 1 ignored). Not investigated further.

## Ranked candidate improvements

Gains are estimates from the numbers above unless marked unmeasured.

1. **task-manager never paints.** Evidence: startup table, 3 of 3 runs, `painted` None. Gain: a correctness fix, not a speed gain. Verify with `startup.sh task-manager` and a runtime screenshot.
2. **Full-frame blur dominates the CPU frame.** Evidence: blurs 77 to 83 ms of an 83 to 93 ms batch. Gain: up to ~80 ms per frame on CPU if blur is removed or moved to the GPU path (estimate). The GPU path was not measured because `flamewm-gpu-helper` was not found.
3. **Settings startup is the outlier.** Evidence: 1774 ms mapped, 1844 ms painted, against 64 to 276 ms for the other apps. Gain: up to ~1.5 s per open (estimate). Not profiled yet.
4. **Idle RSS of flamewm-wayland and Xwayland.** Evidence: 173 MB and 121 MB RSS at idle, 31 threads on flamewm-wayland, 50 MB anon. Gain: unmeasured. Needs a heap or allocation profile before any target is set.
5. **Terminal paints about 340 ms after map.** Evidence: 276 ms mapped, 618 ms painted. Gain: up to ~340 ms per open (estimate). Not profiled yet.

Build-time items (lower priority, gain not measured): `syn` and `x11rb-protocol` each take 50 to 110 s as single units. `docs/BUILD_TIMES.md` already lists duplicate `syn` and `x11rb` versions caused by smithay and winit, so the fix needs upstream upgrades.

## Not measured

- Release binary sizes: the release profile uses fat LTO and was not built. Only fast-profile sizes are recorded.
- Slowest 10 individual tests: no per-test timing on stable. Per-binary wall time only.
- Incremental timing beyond one no-op touch: a content change to a deep crate was not timed.
- GPU frame cost: `flamewm-gpu-helper` missing.
- `raster_bench` (`flamewm-render-core`, `flamewm-ribbons`): ignored tests that require `--release`, not run.
- Idle wakeup counts: owned by the other worker (`docs/PERF_IDLE.md`, not on main yet).
- Competitor comparison: `docs/competitors/` marks footprint numbers as `unmeasured`, so no comparison is possible.
- Idle and startup numbers are from a fast-profile build on a shared 12-core machine, so they are not release-grade.
