# Unit u92-active-scenario
DONE
Area: Active-use composition. Write scripts/compose-scenario (bash only): start the nested compositor with FLAMEWM_PROFILE_COMPOSE=1 plus one terminal client, feed it 1 s of output via its PTY (e.g. `yes | head -n 2000`), capture the COMPOSITION block, print shares. No python/sleep binary; use `read -rt`. Then name the largest share and add the cheapest fix as code.
Owned paths: scripts/compose-scenario, crates/flamewm-wm-wayland
Task: Read .agents/skills/no-adhoc-scripts/SKILL.md and scoped-perf-tests/SKILL.md first. No cargo/git. Log to docs/perf-loop/LOG-u92-active-scenario.md.
