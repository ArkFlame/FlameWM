# Unit u15-imageviewer-archiver
DONE
Area: imageviewer, image-core, archiver, conversion-core: decode off UI thread, downscale-on-decode, cap caches, stream IO
Owned paths: crates/flamewm-{imageviewer,image-core,archiver,archiver-app,conversion-core}/src/**
Task: Scout this area, find top hotspots by composition share (idle wakeups, full redraws, allocations, blocking IO, clones, dead code) and implement at least 3 concrete optimizations keeping visuals/features. No cargo/git. Log changes to docs/perf-loop/LOG-u15-imageviewer-archiver.md.
