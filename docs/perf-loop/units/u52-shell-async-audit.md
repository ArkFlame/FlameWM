# Unit u52-shell-async-audit
OPEN
Area: Shell: audit blocking calls (fs reads, process spawn wait, dbus calls) on the UI thread and move off-thread
Owned paths: crates/flamewm-shell/src/**
Task: Batch seven. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u52-shell-async-audit.md.
