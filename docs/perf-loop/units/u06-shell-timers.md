# Unit u06-shell-timers
OPEN
Area: Shell: idle wakeups in osd, notification_center, launch_feedback, weather_host, usage_host, progress_gate, quick_controls -> event driven/coalesced
Owned paths: crates/flamewm-shell/src/{osd,notification_center,launch_feedback,weather_host,usage_host,app/progress_gate}.rs, quick_controls/**
Task: Scout this area, find the top hotspots by share of CPU/GPU/RAM composition (idle wakeups, full redraws, allocations, blocking IO, clones), and implement at least 3 concrete optimizations that keep visuals and features. Simplify and delete dead code. Do not run cargo or git. Append one line per change to docs/perf-loop/LOG-u06-shell-timers.md.
