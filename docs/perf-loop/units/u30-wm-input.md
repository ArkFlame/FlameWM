# Unit u30-wm-input
OPEN
Area: desktop/ui-controller: input event coalescing (mouse move), hover refresh cost, resize throttling
Owned paths: crates/flamewm-desktop/src/**, flamewm-ui/src/**
Task: Third-pass: find the NEXT worst hotspots by composition share (see git log for prior work), implement at least 3 more optimizations; also remove dead code and unwrap/panic paths. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u30-wm-input.md.
