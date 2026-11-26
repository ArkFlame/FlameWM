# Unit u17-settings-assistant-lock
DONE
Area: settings, assistant, lock, ribbons, skin: lazy init, idle wakeups, cache caps, dead code removal
Owned paths: crates/flamewm-{settings,settings-core,assistant,assistant-core,lock,ribbons,skin}/src/**
Task: Scout this area, find top hotspots by composition share (idle wakeups, full redraws, allocations, blocking IO, clones, dead code) and implement at least 3 concrete optimizations keeping visuals/features. No cargo/git. Log changes to docs/perf-loop/LOG-u17-settings-assistant-lock.md.
