# Unit u140-dock-backdrop-clones
OPEN
Area: Compositor: dock_backdrop per-frame clones
Owned paths: crates/flamewm-wm-wayland/src/render/dock_backdrop.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p <crate> once at the end). Eight clone() sites in the per-frame path. Replace with borrows, Arc or Cow, cache the built element/texture keyed on (size, scale, theme, content hash) and rebuild only on change; no per-frame allocation when nothing changed. Keep all features and visuals. Add or extend a small scoped test where logic is pure. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
