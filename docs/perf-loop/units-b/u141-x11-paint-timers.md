# Unit u141-x11-paint-timers
OPEN
Area: X11 WM: paint and provider timers
Owned paths: crates/flamewm-wm-x11/src/wm/paint.rs, crates/flamewm-wm-x11/src/runtime/providers.rs, crates/flamewm-wm-x11/src/runtime/mod.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Find interval/timer based repaint or provider polling. Make it event-driven: repaint only on damage/dirty flags, providers refresh on their source event with a coalescing delay, and no timer armed while idle. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
