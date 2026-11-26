# Unit u51-explorer-async-audit
DONE
Area: Explorer: audit every std::fs / read_dir / metadata / canonicalize call reachable from UI event handlers and move to worker threads with result channels
Owned paths: crates/flamewm-explorer/src/**
Task: Batch seven. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u51-explorer-async-audit.md.
