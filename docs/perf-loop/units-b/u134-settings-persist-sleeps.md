# Unit u134-settings-persist-sleeps
DONE
Area: Platform: settings_persist short sleeps
Owned paths: crates/flamewm-platform/src/services/settings_persist.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no cargo/git). Replace sleep-based debounce/polling with a condvar or channel recv_timeout coalescer: parked when idle, one write per burst, no lost last write on shutdown. Keep all features and visuals. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
