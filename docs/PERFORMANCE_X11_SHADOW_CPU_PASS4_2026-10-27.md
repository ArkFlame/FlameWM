# FlameWM 0.1.2 — Performance Pass 4: X11 shadow reconciliation and per-pixel CPU

**Source baseline:** `FlameWM_0.1.2_X11_Performance_Pass3_27_10_26.tar.gz` (complete uploaded source). **Date:** 2026-10-27. Active runtime: independent Rust/X11 WM. Prior shadow-strip memory savings, fixed-shape caching, batch stacking query, and selection damage are preserved.

## Plan and source findings

1. `crates/flamewm-wm-x11/src/wm/shadow.rs::reconcile_frame_shadows` executes after event processing on each X11 WM reactor turn. With floating windows present, it built an owned `Vec<WantedShadow>`, and `advance_shadow_generation` built fresh owned `Vec`s for visible membership, output geometry and settled window geometry. All of them were dropped at the end of each turn. This causes allocator traffic during pointer motion, live moving, and unrelated WM wakeups even when no scene state changed.
2. `frame/shadow.rs::keep_at` evaluated `f32` falloff for each pixel of the right/bottom visible shadow, both when first shading a window and when `frame/shadow_rubber.rs::compose_pixels` recomposed changed desktop selection pixels. Because the shadow's offset and spread are constants, the integer distance has **17 relevant attenuation values** (0..16, with every larger distance identical at 256).
3. Retain exact hash ordering and values, generation transition rules, shadow visibility and X11 effect ordering. Change neither user-visible mechanics nor packet/event scheduling. No cache of window states, no new timers, X11 calls or daemon processes.

## Implementation

- `wm/shadow.rs`: Reuse the wanted-frame list across turns, plus retained buffers for sorted membership, sorted settled geometry, output layout and drag states. `mem::take` temporarily owns the wanted list during native X11 calls and restores it after a successful batch. Two drag vectors swap their backing allocations while retaining the original semantics: bump generation **only** on the end of a gesture or a previously meaningful scene change. Retain the *same* `hash_of` logic and sort order; use borrowed buffer references in the hash tuples, whose `Hash` implementations are value equivalent to owned `Vec`s.
- `frame/shadow.rs`: Generate a 17-word shadow-attenuation lookup *locally per shading operation* by calling the original `keep_at` reference function at distances 0..16. Pixel loops use integer-only index lookup, retaining the original 8-bit rounding and palette/focus factors. Keep `keep_at` and the original full-frame shader unchanged as parity oracles.
- `frame/shadow_rubber.rs`: Reuse that exact lookup for live selection compositing, including recolored and moved selections using the prior conservative damage fallback.
- Source Rust tests compare borrowed-vs-owned snapshot hashing, drag lifecycle generation transitions, and integer table vs original floating-point `keep_at` over negative/positive display origins and multiple frame geometries.

## Expected savings — analytical, not measured

| Situation | Previous repeated work | After | Expected effect |
|---|---|---|---|
| X11 WM reactor turn with floating frames | Up to four nonempty `Vec` allocation/deallocation cycles for wanted windows, membership, geometry and outputs (plus drag-vector allocation only when dragging) | Reuse heap capacities after initial warmup and ordinary grow/shrink | Less allocation and allocator CPU; sorting and hash work still occurs |
| Repainting shadow pixels after selection change | `f32` attenuation expression per pixel | 17 `keep_at` calls per updated region, then integer lookup per pixel | Reduced arithmetic for sufficiently large damaged strips |
| Initial creation/re-shading of frame shadow | `f32` attenuation expression per visible pixel | 17 lookups initialized, then integer lookup per visible pixel | Lower CPU in the shading loop; unchanged X11 get/put image costs |
| Normal idle with reactor blocked | No repeated WM shadow reconciliation | Same | No meaningful expected idle CPU reduction |
| Normal idle memory | Short-lived allocation buffers | Four/five retained small-capacity vectors | **Small increase**, typically hundreds of bytes to a few KiB depending on window count; avoids allocation churn |

Illustration: for 5 floating frames and 2 outputs, each ordinary reactor turn previously allocated and freed ~4 nonempty vectors; assuming 60 turns/s this is ~240 allocation/free pairs/s avoided once capacities warm. **This is an analytical count of just these allocations, not a claim of measured CPU percentage.** Peak memory is not reduced. No per-window display-side X pixmap or IPC call is removed.

## Verification and release gate

- Existing selected Python security/benchmark/idle wakeups tests: 36/36 passed (`python3 -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare`).
- Independent float32 surrogate checked 160,000 sampled shadow-pixel attenuation factors, including negative monitor origins: all matched (this **does not** execute Rust).
- New Rust source unit tests were added but **not executed**: `cargo` and `rustc` unavailable in the current container.
- Native release build, visual X11 parity, actual CPU/PSS reductions and input latency: **unverified**.

On the user's Linux workstation:

```sh
cargo fmt --all -- --check
cargo test -p flamewm-wm-x11 --lib scratch_equivalence_tests
cargo test -p flamewm-wm-x11 --lib integer_keep_table
cargo test -p flamewm-wm-x11 --lib
cargo build --release --workspace
python3 -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare
```

Compare before/after using identical X11 session, window count, resolution, theme and workload. Run `tools/bench-flamewm-cpu.py` against an isolated nested X11 display (not the host desktop), and sample process PSS via `/proc/<pid>/smaps_rollup`. Scenarios: plain idle, rapid pointer sweeps, opening/moving/resizing 5 floating windows, stationary-anchor selection growth/shrink, selection crossing shadows, and monitor/desktop changes. Confirm identical focus/selection/shape pixels and window move/snap responsiveness. Any violation mandates reverting this pass, not weakening regression tests.

## Deliberately excluded

No extra retained scene state authority, no global cache holding full pixel buffers, no Xorg render configuration change, no disabled FPS pacing, no Wayland modifications or unrelated UI rework. Actual Xorg CPU, driver CPU and shell/desktop RAM may dominate; this optimization does not assert meaningful whole-desktop percentage savings without a native benchmark.
