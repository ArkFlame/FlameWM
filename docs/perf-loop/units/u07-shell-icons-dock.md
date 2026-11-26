# Unit u07-shell-icons-dock
DONE
Area: Shell: icon_loader caching/RAM, dock redraw, runtime.rs/main.rs clones and recomputation
Owned paths: crates/flamewm-shell/src/{icon_loader,dock,runtime,main}.rs
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u07-shell-icons-dock.md.
