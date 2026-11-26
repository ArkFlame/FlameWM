# Unit u68-dock-hover
DONE
Area: Dock/taskbar 1 s hover+click: relayout and redraw scope, animation tick rate, icon draw cache
Owned paths: crates/flamewm-shell/src/dock.rs and dock/**
Task: Batch ten. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Review recent perf commits here for regressions and fix; find the NEXT biggest share in a one-second scenario of normal use and implement at least 3 optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u68-dock-hover.md.
