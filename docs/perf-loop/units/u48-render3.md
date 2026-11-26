# Unit u48-render3
OPEN
Area: render-core/x11: damage-rect accumulation, glyph/text run cache, image scaling cache with byte cap
Owned paths: crates/flamewm-render-core, flamewm-render-x11, flamewm-ui-x11
Task: Batch six. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code and unused warnings (cargo reported unused imports/dead fns in flamewm-desktop, flamewm-explorer, flamewm-settings). Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u48-render3.md.
