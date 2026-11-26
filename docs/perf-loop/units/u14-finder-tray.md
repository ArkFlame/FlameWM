# Unit u14-finder-tray
DONE
Area: finder, tray, clipboard, usage, weather, screenshot: idle polling to event-driven, lazy init, slow ticks when hidden
Owned paths: crates/flamewm-{finder,tray,clipboard,usage,weather,screenshot}/src/**
Task: Scout this area, find top hotspots by composition share (idle wakeups, full redraws, allocations, blocking IO, clones, dead code) and implement at least 3 concrete optimizations keeping visuals/features. No cargo/git. Log changes to docs/perf-loop/LOG-u14-finder-tray.md.
