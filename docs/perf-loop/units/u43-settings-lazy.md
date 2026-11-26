# Unit u43-settings-lazy
DONE
Area: flamewm-settings: warnings, lazy page construction, only visible page updates, no background polling when hidden
Owned paths: crates/flamewm-settings*/src/**
Task: Batch six. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code and unused warnings (cargo reported unused imports/dead fns in flamewm-desktop, flamewm-explorer, flamewm-settings). Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u43-settings-lazy.md.
