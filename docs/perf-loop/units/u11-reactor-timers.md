# Unit u11-reactor-timers
OPEN
Area: flamewm-reactor timeline/child_pipe/dbus-reactor: replace polling with fd/epoll/timerfd, coalesce wakeups
Owned paths: crates/flamewm-reactor/src/**, flamewm-dbus-reactor/src/**
Task: Scout this area, find top hotspots by composition share (idle wakeups, full redraws, allocations, blocking IO, clones, dead code) and implement at least 3 concrete optimizations keeping visuals/features. No cargo/git. Log changes to docs/perf-loop/LOG-u11-reactor-timers.md.
