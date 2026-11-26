# Unit u39-ui-doc2
DONE
Area: ui-document/ui-layout-core/render-compiler: incremental layout, dirty flags, style resolution caching, string interning
Owned paths: crates/flamewm-ui-document, flamewm-ui-layout-core, flamewm-render-compiler
Task: Batch five. Review the recent perf commits in this area (git log -p -n 8 -- <paths>) for regressions and bugs (fix them), then find the NEXT worst hotspots by composition share and implement at least 3 more optimizations; delete dead code. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u39-ui-doc2.md.
