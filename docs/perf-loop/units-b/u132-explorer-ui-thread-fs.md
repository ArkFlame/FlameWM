# Unit u132-explorer-ui-thread-fs
DONE
Area: Explorer: any blocking fs call reachable from the UI thread
Owned paths: crates/flamewm-explorer/src/dndext.rs, crates/flamewm-explorer/src/lib.rs, crates/flamewm-explorer/src/places.rs, crates/flamewm-explorer/src/properties.rs, crates/flamewm-explorer/src/trash.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no cargo/git). Audit every std::fs/exists()/is_dir()/metadata call in these files and trace whether it runs on the UI/event thread (drag-drop, places refresh, properties, trash). Move each one found onto the existing io worker or a cancellable job with a cached result, so the UI never blocks on disk or network mounts. Keep all features and visuals. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
