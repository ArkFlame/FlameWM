# Unit u29-render-gpu
OPEN
Area: render-core/render-x11 pixel paths: SIMD-friendly loops, blend/blit fast paths, buffer reuse, avoid full-frame clears
Owned paths: crates/flamewm-render-core/src/**, flamewm-render-x11, flamewm-ui-x11
Task: Third-pass: find the NEXT worst hotspots by composition share (see git log for prior work), implement at least 3 more optimizations; also remove dead code and unwrap/panic paths. Keep visuals/features. No cargo/git. Log to docs/perf-loop/LOG-u29-render-gpu.md.
