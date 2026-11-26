# Unit u34-terminal2
DONE
Area: Terminal: VT parser allocations, scrollback storage compaction, glyph cache, redraw rect minimization
Owned paths: crates/flamewm-terminal/src/**
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u34-terminal2.md.
