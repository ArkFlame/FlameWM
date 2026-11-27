# FlameWM performance baseline (frozen)

This file is **frozen after the first capture**. `scripts/bench-baseline` is a
one-shot, scoped tool: run it against an already running flamewm session with
`--out docs/PERF_BASELINE.md` and it appends exactly one dated section holding
the commit hash, machine identity, a composition table and the raw counter rows.
A second capture is refused, so the baseline never drifts.

The sample is small and scoped: one second of simulated usage across the
`flamewm-*` processes only. The composition reports each process's **share of
CPU ticks and of wakeups** in that one-second sample — never absolute CPU, never
a whole-system soak. Wakeups are the delta of voluntary (plus nonvoluntary)
context switches over the one-second window, read from `voluntary_ctxt_switches`
and `nonvoluntary_ctxt_switches` in `/proc/<pid>/status`; RSS and thread count
come from the same `/proc/<pid>/status`; CPU ticks are `utime + stime` from
`/proc/<pid>/stat`. Rows are ordered by CPU-tick share so the largest share can be fixed first and
then re-ranked (see `.agents/skills/scoped-perf-tests/SKILL.md`).

Until the first capture, the table below is a placeholder.

## Composition (placeholder — awaiting first capture)

| Process | RSS (kB) | Threads | Tick Δ | Tick share % | Wakeups/s | Wake share % |
| --- | --- | --- | --- | --- | --- | --- |
| _none — no baseline captured yet_ | — | — | — | — | — | — |

---

## Frozen baseline capture — 2026-11-27T20:43:23Z

<!-- bench-baseline:captured — baseline is taken ONCE; do not capture again -->

- Commit: `7f7db239`
- Machine: `Linux 7.3.0-11-generic x86_64`
- CPU model: 11th Gen Intel(R) Core(TM) i5-11400H @ 2.70GHz
- Window: 1s, 5 flamewm-* process(es)

### Composition (share of sample, NOT absolute CPU)

| Process | RSS (kB) | Threads | Tick Δ | Tick share % | Wakeups/s | Wake share % |
| --- | --- | --- | --- | --- | --- | --- |
| flamewm-desktop (3798072) | 24652 | 6 | 1 | 100.0 | 21.0 | 51.2 |
| flamewm-wayland (3797722) | 123992 | 10 | 0 | 0.0 | 19.0 | 46.3 |
| flamewm-audio-h (3797769) | 6424 | 3 | 0 | 0.0 | 0.0 | 0.0 |
| flamewm-shell (3798071) | 17960 | 4 | 0 | 0.0 | 1.0 | 2.4 |
| flamewm-gpu-hel (3798232) | 142924 | 1 | 0 | 0.0 | 0.0 | 0.0 |

### Raw rows

| PID | Name | RSS (kB) | Threads | ticks t0 | ticks t1 | ctxt t0 | ctxt t1 | Δticks | Δctxt |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 3798072 | flamewm-desktop | 24652 | 6 | 111 | 112 | 816 | 837 | 1 | 21 |
| 3797722 | flamewm-wayland | 123992 | 10 | 71 | 71 | 2706 | 2725 | 0 | 19 |
| 3797769 | flamewm-audio-h | 6424 | 3 | 0 | 0 | 63 | 63 | 0 | 0 |
| 3798071 | flamewm-shell | 17960 | 4 | 17 | 17 | 1017 | 1018 | 0 | 1 |
| 3798232 | flamewm-gpu-hel | 142924 | 1 | 25 | 25 | 468 | 468 | 0 | 0 |

