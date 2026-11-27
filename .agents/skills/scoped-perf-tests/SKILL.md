---
name: scoped-perf-tests
description: Load before measuring or reasoning about CPU/GPU/RAM/compositor cost. Perf tests are tiny, scoped, one second of simulated usage, and report composition, never total CPU.
---
# Scoped performance tests (user order, permanent)

The goal of a perf test is NOT total CPU usage. It is a small COMPOSITION: in normal usage, which part uses the most, as a share of the whole. Fix the largest share, then re-rank.

Rules:
- One scenario, one second of simulated usage (e.g. 1 s of scrolling a list, 1 s of terminal output, 1 s idle desktop, 1 s of window drag). Never minutes, never full-system soaks, never full boots for a perf question.
- Scope to one component or flow. Drive it in-process or with a tiny harness, not the whole OS.
- Report shares (percent of the sample by function/thread/subsystem: wakeups, layout, raster, blit, alloc, IO), not absolute CPU%, fps or ms as a verdict.
- Use the repo's profiler counters (`flamewm-profiler`, `CounterPoint`) and `scripts/perf`; no ad-hoc python or ctx_execute (see `no-adhoc-scripts`).
- Heavy builds stay on the ~30-minute gate; a perf sample never triggers an extra compile.
- Record the composition (top 3 shares) in `docs/perf-loop/LOG.md` with the change it motivated.
