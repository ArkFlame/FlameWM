# Unit u55-taskmgr-clone
DONE
Area: Task manager: remove clones in model->view projection, reuse Vec buffers, intern process names/users
Owned paths: crates/flamewm-task-manager/src/**
Task: Batch seven. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u55-taskmgr-clone.md.
