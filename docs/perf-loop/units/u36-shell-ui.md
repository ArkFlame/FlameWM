# Unit u36-shell-ui
DONE
Area: Shell UI: dock/taskbar/start menu/notification layout caching, icon_loader memory cap and lazy load
Owned paths: crates/flamewm-shell/src/{dock,icon_loader,notification_center,osd,start*}.rs
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u36-shell-ui.md.
