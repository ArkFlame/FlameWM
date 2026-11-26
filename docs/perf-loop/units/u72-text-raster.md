# Unit u72-text-raster
DONE
Area: Text rasterization share: glyph cache hit rate, shaping reuse, subpixel/atlas upload batching across render-core and ui-core
Owned paths: crates/flamewm-render-core, flamewm-ui-core, flamewm-skin
Task: Batch ten. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Review recent perf commits here for regressions and fix; find the NEXT biggest share in a one-second scenario of normal use and implement at least 3 optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u72-text-raster.md.
