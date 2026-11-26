# Unit u60-lock-ribbons-tray
DONE
Area: lock, ribbons, tray, clipboard: wakeups, polling, memory caps, startup cost
Owned paths: crates/flamewm-{lock,ribbons,tray,clipboard}/src/**
Task: Batch eight. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u60-lock-ribbons-tray.md.
