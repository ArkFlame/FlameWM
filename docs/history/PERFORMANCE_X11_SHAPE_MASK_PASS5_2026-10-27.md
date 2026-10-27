# FlameWM 0.1.2 — Performance Pass 5: X11 shadow XShape traffic

**Source baseline:** `FlameWM_0.1.2_X11_Performance_Pass4_27_10_26.tar.gz`; prior four passes are retained. The production owner is `crates/flamewm-wm-x11/src/wm/shadow.rs`, backed by `crates/flamewm-wm-x11/src/frame/shadow.rs`.

## Source-grounded problem and plan

`Wm::refresh_shadow` rebuilt the bounding X Shape mask for each shadow refresh by emitting **one 1-pixel-high X Shape rectangle per visible row**, including when only wallpaper generation, focus, or position changed. For large windows the source assembled hundreds of rectangles despite the right-hand side of the shadow being a constant-width strip for most rows. Window-local shape geometry depends only on the frame width and height, not its X/Y position or focus/backdrop state.

Plan: (1) coalesce identical adjacent visible row spans into rectangles of variable height; (2) avoid constructing and transmitting the mask if the existing shadow window and its width/height are unchanged; (3) retain the same outer window geometry, input-transparent shape, root snapshot, alpha falloff, stacking, wallpaper handling and selection compositing; (4) verify exact coverage and protocol-call eligibility.

## Changes implemented

- `frame/shadow.rs::shadow_mask_rectangles`: run-length packing of `clip_spans_outside_frame(shadow_spans(...))`. Zero-width spans are omitted, identical vertically adjoining rows become a single X Shape rectangle. Tapers, radius and frame clipping still originate from existing pure geometry primitives; no shadow silhouette redesign.
- `wm/shadow.rs::needs_shadow_shape_update`: a new shadow-window identity or resized frame causes `ShapeOp::SET`; translation, refocus and new shading pixels do not. `refresh_shadow` always performs the required pixmap and window refresh; only the unchanged shape operation is skipped.
- Rust unit tests reconstruct the exact original pixel coverage, detect accidental overlapping packed rectangles, include negative display origins and tiny sizes, and check shape invalidation on move, focus-equivalent state, resizing and window replacement.

## Calculated reductions, not observed CPU benchmarks

| Window size | Original nonempty 1px mask rows | Packed rectangles | Fewer rectangles |
|---|---:|---:|---:|
| 1280×720 | 729 | 32 | 95.6% |
| 1920×1080 | 1089 | 32 | 97.1% |
| 192×80 | 89 | 32 | 64.0% |

At 1280×720, this removes 697 8-byte X Shape rectangle items (~5.4 KiB) each time the mask actually changes. For same-size refreshes, the shape request itself is avoided. Both numbers apply only to shadow shape updates; **neither is a measured whole-desktop CPU or memory percentage**. The actual shadow pixmap remains full-frame and can dominate rendering time and server memory. The steady-state idle CPU and PSS impacts are expected to be near zero; bursts of opening/resizing/refocusing windows should send less mask data and perform less geometry allocation.

## Evidence and remaining gates

- Independent Python geometry surrogate: 2,500 randomized frames, validating each packed rectangle against exact original nonempty row coverage, passed. This is separate from the new Rust unit tests; it is not a Rust compilation check.
- Selected regression suite: 36/36 Python tests passed using `python3 -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare`.
- `cargo` and `rustc` were unavailable in the packaging environment. Rust build, all Rust unit tests, live nested X11 mask/visual verification, and measured CPU/PSS changes are **UNVERIFIED**.

Workstation acceptance commands:

```bash
cargo fmt --all -- --check
cargo test -p flamewm-wm-x11 --lib shadow_shape_tests
cargo test -p flamewm-wm-x11 --lib packed_xshape_mask
cargo test -p flamewm-wm-x11 --lib
cargo build --release --workspace
python3 -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare
```

Compare nested X11 desktop screenshots and `XShape` masks at 1280×720, 1920×1080, negative-offset outputs, focus changes, same-size window moves, resizing, fullscreen transitions and active desktop selection. Compare per-process CPU and PSS in identical sessions; do not extrapolate mask-rectangle savings to total CPU savings.

## Deferred (not silently changed)

The shadow's full-frame 24-bit pixmap, synchronous desktop readback on re-shade, and full-window `put_image` remain higher-potential Xorg CPU/server-memory targets. Converting them to split-strip windows/pixmaps changes shadow-window topology and clipping semantics; doing so without a Rust compiler and live X11 test would violate the non-breaking constraint. Idle timer/Wayland changes are not made in this pass.
