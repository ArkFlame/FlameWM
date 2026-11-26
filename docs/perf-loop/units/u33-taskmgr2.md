# Unit u33-taskmgr2
DONE
Area: Task manager sampling: /proc readers reuse buffers, parse without allocation, diff-based table updates, cadence by visibility
Owned paths: crates/flamewm-task-manager/src/**
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u33-taskmgr2.md.
