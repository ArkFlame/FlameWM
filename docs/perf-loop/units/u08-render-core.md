# Unit u08-render-core
DONE
Area: Render core: damage regions, pixel ops, per-frame allocations, cache blur/shadow/gradients, batching
Owned paths: crates/flamewm-render-core/src/**, flamewm-render-x11, flamewm-ui-x11
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u08-render-core.md.
