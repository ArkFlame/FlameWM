# Unit u66-window-drag
DONE
Area: Compositor 1 s window drag/resize: damage, layout recompute, shadow/blur cache reuse
Owned paths: crates/flamewm-desktop/src/**, flamewm-render-x11
Task: Batch nine. Read .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep). Review recent perf commits here for regressions and fix; find the NEXT biggest hotspot by composition share in normal use and implement at least 3 optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u66-window-drag.md.
