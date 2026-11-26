# Unit u145-terminal-ui-allocs
DONE
Area: Terminal: ui.rs per-frame allocations
Owned paths: crates/flamewm-terminal/src/ui.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Remove per-frame clone/to_string/format in the draw path: cache formatted strings keyed on their inputs, borrow instead of cloning, reuse buffers across frames. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
