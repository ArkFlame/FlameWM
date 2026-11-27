# FlameWM performance baseline (frozen)

This file is **frozen after the first capture**. `scripts/bench-baseline` is a
one-shot, scoped tool: run it against an already running flamewm session with
`--out docs/PERF_BASELINE.md` and it appends exactly one dated section holding
the commit hash, machine identity, a composition table and the raw counter rows.
A second capture is refused, so the baseline never drifts.

The sample is small and scoped: one second of simulated usage across the
`flamewm-*` processes only. The composition reports each process's **share of
CPU ticks and of wakeups** in that one-second sample — never absolute CPU, never
a whole-system soak. Wakeups are the sum of voluntary and nonvoluntary context
switches from `/proc/<pid>/sched`; RSS and thread count come from
`/proc/<pid>/status`; CPU ticks are `utime + stime` from `/proc/<pid>/stat`.
Rows are ordered by CPU-tick share so the largest share can be fixed first and
then re-ranked (see `.agents/skills/scoped-perf-tests/SKILL.md`).

Until the first capture, the table below is a placeholder.

## Composition (placeholder — awaiting first capture)

| Process | RSS (kB) | Threads | Tick Δ | Tick share % | Wakeups/s | Wake share % |
| --- | --- | --- | --- | --- | --- | --- |
| _none — no baseline captured yet_ | — | — | — | — | — | — |
