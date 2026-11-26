# Unit u24-profiler-debug
OPEN
Area: profiler, debug, api, control-*: ensure zero cost when disabled, no background threads when idle, lazy init
Owned paths: crates/flamewm-{profiler,debug,api,control-core,control-dbus,control-wire,platform,session-core}/src/**
Task: Second-pass scout: re-read the area, find the NEXT worst hotspots by composition share (they differ from earlier units; see git log), implement at least 3 more optimizations keeping visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u24-profiler-debug.md.
