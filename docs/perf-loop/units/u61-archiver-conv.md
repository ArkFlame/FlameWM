# Unit u61-archiver-conv
OPEN
Area: archiver, archiver-app, conversion-core, downloads-core: stream IO with big buffers, progress throttling to 10Hz, cancel support, off UI thread
Owned paths: crates/flamewm-{archiver,archiver-app,conversion-core,downloads-core}/src/**
Task: Batch eight. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u61-archiver-conv.md.
