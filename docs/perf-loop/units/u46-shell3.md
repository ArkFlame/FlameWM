# Unit u46-shell3
DONE
Area: shell: tray/quick-controls/clock/weather ticks (align to second boundaries, stop when hidden), dock hover redraws
Owned paths: crates/flamewm-shell/src/**
Task: Batch six. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code and unused warnings (cargo reported unused imports/dead fns in flamewm-desktop, flamewm-explorer, flamewm-settings). Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u46-shell3.md.
