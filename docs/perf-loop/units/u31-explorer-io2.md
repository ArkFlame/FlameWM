# Unit u31-explorer-io2
DONE
Area: Explorer I/O: copy/move/delete/trash/size-calc progress on worker threads, cancel tokens, bounded channels
Owned paths: crates/flamewm-explorer/src/{jobs,search,refresh,remote,dndext}.rs
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u31-explorer-io2.md.
