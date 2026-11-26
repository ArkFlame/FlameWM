# Unit u59-finder-assistant
DONE
Area: Finder and assistant: index build off-thread, incremental search, result list cap, idle teardown
Owned paths: crates/flamewm-{finder,assistant,assistant-core}/src/**
Task: Batch eight. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u59-finder-assistant.md.
