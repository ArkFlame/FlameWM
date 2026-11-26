# Unit u35-shell-loop
DONE
Area: Shell: event loop wakeups, timers merged into one deadline queue, popups skip work when closed
Owned paths: crates/flamewm-shell/src/{main,runtime,event_router,app}/**
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u35-shell-loop.md.
