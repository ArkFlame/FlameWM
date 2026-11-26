# Unit u137-taskmgr-hidden-tick
DONE
Area: Task manager: sampling cost when hidden or unfocused
Owned paths: crates/flamewm-task-manager/src/lib.rs, crates/flamewm-task-manager/src/model.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Make /proc sampling slow down (for example 1 s to 5 s) or pause while the window is hidden, minimized or unfocused, resume with an immediate sample on show, and format row strings only for visible rows. Keep the live graph correct after a pause (no fake interpolation). Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
