# Unit u54-terminal-clone
DONE
Area: Terminal: remove clones of lines/cells/strings in render and scrollback paths; Arc/Cow where shared
Owned paths: crates/flamewm-terminal/src/**
Task: Batch seven. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u54-terminal-clone.md.
