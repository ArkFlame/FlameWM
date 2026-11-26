# Unit u41-desktop-dead
OPEN
Area: flamewm-desktop warnings and dead code, widgets lifecycle/render, hide_panels, visibility_effect
Owned paths: crates/flamewm-desktop/src/widgets/**, main.rs
Task: Batch six. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code and unused warnings (cargo reported unused imports/dead fns in flamewm-desktop, flamewm-explorer, flamewm-settings). Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u41-desktop-dead.md.
