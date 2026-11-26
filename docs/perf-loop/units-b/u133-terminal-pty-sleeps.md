# Unit u133-terminal-pty-sleeps
DONE
Area: Terminal: PTY read loop sleeps and polling
Owned paths: crates/flamewm-terminal/src/pty.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no cargo/git). Replace thread::sleep/short-timeout polling in the PTY path with blocking reads or poll(2)/epoll wakeups, batching reads into one dirty notification per burst, so an idle terminal has zero wakeups. Keep all features and visuals. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
