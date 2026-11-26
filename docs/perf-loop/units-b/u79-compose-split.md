# Unit u79-compose-split
DONE
Area: Within-thread composition of the nested compositor idle second: which subsystems (render, input, dbus, timers, xwayland) take the share; use existing flamewm-profiler counters/spans, add cheap span counters if missing, expose via an env-gated one-second dump. Then lower the biggest share.
Owned paths: crates/flamewm-wm-wayland, flamewm-profiler
Task: Read .agents/skills/no-adhoc-scripts/SKILL.md and scoped-perf-tests/SKILL.md first. No python/sleep, no cargo/git. Log to docs/perf-loop/LOG-u79-compose-split.md.
