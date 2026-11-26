# Unit u65-terminal-burst
DONE
Area: Terminal 1 s output burst: parse, screen update, redraw rects; coalesce frames to vsync
Owned paths: crates/flamewm-terminal/src/**
Task: Batch nine. Read .agents/skills/no-adhoc-scripts/SKILL.md first (NO python/ctx_execute/sleep). Review recent perf commits here for regressions and fix; find the NEXT biggest hotspot by composition share in normal use and implement at least 3 optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u65-terminal-burst.md.
