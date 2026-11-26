# Unit u49-assets-ram
DONE
Area: assets and RAM: lazy icon theme index, SVG raster cache cap, ppm asset loading, embedded vs loaded duplicates, startup allocations
Owned paths: crates/flamewm-skin, flamewm-ui, flamewm-shell/src/icon_loader.rs
Task: Batch six. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code and unused warnings (cargo reported unused imports/dead fns in flamewm-desktop, flamewm-explorer, flamewm-settings). Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u49-assets-ram.md.
