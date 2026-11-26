# Unit u37-compositor
DONE
Area: flamewm-desktop compositor paths: damage union, skip occluded windows, shadow/blur cache keyed by size, avoid per-frame Vec
Owned paths: crates/flamewm-desktop/src/**
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u37-compositor.md.
