# FlameWM 0.1.2 — X11 selection-damage optimization, pass 3

Date: 2026-10-27. Baseline: `FlameWM_0.1.2_X11_Performance_Pass2_27_10_26.tar.gz`.

## Source-grounded plan and decision

The earlier passes compressed the X11 per-window shadow backing memory, restricted selection recomposition to visible right/bottom strips, cached X11 visuals/shapes, and batched root-stacking queries. They remain intact.

`crates/flamewm-wm-x11/src/wm/shadow.rs::on_root_rubber_changed` still processed the union bounding box of both selection rectangles across a visible shadow strip. When the desktop selection grows one pixel at a time, most of that union is an *unchanged* fill, yet `compose_visible_region` processes it and `put_image` transfers it to the X server again. In addition, every changed strip allocated and freed its own X11 graphics context.

Narrowest safe change: when the old and new selections share a top-left anchor and the same fill/border colours, only their moving right and bottom edges can change. Compute those two disjoint edge bands, clip each to the two already-defined visible shadow strips, and recompose just the resulting regions. If either anchor or colours change, retain the existing conservative damage calculation. Reuse one GC per shadow-update batch, and release it on success or after an upload error.

## Changed files

- `crates/flamewm-wm-x11/src/frame/shadow_rubber.rs`: `visible_motion_damage_regions` (pure bounded damage planner) and three Rust unit tests. The existing conservative helper remains unchanged and becomes the fallback.
- `crates/flamewm-wm-x11/src/wm/shadow.rs`: use the new damage planner; move GC creation/free out of the per-region uploader and into the per-shadow update transaction.
- `CHANGELOG.md` and this document.

No new background service, dependency, persistent cache, timer, event source, configuration, API, or compositor state is added. Existing selection semantics, pointer event flow, shadow shading, pixels, frame placement, geometry, focus, taskbar, window gestures, Wayland, and fullscreen logic are not intentionally changed.

## Evidence

- Independent pixel-coverage checker: 2,400 deterministic randomized frame/selection geometries, including negative origins, border widths, growing and shrinking rectangles: **PASS**. It verified that every pixel whose geometric selection colour changed is included in a dirty rectangle. This checker does *not* compile or execute Rust.
- Source Rust tests cover stationary-origin growth/shrink pixel coverage across 2,304 combinations, conservative fallback on moved/recoloured rectangles, and lower dirty-pixel counts for a typical expanding selection. They were **added but not executed**.
- Existing selected Python security, benchmark, frame-interval and idle-wakeup suites: **36/36 PASS**.
- Example analytic frame shadow intersection for a 1-pixel selection expansion: recomposed/uploaded pixels **4,610 -> 946 (79.5% fewer)**. This is *not* total Xorg CPU, graphics bandwidth or a measured process benchmark. Thin selections, moves to another origin, or selections not crossing shadows may see zero benefit.

## Resource estimates and tradeoffs

- Normal idle CPU: essentially unchanged; these functions run only on root rubber-band updates, not a permanent timer.
- Normal idle PSS: effectively unchanged; only tiny stack-local damage arrays and temporary graphics resources are affected, not retained window buffers.
- During repeated selection growth/shrink while shadowed floating windows exist: usually fewer pixels shaded, allocated and transferred to Xorg. Up to four clipped damage regions instead of two can increase the number of small `put_image`/`clear_area` requests on rare cases; one GC per affected shadow avoids multiplying context allocations with that split.
- Window moving/resizing: existing shadow movement/settlement optimizations from passes 1 and 2 remain; no new speedup is asserted.

## Unverified release gates

The container does not have `rustc`, `cargo`, a live Xephyr/Xorg session or GPUs. Therefore this patch is **not production-certified**. On the target development machine run:

```sh
cargo fmt --all -- --check
cargo test -p flamewm-wm-x11 --lib frame::shadow_rubber
cargo test -p flamewm-wm-x11 --lib
cargo build --release --workspace
python3 -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare
```

Then compare before/after in a nested X11 session with identical windows and display settings: select to bottom-right from a fixed anchor, shrink selection, select to top-left (fallback case), cross floating-window shadows, and verify pixel-perfect borders/fill, no stale shadow pixels, no X errors, stable cursor/input responsiveness, and actual `Xorg`/WM CPU and PSS. Do not claim a CPU percentage until that evidence exists.

## Deferred

No polling changes, no new cached window/window-stacking authority, no renderer migration, no cache-size reductions, no Wayland change. Those require separate source-proven budgets and runtime tests.
