# Unit u146-session-supervisor-tick
DONE
Area: Session supervisor: 100 ms TICK idle wakeup
Owned paths: crates/flamewm-wm/src/bin/flamewm-session.rs, crates/flamewm-wm/src/bin/flamewm-session/readiness.rs
Task: Read AGENTS.md and .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep, no git; you may run cargo check -p flamewm-wm once at the end). Find every use of TICK (100 ms) in the supervisor loop and readiness wait. Where the loop only polls for child exit or readiness, replace the fixed-interval wake with a blocking wait on the event (pidfd, signalfd, or child pipe) and keep a long deadline-bound timeout only as a fallback. Steady state (all children healthy) must cost zero periodic wakeups. Keep every restart, crash-loop and readiness behaviour. Add a small pure test for the wait-timeout choice. Log area, change and expected composition shift in docs/perf-loop/LOG.md. Report files changed.
