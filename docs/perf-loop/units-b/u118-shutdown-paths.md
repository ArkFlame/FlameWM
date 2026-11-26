# Unit u118-shutdown-paths
DONE
Area: Shutdown/restart paths of explorer, shell, terminal, task manager: worker threads joined or detached cleanly, no hang on exit, temp files and sockets removed, state saved once
Owned paths: crates/flamewm-explorer, flamewm-shell, flamewm-terminal, flamewm-task-manager (main.rs and app teardown)
Task: Batch nineteen. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u118-shutdown-paths.md.
