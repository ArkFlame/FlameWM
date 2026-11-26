# Unit u40-misc-apps
DONE
Area: settings, finder, imageviewer, archiver, tray: startup cost (lazy init), idle wakeups, memory caps, dead code
Owned paths: crates/flamewm-{settings,finder,imageviewer,archiver,tray,ribbons,lock}/src/**
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u40-misc-apps.md.
