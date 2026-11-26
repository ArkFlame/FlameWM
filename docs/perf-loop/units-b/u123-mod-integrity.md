# Unit u123-mod-integrity
OPEN
Area: Integrity: for every mod declaration (mod NAME; or pub mod NAME;) in crates/**/src verify NAME.rs or NAME/mod.rs exists on disk AND is tracked by git (git ls-files); report any gap; also list files referenced by include_str or include_bytes that are untracked
Owned paths: crates/**/src (read-only audit, fix only missing declarations)
Task: Batch twenty. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u123-mod-integrity.md.
