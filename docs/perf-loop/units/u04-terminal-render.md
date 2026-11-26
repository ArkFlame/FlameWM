# Unit u04-terminal-render
OPEN
Area: Terminal: dirty-line rendering, glyph atlas reuse, no redraw when unchanged
Owned paths: crates/flamewm-terminal/src/{screen,main}.rs
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u04-terminal-render.md.
