# Unit u138-wm-damage-skip
DONE
Area: Compositor: redundant redraw and frame callbacks
Owned paths: crates/flamewm-wm-wayland/src/render, crates/flamewm-wm-wayland/src/state/redraw.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Read the redraw path. Find any case where a frame is rendered or frame callbacks are sent to occluded, minimized or off-output surfaces, or where an output with empty damage still renders, and skip it. Cite the exact function and the condition added. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
