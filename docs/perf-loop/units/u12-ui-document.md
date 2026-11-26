# Unit u12-ui-document
OPEN
Area: UI document/layout: skip relayout when unchanged, hash-before-invalidate, avoid Vec/String clones per frame
Owned paths: crates/flamewm-ui-document, flamewm-ui-layout-core, flamewm-ui-core
Task: Scout this area, find top hotspots by composition share (idle wakeups, full redraws, allocations, blocking IO, clones, dead code) and implement at least 3 concrete optimizations keeping visuals/features. No cargo/git. Log changes to docs/perf-loop/LOG-u12-ui-document.md.
