# Performance Contract

## Runtime rules

1. No browser/WebView/JavaScript/WGPU runtime.
2. HTML/CSS parsing/cascade occurs at build time only.
3. No continuous frame loop.
4. Layout/repaint follows state/damage/input events only.
5. Pointer motion is coalesced only across contiguous same-window MotionNotify events; event ordering is never crossed.
6. Cached X11 pixmaps/masks/fonts/colors are reused.
7. GraphicsExpose events are disabled for retained image copy operations.
8. Network/audio/media use subscriptions, not repeated CLI polling.
9. The WM remains the single managed-window/workspace authority.
10. Measure PSS, idle CPU/wakeups, startup and interaction latency before accepting additional architecture.

## Release profile

```text
opt-level = 3
lto = fat
codegen-units = 1
panic = abort
strip = symbols
```

The compiler crate uses package-specific `opt-level=2`; parser speed is not steady-state interaction cost.

## Acceptance targets

- combined Flame-owned graphical process PSS <= 40 MiB target (met by the 2026-10-29 idle figures below);
- goal: combined idle PSS <= 10 MB. **Not achieved.** Current idle PSS is 23.1 MiB.
- near-zero idle CPU outside real events/timers;
- no visible drag backlog in Xephyr;
- no stale motion rendered after release;
- Start/taskbar/window manipulation must not block on disk/network/system commands.

## Benchmark

Measured 2026-10-29 on Intel TGL UHD (Mesa), Linux 7.0. Figures are not a
release claim. The machine was shared with other builds, so treat every number
as a noisy sample (see [Noise and method](#noise-and-method)).

### How to run

Pixel operations (CPU and GPU backends, 1350x641 frame):

```sh
CARGO_TARGET_DIR=$PWD/target-rel CARGO_BUILD_JOBS=2 \
  flock -w 1800 /tmp/flamewm-cargo.lock \
  cargo run --release --offline -p flamewm-render-x11 --example pixel_bench
```

Source: [`crates/flamewm-render-x11/examples/pixel_bench.rs`](../crates/flamewm-render-x11/examples/pixel_bench.rs).
Each row is the median of 25 iterations inside one run. Repeat the run and take the minimum per row.

Per-process memory (RSS and PSS) on a running session. Start a FlameWM session
on a nested Xephyr display, wait for it to idle, then:

```sh
BENCH_DISPLAY=:166 tools/bench_mem.sh idle
```

[`tools/bench_mem.sh`](../tools/bench_mem.sh) selects processes whose `DISPLAY`
matches `BENCH_DISPLAY` and prints RSS and PSS per process from
`/proc/<pid>/smaps_rollup`. PSS divides shared pages between the processes that map
them, so the PSS total is the footprint to compare. RSS double-counts shared pages.

### Memory, idle (RSS / PSS, MiB)

Values recorded in the 2026-10-29 session measurement. They were not re-run in this pass.

| Process | RSS | PSS |
| --- | --- | --- |
| `flamewm` (WM) | 10.7 | 5.5 |
| `flamewm-desktop` | 19.1 | 10.4 |
| `flamewm-shell` | 15.8 | 7.1 |
| **Total** | **45.6** | **23.1** |

Earlier measurement: 53.6 MB RSS total. The per-process rows above are the 2026-10-29
figures; the earlier per-process split is not recorded here. The sum of the PSS
rows is 23.0 because of rounding.

### Pixel operations (`pixel_bench`, min of 15 runs, ms)

Run 2026-10-29 with the release `pixel_bench` built from the workspace, one binary copy for all runs.
Machine load average during the 15 runs was 56 to 81. Raw per-run output is not committed.

| Operation | CPU | GPU (Mesa TGL) |
| --- | --- | --- |
| blur radius 12, full frame | 58.09 | 4.84 |
| fill rounded 600x300 r24 | 4.00 | 3.83 |
| stroke rounded 600x300 t2 | 0.07 | 3.73 |
| selection fill+stroke 320x210 | 1.34 | 8.51 |
| move outline 800x500 r12 t2 | 0.11 | 3.84 |
| alpha blend mask 1000x700 | 4.86 | 6.91 |
| blur radius 28 on 800x500 | 10.05 | 2.18 |
| frame batch 60 fills + 20 strokes + 2 blurs, per-op calls | 133.45 | 468.33 |
| same batch, one `paint_batch` call | 162.22 | 29.84 |
| batch fills + strokes only (60f + 20s) | 14.54 | 27.37 |
| batch blurs only (2 blurs) | 143.42 | 6.24 |

What these rows show:

- On this machine under this load, the GPU backend is faster than the CPU backend for blur
  and the frame batch via `paint_batch` (29.8 ms GPU vs 162.2 ms CPU). The GPU fill, stroke and
  selection rows are slower than CPU, which is consistent with per-call upload and readback cost.
- The GPU per-op batch (468 ms) shows that per-call upload and readback is the dominant cost, not the
  draw itself. This is why the batch API exists.
- In this run, CPU `paint_batch` (162 ms) is not faster than CPU per-op (133 ms). The earlier
  2026-10-29 baseline (median of 25, pre-optimization, measured at lower load) showed 112.5 ms
  for `paint_batch` against 167.8 ms per-op. These runs are not comparable, because
  the baseline was measured at different load and the machine was busy here. This pass does
  not show an improvement over the baseline, and it does not show a regression.

### Interaction and CPU (2026-10-30)

Headless Xorg dummy session (1280x800, single output), release build of `main` at `b7e8c300`
against this branch, both with symbols kept for profiling. Each workload runs 8 s, two rounds,
minimum per row. Percent of one core, summed over every `flamewm*` process on the display, with
Xorg separately.

```sh
python3 tools/flamewm_cpu_workloads.py .target/release \
  idle,hover-desktop,hover-taskbar,start-toggle,term-idle,term-seq,term-type,term-move,term-hover,explorer-idle,explorer-hover,explorer-move 8
```

| Workload | FlameWM before | after | Xorg before | after |
| --- | --- | --- | --- | --- |
| idle desktop | 1.49 | 0.37 | 0.50 | 0.37 |
| pointer sweep over desktop | 3.12 | 1.62 | 1.00 | 1.12 |
| pointer sweep over taskbar | 2.62 | 1.75 | 1.62 | 1.50 |
| Start menu toggle every 0.4 s | 5.11 | 3.74 | 1.12 | 1.25 |
| terminal open, idle | 1.62 | 0.50 | 0.37 | 0.50 |
| terminal `seq 1 300000` | 5.61 | 4.37 | 0.50 | 0.62 |
| typing in terminal (30 ms/key) | 11.81 | 5.84 | 13.05 | 12.06 |
| dragging terminal window | 9.58 | 7.96 | 6.83 | 6.71 |
| pointer sweep over terminal | 4.36 | 2.87 | 1.12 | 1.25 |
| explorer on `/usr/lib`, idle | 1.50 | 0.50 | 0.37 | 0.37 |
| pointer sweep over explorer | 2.99 | 1.87 | 1.25 | 1.12 |
| dragging explorer window | 9.33 | 7.59 | 7.47 | 7.34 |

The terminal process alone went from 9.5 % to 4.1 % while typing; most of that cost was walking
the 96x128 pooled run nodes (about 25 000, nearly all hidden) several times per keystroke. The
idle desktop cost is the once-per-second clock repaint.

Wheel scrolling in the explorer is not in the table: the glide animation renders anywhere from
about 100 to 1 200 frames in the same window, so CPU percent tracks frame count, not cost.
Measured per frame it went from about 2.3 ms to about 1.7 ms.

Not changed here: every repaint is still a full-window repaint (`FLAMEWM_RENDER_REGION` stays
opt-in), which is what keeps Xorg near 12 % while typing.

## Noise and method

- Use the minimum of at least 15 runs per row. The minimum is the least-disturbed sample; the median and maximum
  include other load on the machine.
- Load average was 56 to 81 during the pixel runs, from other builds on the same box. Results are
  not valid as absolute numbers on an idle machine. Re-run on an idle machine before quoting a
  number outside this document.
- Each run is one process. Memory figures come from a single idle snapshot per session.
- Debug builds are not performance evidence.

## Profiler (`flamewm-profiler`, frozen seams R01-R06)

- Static `&'static str` labels only; no per-span allocation or thread.
- Env: `FLAMEWM_PROFILE=1`, `FLAMEWM_PROFILE_INTERVAL` (default 60, min 10),
  `FLAMEWM_PROFILE_TOP` (default 8, max 8).
- Report: single `FLAMEWM_PROFILE_SUMMARY` block via `report_window()` with
  `SLOWEST_SINGLE` / `HIGHEST_TOTAL` / `HIGHEST_CALL_COUNT` / `MEMORY_TOP`.
- No product instrumentation yet; product crates opt in later.
