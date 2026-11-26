# Unit u32-explorer-thumbs
DONE
Area: Explorer thumbnails/icons: decode off-thread, downscale on decode, LRU byte cap, prioritize visible rows, drop on scroll-away
Owned paths: crates/flamewm-explorer/src/** thumb/icon files
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u32-explorer-thumbs.md.
