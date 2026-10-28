# FlameWM 0.1.2 — Performance pass 7: streaming shadow rows

**Source baseline:** `FlameWM_0.1.2_X11_Performance_Pass6_27_10_26.tar.gz` (passes 1–6 preserved).

## Plan and source findings

1. **Owner:** `crates/flamewm-wm-x11/src/frame/shadow.rs`. Both `shadow_mask_rectangles` and `shade_visible_shadow` called `clip_spans_outside_frame(shadow_spans(area), area, frame)`. That created an entire `Vec<(u32, u32, u32)>` of frame-height rows **twice** (first unmodified, then clipped) for a single-pass consumer.
2. **Change:** Add one lazy row generator and a lazy frame-clip adapter. Reuse the row generator in the existing `shadow_spans` API; feed mask rectangle packing and visible-pixel shading directly from the clipped iterator. Keep `clip_spans_outside_frame` and `shadow_spans` public signatures and behavior unchanged. No additional state, thread, data migration, API change or dependency.
3. **Preservation:** Preserve silhouette, taper, radius, coordinate origin (including negative multi-monitor coordinates), 24-bit image data, shading opacity, darkening order, selection blending, X Shape rectangle order, pixmap lifetime, Wayland behavior and fullscreen/drag mechanics. The mask output vector and X11 requests remain unchanged.
4. **Verification:** Compare lazy rows against the original owned clip path (including degenerates); compare packed rectangles and all bytes in shaded output against the old loop using the same sampled pixel factor. Run existing Python security/benchmark regressions. Native build and X11/Xephyr rendering remain mandatory deployment gates.

## Calculated resource effect (not measured)

`shadow_spans` returned a `Vec` of `height + SPREAD` tuple rows; `clip_spans_outside_frame` allocated another. Each `(u32,u32,u32)` is 12 bytes. For one 1280×720 floating window (730 rows), this avoids **two intermediate Vec heap allocations** and ~17,520 bytes of *temporary tuple payload* per mask construction or visible-shading call. For a 1920×1080 frame (1090 rows), the payload is ~26,160 bytes. This is temporary memory and allocator traffic, not a persistent-PSS reduction. Total shadow pixmap memory, X11 image upload bytes, number of shaded pixels, and protocol requests are unchanged. CPU improvements should be small and restricted to the affected operations; unchanged when idle without shadow refreshes.

## Verification performed here

- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare` — **36/36 passed**.
- Independent randomized geometry differential — **6,000 layouts / 2,577,763 rows compared**, including zero/small dimensions and negative monitor origins. The reference emulates original row generation and clipping; the tested candidate emulates lazy generation.
- Rust regression tests added: `streamed_rows_match_owned_clipped_spans_including_edge_cases`, `streaming_preserves_mask_rectangles_and_shaded_pixels`.

**Unavailable here:** Rust toolchain (Cargo/rustc), live X11 desktop, CPU profiler and graphical display oracle. These source-level checks do not prove the compiled product's behavior or quantify whole-session savings.

## Workstation gate before release

```bash
cargo fmt --all -- --check
cargo test -p flamewm-wm-x11 --lib streamed_rows_match_owned_clipped_spans_including_edge_cases
cargo test -p flamewm-wm-x11 --lib streaming_preserves_mask_rectangles_and_shaded_pixels
cargo test -p flamewm-wm-x11 --lib
cargo build --release --workspace
```

Re-run the nested desktop benchmark and observe per-process PSS, Xorg/X11 CPU, window opening/shadow refresh, resize, selection rectangle, multi-monitor coordinates and visual parity. Compare against Pass 6 under matching conditions. Defer full-frame pixmap topology changes or idle-event state-machine redesign until native verification is available.
