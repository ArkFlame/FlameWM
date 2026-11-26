# Unit u58-settings-ui
DONE
Area: Settings app: page switch cost, list virtualization, avoid rebuilding documents on every change, debounce writes
Owned paths: crates/flamewm-settings*/src/**
Task: Batch eight. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u58-settings-ui.md.
