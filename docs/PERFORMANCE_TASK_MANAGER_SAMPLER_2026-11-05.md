# Task manager sampler: CPU and syscall cost (2026-11-05)

Scope: `crates/flamewm-task-manager/src/model.rs` (`Sampler::sample_task`). The UI paint path is unchanged.

## What changed

- One `/proc/<pid>/stat` read per task per sample. RSS comes from stat field 24 (pages x AT_PAGESZ, read once from `/proc/self/auxv`), so `/proc/<pid>/status` is no longer read every sample.
- Static fields (`uid`, `user`, `command`) are cached per PID and re-read on first sight, on a comm change, or every 5 samples. A PID reuse changes the start time and forces a refresh, so it never inherits old CPU counters.
- One reused byte buffer for every procfs read; no `read_to_string` size probe.
- The PID map is updated in place and pruned by sample tick instead of rebuilt per sample.

## Measured (same host, same process count, release build)

Bench: `cargo test -p flamewm-task-manager --release bench_sampler -- --ignored --nocapture` (600 samples, self CPU ticks).

| Variant | CPU ms/sample | Tasks | Syscalls/sample (strace -c -f) |
| --- | ---: | ---: | ---: |
| before (origin/main) | 44.8, 44.5 (earlier run 54.2 under load) | 631-632 | ~17,500 |
| after | 18.0, 18.7 | 632-633 | ~3,550 |

GUI task manager on private Xvfb `:99`, 30 s window, `cpu_percent = utime+stime / wall`:

| Run | before | after |
| --- | ---: | ---: |
| 1 | 8.06% | 6.03% |
| 2 | 7.03% | 5.89% |

`app_wakes.sh` total wakes/s was 155 vs 146 (run 1) and 101 vs 134 (run 2). This is too noisy to claim a wake reduction. The sampler still wakes once per second.

## Considered and not done

- Persistent per-PID fds with `pread`: a stale fd can keep reporting an exited task after PID reuse, and the default soft fd limit (1024) is too small for hundreds of tasks.
- Skipping non-visible rows: CPU sort needs every task's ticks, so this would change results.
- Adaptive sample rate: kept at 1 Hz while visible, as required.
- `text_ref` for per-row UI text: each sample rewrites about 20 rows. Not measured in-process; estimated well below 3% of the sampler cost.

## Known lag

A same-name `exec` (comm unchanged) can show the old command line for up to 5 samples.
