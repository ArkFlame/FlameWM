# Unit u09-reactor-wm
OPEN
Area: Reactor/WM: timeline/child_pipe wakeups, layout recompute, hash-before-invalidate, frame callbacks only to visible surfaces, lock contention
Owned paths: crates/flamewm-reactor/src/**, flamewm-desktop*, flamewm-ui-layout-core
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u09-reactor-wm.md.
