# Unit u57-imaging
OPEN
Area: image-core, imageviewer, screenshot: decode/scale pipeline memory (reuse buffers, downscale early), avoid duplicate full-size copies
Owned paths: crates/flamewm-{image-core,imageviewer,screenshot}/src/**
Task: Batch seven. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u57-imaging.md.
