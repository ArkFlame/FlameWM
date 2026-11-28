# Unit p79-hotlist-rerank
STATUS: DONE

Read every Result table in docs/swarm/pool/p*.md (rg '^\|' per file). Rewrite the ranked rows of docs/PERF_HOTLIST.md: group landed cuts (mark landed UNMEASURED), keep the BASELINE section and the correction note as is, add new rows for every finding a worker marked NONE-but-risky or PARTIAL. Docs only.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `docs/PERF_HOTLIST.md:15-26` | Ranked rows 1-12 mixed landed cuts with open/deferred ones and had no uniform measurement label. | Marked landed and partially landed cuts `LANDED`/`PARTIAL — UNMEASURED`; closed rows left as is. | UNBUILT: docs only; no build, test or measurement run. |
| `docs/PERF_HOTLIST.md:32-41` | Rows 13-20 used inconsistent "Cut landed" wording. | Normalised landed rows to `LANDED — UNMEASURED`; closed and deferred rows unchanged. | UNBUILT: docs only. |
| `docs/PERF_HOTLIST.md:42-58` | The interrupted pass had already added rows 21-32; the resumed sweep duplicated them as 21-36. | Merged both into one deduplicated rows 21-37 table covering every NONE-but-risky and PARTIAL finding, adding p47 damage gating, p90 bold font, p53 parse borrows and p02 wall-clock anchor. No rank is duplicated. | UNBUILT: docs only; every cited worker edit stays UNBUILT/UNMEASURED. |
| `docs/PERF_HOTLIST.md:60-73` | The "Landed cuts (all UNMEASURED)" grouping from the prior pass referenced the old numbering. | Kept the grouping and left the BASELINE section (74-85), the same-day correction note (87) and the TRUE COMPARISON table byte-identical. | UNBUILT: docs only; no runtime claim made. |
| `docs/swarm/pool/p79-hotlist-rerank.md:6` | STARTED marker and stale bullet Result remained from the interrupted pass. | Replaced with this table and a PASS paragraph; STATUS was already DONE. | UNBUILT: tracking file only. |

PASS — Every Result table in `docs/swarm/pool/p*.md` was read; the ranked rows of `docs/PERF_HOTLIST.md` now group landed cuts under a `LANDED — UNMEASURED` label, and a single deduplicated rows 21-37 table carries every finding a worker marked NONE-but-risky or PARTIAL (including p34 popup-surface retention, p47 damage gating, p65 EGL deferral, p90 bold font and the p43 `panic = "abort"` risk). The BASELINE section and the correction note are byte-identical, no rank is duplicated, and no gate or visual criterion was loosened. All three production edits were documentation-only and UNBUILT: no cargo, build, test, git or runtime measurement was run here, so the worker claims remain unverified until the orchestrator builds the batch and samples the cuts.
