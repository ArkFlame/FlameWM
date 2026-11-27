# Unit p60-bench-baseline-script
STATUS: OPEN

Check scripts/bench-baseline (create if missing, bash, reusable): per .agents/skills/scoped-perf-tests/SKILL.md measure per-process RSS (/proc/PID/status VmRSS) and wakeups/s (/proc/PID/status voluntary_ctxt_switches delta over 1 s) for each flamewm-* process in a running session or nested smoke, print a share table, write docs/PERF_BASELINE.md rows. Rows must be reproducible. Do not run it; the orchestrator runs it once.
