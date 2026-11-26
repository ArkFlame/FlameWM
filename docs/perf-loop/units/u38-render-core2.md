# Unit u38-render-core2
DONE
Area: render-core pixels.rs/model: blend and fill fast paths, premultiply caching, scratch buffer reuse, clone removal
Owned paths: crates/flamewm-render-core/src/**
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u38-render-core2.md.
