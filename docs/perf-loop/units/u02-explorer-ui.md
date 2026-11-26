# Unit u02-explorer-ui
DONE
Area: Explorer: virtualized lists, thumbnail/icon LRU caches with caps, lazy metadata, scroll redraw
Owned paths: crates/flamewm-explorer/src/app/**, places.rs
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u02-explorer-ui.md.
