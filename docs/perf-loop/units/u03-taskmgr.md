# Unit u03-taskmgr
OPEN
Area: Task manager: incremental /proc sampling, visible rows only, slow tick when hidden, no per-frame formatting
Owned paths: crates/flamewm-task-manager/src/**
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u03-taskmgr.md.
