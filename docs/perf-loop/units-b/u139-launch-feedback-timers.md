# Unit u139-launch-feedback-timers
DONE
Area: Shell: launch_feedback timers
Owned paths: crates/flamewm-shell/src/launch_feedback.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Six timer/interval sites. Make the animation/timeout timers armed only while a launch is in flight and fully disarmed (no wakeups) when idle; coalesce multiple pending launches onto one timer; no per-tick allocation. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
