# Unit u45-terminal3
OPEN
Area: terminal: selection/scroll redraw cost, resize reflow, suggest worker thread, cursor blink timer wakeups
Owned paths: crates/flamewm-terminal/src/**
Task: Batch six. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code and unused warnings (cargo reported unused imports/dead fns in flamewm-desktop, flamewm-explorer, flamewm-settings). Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u45-terminal3.md.
