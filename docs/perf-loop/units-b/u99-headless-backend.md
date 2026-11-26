# Unit u99-headless-backend
OPEN
Area: Active-use measurement without a host: check whether wm-wayland has or can cheaply get a headless/offscreen render backend (no swap_buffers) so scripts/compose-scenario can sample a steady active second; if smithay offers a headless/pixman path, wire it behind an env var; otherwise document why not
Owned paths: crates/flamewm-wm-wayland/src, scripts/compose-scenario
Task: Batch fifteen. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u99-headless-backend.md.
