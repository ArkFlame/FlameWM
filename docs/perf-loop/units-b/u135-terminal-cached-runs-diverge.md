# Unit u135-terminal-cached-runs-diverge
DONE
Area: Terminal correctness. `cargo test -p flamewm-terminal --lib cached_runs` fails deterministically at view.rs:927 "cache diverged from a full rebuild" (cached row runs differ from a full rebuild after random updates). Earlier attempt u121 added a view_offset==0 / changed.is_empty() guard but it still fails.
Owned paths: crates/flamewm-terminal/src/view.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep). You MAY run `cargo test -p flamewm-terminal --lib cached_runs` (only that, no git). Print the first divergent row (left vs right) and the update step that caused it, find the root cause in the run cache invalidation (e.g. changed-line tracking, scroll, resize, selection, cursor row, trailing-blank trimming), and fix the PRODUCT cache logic with the smallest invalidation that keeps the cache win. Never weaken or delete the test. Report root cause and test exit code.
