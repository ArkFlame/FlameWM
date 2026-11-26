# Unit u05-terminal-pty
OPEN
Area: Terminal: batched PTY reads, bounded scrollback ring buffer, suggest.rs cost, session wakeups
Owned paths: crates/flamewm-terminal/src/{pty,session,suggest,settings}.rs
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u05-terminal-pty.md.
