# Unit u13-compiler
DONE
Area: render-compiler: cache compiled display lists, avoid rebuilding unchanged subtrees, reduce allocation
Owned paths: crates/flamewm-render-compiler/src/**
Task: Scout this area, find top hotspots by composition share (idle wakeups, full redraws, allocations, blocking IO, clones, dead code) and implement at least 3 concrete optimizations keeping visuals/features. No cargo/git. Log changes to docs/perf-loop/LOG-u13-compiler.md.
