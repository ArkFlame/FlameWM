# Unit u142-reactor-timers
DONE
Area: Reactor: timer wheel idle behaviour
Owned paths: crates/flamewm-reactor/src/lib.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Verify the reactor sleeps with an infinite timeout when no timer is due (no fixed tick), recomputes the next deadline only when timers change, and drops cancelled timers without scanning. Fix what is missing; keep the typed facade and public API. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
