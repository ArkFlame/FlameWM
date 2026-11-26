# Unit u108-alloc-hot
DONE
Area: Per-frame allocation audit: rg format!/to_string/collect/Vec::new in draw functions of ui-core and the four apps; replace with reused buffers, write! into scratch String, or cached strings
Owned paths: crates/flamewm-ui-core/src, apps draw modules
Task: Batch sixteen. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep; a ctx_execute spawn from a worker was already killed once). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u108-alloc-hot.md.
