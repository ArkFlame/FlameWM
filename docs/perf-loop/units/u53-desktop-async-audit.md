# Unit u53-desktop-async-audit
OPEN
Area: Desktop: audit blocking fs/process/dbus calls in event paths (icons, wallpaper decode, desktop entry scan) and make async/cached
Owned paths: crates/flamewm-desktop/src/**
Task: Batch seven. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u53-desktop-async-audit.md.
