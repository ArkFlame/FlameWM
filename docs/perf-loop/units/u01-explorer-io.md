# Unit u01-explorer-io
OPEN
Area: Explorer: all disk work async/off UI thread, streamed listing, cancelable jobs, inotify debounce
Owned paths: crates/flamewm-explorer/src/{jobs,refresh,search,lib,main}.rs
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u01-explorer-io.md.
