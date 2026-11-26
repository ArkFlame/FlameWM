# Unit u144-shell-network-projection
DONE
Area: Shell: network projection per-refresh strings
Owned paths: crates/flamewm-shell/src/projection/network.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). 22 clone/to_string/format sites rebuilt on every network event. Skip identical snapshots, borrow instead of cloning, and only format visible rows. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
