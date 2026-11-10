# FlameWM Measurement Backlog

> Rows: `meas-blur`, `meas-settings`, `meas-terminal`, `meas-build`, `meas-tm`, `meas-shadow`, `meas-blur-eq`, `meas-sign`, `meas-perf-blocked` in `docs/BACKLOG.md`.

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

## Cold settings paint after text-shadow cache (819a2284)

Baseline only. No product code changed. Binary: `/run/media/linsaftw/SDD/gate-targets/shadow-fast/fast/flamewm-settings` (prebuilt by the cache worker, commit not re-verified). Load average during runs: 13 to 19 on 12 cores.

| Metric | Value | Command |
|---|---|---|
| Settings startup, 5 timed runs + 1 warmup | mapped median 451.7 ms, painted median 463.0 ms; runs 419.7 to 486.6 ms; 0 failed | `scripts/perf/startup.sh --bin-dir <above> --runs 5 --warmups 1 settings` |
| Settings startup, 3 runs under `perf record` | not measured | `perf` blocked: `perf_event_paranoid=4`, "Failure to open any events" |

Cold-path breakdown (blur ~322 ms over 1186 misses, Xft glyph prefetch 110 to 290 ms) is the cache worker's timing-counter evidence, not re-measured here. The 1186 figure matches the 4 MiB eviction run in `text_shadow_cache.rs` docs, so the miss count on the 16 MiB cap is unverified.

Candidates checked against the code, none landed:
- Share one blur across colours (key without colour): not pixel-identical. The blur runs on RGBA, so the alpha quantisation `(coverage * a + 127) / 255` and per-channel unpremultiply round differently per colour.
- Dedupe glyph prefetch: already done. `xft/backend_mask.rs` `prefetch_text_masks` skips cached masks and uses a `seen` set keyed by (family, text, size, weight, pad).
- Skip blank runs: already done (`has_ink`). Skip zero-alpha colour: pixel-identical but rare, not measured.

Next step: add per-phase counters (mask, blur, premultiply, server upload) to `text_shadow_image` and run 5 cold settings starts. Gate a code change only after that shows one phase at >= 15% of the 463 ms painted median.

## Terminal sustained output per-frame cost (meas-terminal)

Measurement only. No product code changed. Base: origin/main `0f70161e`, `flamewm-terminal` fast profile, own target `/run/media/linsaftw/SDD/gate-targets/termframe-base`. Private `Xvfb :220` 1920x1080x24, no WM. Terminal run as `flamewm-terminal -e sh -c '<workload>'`; the terminal exits when its tab's child exits. Harness: `/tmp/claude-1000/termframe-bench.sh <bin> <label> <runs>` (not committed). Host: 12 cores, shared; load1 17 to 27 during runs, so timings are noisy.

| Workload | Runs | Wall (mean, sd) | Throughput | CPU% of terminal (mean, range) | VmHWM (kB) |
| --- | --- | --- | --- | --- | --- |
| `yes \| head -c 20000000` | 5 | 9.85 s, sd 0.43 | 2.0 MB/s, about 1.0 M lines/s | 119.6 (114.2 to 126.2) | 79 k to 89 k |
| `seq 1 200000` | 5 | 0.53 s (mostly startup) | not meaningful | 91 (85 to 98) | 43 k to 45 k |

Profiled run (`FLAMEWM_PROFILE=1`, one run, `yes \| head -c 60000000`, 9.6 s wake time; profiler adds overhead):

| Span | Count | Total wall | Share of wake | Note |
| --- | --- | --- | --- | --- |
| `terminal.wake` | 149 | 9609 ms | 100% | one dispatch per pacer wake |
| `terminal.parse` (`session.rs:56`) | 1 905 562 | 7464 ms | 78% | VT parser + `Screen::print`/scroll; 1.9 M feeds, about 31 bytes each |
| `render.draw` | 72 | 405 ms | 4.2% | 5.6 ms per frame avg, max 10.5 ms |
| `render.present` | 72 | 411 ms | 4.3% | includes draw |
| `terminal.project` | 149 | 29 ms | 0.3% | row dedupe already in `ui.rs` |
| `render.paint` / `shape` / `layout` | 72 each | 13 / 2 / 2 ms | <0.2% | |

Frames: the profiler shows 72 presented frames in 9.6 s, about 7.5 frames/s, about 83 ms of render per second. Frames per second without the profiler were not counted (no counter in the unprofiled build). Allocations per frame: not measured (no heaptrack, ltrace or counting allocator; `perf` blocked by `perf_event_paranoid=4`).

Findings:
- The render path is about 5% of sustained-output CPU (draw + present). No render-path change can beat the 5% bar.
- The largest bucket is `terminal.parse` at 78%. Per-byte parser and screen print cost, about 120 ns/byte, is the top hot spot. Without a CPU profile, no single line is proven to dominate, so no code change was made.
- Per-event overhead: the reader sends one `SessionEvent::Output` per PTY read, which averages about 31 bytes here, so the UI handles about 1.9 M events per 60 MB. The wake self time (2.1 s of 9.6 s) includes channel and event dispatch. Coalescing consecutive `Output` bytes per tab before `feed` is a candidate; it is output-identical because the parser is streaming, but unmeasured.
- Frame coalescing already exists: the pacer (`session.rs:107`) limits wakes to one per `FRAME`, and `OUTPUT_BUDGET` (256 KB) bounds each dispatch.

Next step: `perf` access or a sampling profiler build for `terminal.parse`, then measure the event-coalescing candidate with 5 runs before and after.

### Event coalescing measured (meas-termparse)

Result: below the 10% bar, code not kept. Base: origin/main `4dbfcda4`, `flamewm-terminal` fast profile, own target `/run/media/linsaftw/SDD/gate-targets/termparse-base`, Xvfb `:260`. Harness: `/tmp/claude-1000/termparse-bench.sh` (copy of `termframe-bench.sh` on `:260`, not committed), workload `yes | head -c 20000000`, 5 runs each.

Candidate: in `wake::drain_and_commit`, merge consecutive `SessionEvent::Output` for one tab into one `App::feed_output` call (one feed and one banner update per run, not per PTY read). Output-identical: the parser is streaming, and a regression test compared a grid fed byte-by-byte with one fed in one event (escape sequences split at every boundary; same grid and cursor).

| Build | Runs | Wall (mean, median, range) | Throughput (mean) | CPU% (mean) | load1 |
| --- | --- | --- | --- | --- | --- |
| base, first pass | 5 | 9.96 s, median 10.08 s | 2.0 MB/s | 113 | 13.3 to 13.8 |
| base, second pass (matched load) | 5 | 6.31 s, median 6.44 s (range 6.06 to 6.67) | 3.17 MB/s | 142 | 5.2 to 7.3 |
| coalesced | 5 | 5.81 s, median 5.97 s (range 5.26 to 6.36) | 3.44 MB/s | 147 | 5.4 to 5.9 |

Gain at matched load: +8.6% by mean, +2.6% by median, ranges overlap. Below the 10% bar, so the change was reverted and not pushed. The first base pass ran at load 13 and is not comparable to the coalesced run at load 5.5. Per-read `feed` overhead is therefore not the dominant cost; the remaining time is in the per-byte parser and screen print. Next: a sampling profile of `terminal.parse` (needs `perf` access or an external profiler) before any further parse change. Allocations per PTY read (`buffer[..count].to_vec()` in `session.rs` read loop) are still unmeasured.

## Terminal cold start re-measure (meas-terminal, origin/main be106c4d)

Measurement only. No product code changed. Base: origin/main `be106c4d`, `flamewm-terminal` fast profile, own target `/run/media/linsaftw/SDD/gate-targets/termstart-base`. Private `Xvfb :320` 1920x1080x24 with `-noreset` (copy of `scripts/perf/startup.sh` with `DNUM=320` and `-noreset`, not committed). Command: `startup.sh --bin-dir <fast> --runs 7 --warmups 1 terminal`. Load1 8.5 to 8.9 before and after the run (12 cores, shared).

| Run | Mapped (ms) | Painted (ms) | Map-to-paint (ms) |
| --- | --- | --- | --- |
| 1 | 130.1 | 155.9 | 25.8 |
| 2 | 115.9 | 140.2 | 24.3 |
| 3 | 146.9 | 179.5 | 32.6 |
| 4 | 181.3 | 215.1 | 33.8 |
| 5 | 117.7 | 134.9 | 17.2 |
| 6 | 99.6 | 116.8 | 17.2 |
| 7 | 100.1 | 105.4 | 5.3 |
| Median | 117.7 | 140.2 | **24.3** (median of per-run differences) |

Result: median map-to-paint 24.3 ms, below the 100 ms threshold, so no startup span was profiled and no code was changed. The earlier 276/618 ms figures and the ~340 ms map-to-paint gap in this row are stale. The f528ab83 change holds: the terminal paints about 24 ms after map on an idle-ish host.
