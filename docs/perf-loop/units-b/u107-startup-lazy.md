# Unit u107-startup-lazy
DONE
Area: Process startup: defer non-critical init (fonts, icons, settings sync, dbus names) until after first frame in explorer, task manager, terminal, shell
Owned paths: crates/flamewm-explorer, flamewm-task-manager, flamewm-terminal, flamewm-shell (main.rs/app init)
Task: Batch sixteen. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep; a ctx_execute spawn from a worker was already killed once). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u107-startup-lazy.md.
