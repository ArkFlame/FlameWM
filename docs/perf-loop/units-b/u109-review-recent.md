# Unit u109-review-recent
DONE
Area: Review the last 40 perf commits (git log -40 -p on crates) for regressions: dropped wakeups, missed redraws after state change, races on the new worker threads, lost cancel paths; fix what you find with minimal edits
Owned paths: crates/**/src
Task: Batch seventeen. Read .agents/skills/no-adhoc-scripts/SKILL.md and .agents/skills/scoped-perf-tests/SKILL.md first (NO python/ctx_execute/sleep). Do the area task with rg/Edit, keep visuals/features, no cargo/git. Log to docs/perf-loop/LOG-u109-review-recent.md.
