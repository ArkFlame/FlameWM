# meas-terminal audit

Status: discovery only; no fresh performance run was performed for this audit, and no performance conclusion is made here.

## Conflicting historical startup figures

`docs/backlog/measure.md` records an earlier terminal startup result of 276.4 ms to map and 618.1 ms to paint (three runs; fast-profile build, shared host). Its later cold-start re-measure, based on `be106c4d`, records seven timed runs after one warmup and a 24.3 ms median map-to-paint; it explicitly marks the earlier figures and the approximately 340 ms gap as stale. Treat the later record as the current historical observation, not as a fresh measurement or a release-grade result.

The records differ in workload/harness and run count; neither alone establishes a code regression or improvement. No averaging or direct comparison is valid. The later record reports the result below its 100 ms profiling threshold, so no startup span was profiled and no code change was made.

## Sustained-output evidence

The historical sustained-output section reports a profiled run under noisy shared-host conditions. It attributes most sampled time to parsing/screen printing, while explicitly noting that no sampling CPU profile, unprofiled FPS count, or allocations-per-frame measurement was available. A separate event-coalescing trial was below the stated 10% acceptance bar and was reverted. These findings are historical and do not establish current performance.

## Audit disposition

- No product code changed and no benchmark was rerun.
- Do not infer current startup latency, throughput, allocation cost, or a performance win from these historical records.
- Before a new comparison, run the same harness/workload and build configuration on base and candidate revisions, retain raw runs and host/display/load metadata, and report the sampling distribution and exclusions.
- Profiling remains separately gated by available profiler permissions; absence of a profile is not evidence that any specific function dominates.
