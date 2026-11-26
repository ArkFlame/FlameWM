# Unit u143-shell-audio-projection
DONE
Area: Shell: audio projection per-refresh strings
Owned paths: crates/flamewm-shell/src/projection/audio.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). 24 clone/to_string/format sites in a projection rebuilt on every audio event. Hash or compare the new snapshot against the previous one and skip the projection write when nothing visible changed; use text_ref / borrowed strings instead of allocating per row. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
