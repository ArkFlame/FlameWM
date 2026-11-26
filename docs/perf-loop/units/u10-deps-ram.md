# Unit u10-deps-ram
DONE
Area: RAM and deps: trim Cargo.toml features/unused deps, lazy-init fonts/icons, shrink structs, intern strings, cap caches across apps
Owned paths: Cargo.toml files, flamewm-ui, flamewm-ui-core, flamewm-skin
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u10-deps-ram.md.
