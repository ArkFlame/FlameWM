# Unit u62-explorer-remote
DONE
Area: explorer remote-fs: network listing async with timeouts, cache dir listings, never block UI on mounts
Owned paths: crates/flamewm-remote-fs, crates/flamewm-explorer/src/remote*
Task: Batch eight. Review recent perf commits here (git log -p -n 6 -- <paths>) for regressions/bugs and fix; then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u62-explorer-remote.md.
