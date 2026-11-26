# Unit u94-xwayland-damage
DONE
Area: SUSPECTED BUG: an X11 client (flamewm-terminal, 25-42%% CPU painting) yields only a few render dispatches/s in the nested compositor (99%% wait). Trace XWayland surface commit -> damage -> render scheduling in wm-wayland and find where updates are dropped or throttled (frame callback gating, damage not marked, redraw not requested after X11 commit). Fix it if it is a bug; if it is correct behaviour explain with code refs.
Owned paths: crates/flamewm-wm-wayland/src (xwayland*, render*, scene*)
Task: Batch fourteen. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u94-xwayland-damage.md.
