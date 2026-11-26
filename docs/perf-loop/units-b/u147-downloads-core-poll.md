# Unit u147-downloads-core-poll
DONE
Area: downloads-core: dead or hot POLL constant
Owned paths: crates/flamewm-downloads-core/src/lib.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p flamewm-downloads-core once at the end). `pub const POLL` (100 ms) is defined at lib.rs:16. Use rg across the whole workspace to find who uses it. If nothing does, delete it. If a consumer polls at 100 ms while idle, make it wake only while a download is active and park with no timer otherwise. Keep all features. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
