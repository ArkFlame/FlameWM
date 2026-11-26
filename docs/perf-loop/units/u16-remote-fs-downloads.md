# Unit u16-remote-fs-downloads
OPEN
Area: remote-fs, downloads-core, integrations: async IO, no blocking calls on UI path, bounded buffers
Owned paths: crates/flamewm-{remote-fs,downloads-core,integrations-core,integrations-linux}/src/**
Task: Scout this area, find top hotspots by composition share (idle wakeups, full redraws, allocations, blocking IO, clones, dead code) and implement at least 3 concrete optimizations keeping visuals/features. No cargo/git. Log changes to docs/perf-loop/LOG-u16-remote-fs-downloads.md.
