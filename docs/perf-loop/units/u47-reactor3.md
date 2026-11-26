# Unit u47-reactor3
DONE
Area: reactor + dbus-reactor + control-*: epoll fan-in, fewer threads, bounded queues, drop idle timers
Owned paths: crates/flamewm-reactor, flamewm-dbus-reactor, flamewm-control-*
Task: Batch six. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code and unused warnings (cargo reported unused imports/dead fns in flamewm-desktop, flamewm-explorer, flamewm-settings). Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u47-reactor3.md.
