# Unit p80-fault-matrix
STATUS: DONE

Read docs/FAULT_TESTS.md and rg for fault/fallback tests in crates/*/src (rg 'fn .*(fallback|missing|corrupt|degrad|absent)'). Produce a matrix in docs/FAULT_TESTS.md: each fallback in the goal (backend DRM/nested/X11/minimal, GPU hw/sw/cpu, missing lib/font/icon/theme/portal, corrupt settings, panic restart) -> test path or GAP. Docs only; list top 5 gaps as next units at the end.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `docs/swarm/pool/p80-fault-matrix.md:6` | Unit file had no result section | Appended `## Result` / `STARTED` marker (bookkeeping) | None; docs only, UNBUILT |
| `docs/FAULT_TESTS.md:72` | The 14 goal fallbacks had no single map to a test path or GAP | Appended `Fallback -> test matrix` with 14 rows, each with `file:line` test evidence and coverage status | `file:line` anchors drift on test rename; UNBUILT |
| `docs/FAULT_TESTS.md:98` | No next-unit backlog for the uncovered fallbacks | Appended top 5 gaps as p81-p85 | Priorities are static-read estimates, not runtime-verified; UNBUILT |
| `docs/swarm/pool/p80-fault-matrix.md:2` | `STATUS: OPEN` | Set `STATUS: DONE` and recorded these rows | Bookkeeping only; UNBUILT |

PARTIAL: the matrix and gap backlog are complete from source-level evidence; every edit is UNBUILT
because the brief forbids git, cargo and builds. Four fallbacks are marked partial rather than
covered (headless runner state, missing-lib supervisor run, missing-font injection, panic-restart
end-to-end) and are handed to p81-p85. No gates were loosened, no visuals changed, and no code was
built or tested in this unit.
