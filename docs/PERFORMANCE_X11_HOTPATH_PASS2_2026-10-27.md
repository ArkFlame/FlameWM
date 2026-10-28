# FlameWM 0.1.2 — X11 interactive hot-path optimization, pass 2

## Source and scope

Continues the previous complete archive `FlameWM_0.1.2_X11_Shadow_Performance_Optimized_27_10_26.tar.gz`. The 2026-10-27 pass that reduced retained X11 shadow CPU buffers and compacted selection compositing is preserved without modification. Active runtime is independent Rust FlameWM.

Files touched: `crates/flamewm-render-x11/src/app/mod.rs`, `crates/flamewm-render-x11/src/app/render.rs`, `crates/flamewm-wm-x11/src/wm/shadow.rs`, this report, and `CHANGELOG.md`.

The existing renderer's event coalescing, retained paint cache, `RedrawPlan::Skip`, and default full-paint behavior are deliberately preserved. `FLAMEWM_RENDER_REGION=1` stays **opt-in**; source explicitly warns that its Xft region path can clip text/terminal updates. No new always-on timer, thread, background service, dependency, scene owner or parallel state authority is added.

## Plan and implemented changes

1. **Cache immutable ARGB visual discovery per X11 display.** `X11App::new_surface` already matched the 32-bit visual once to decide the alpha mode. Retain that `Visual*` in the app, and reuse it for each text-shadow blit or ARGB image upload instead of calling `XMatchVisualInfo` again. X11 visuals do not change during the lifetime of an open Display. Keep the previous missing-visual error/fallback behavior, and return shadow scratch to the per-thread cache on the no-visual path.

2. **Do not rebuild invariant fixed XShape masks on paint.** Ring and rounded-dock masks depend on window size and their own fixed parameters, not the document/hover/text state. Cache an exact typed tuple of `(width,height,shape_kind,radius,border)` after successful shape application. Skip expensive row-span reconstruction on subsequent paints only if both the exact tuple and the existing backend-success `shape_applied` latch match. Window resizes, ring/radius changes, clipped slide masks and failed backend operations require recalculation. Alpha-silhouette overrides and general command-coverage masks are *not* memoized: they can depend on runtime image/paint contents.

3. **Skip WM shadow-generation hashing when no shadow can exist.** If there are zero wanted floating frames and zero retained shadow windows, return after cleanup. The first subsequently eligible frame has no prior shadow and must be freshly shaded, so omitting generation tracking while empty cannot produce stale pixels. This avoids allocating/hashing/sorting window membership, geometry and drag snapshots on every unrelated reactor wake while there are no floating shadows.

4. **Query X11 root stacking once for a shadow refresh batch.** `ShadowAction::Reshade` previously queried the entire root child list synchronously, once per frame. Resolve the lowest *managed* frame once per synchronous reconciliation batch, then reuse that sibling for all shadow windows. Managed stacking order cannot change within the non-reentrant pass, and shadow windows are not included in the managed-frame filter. If the query fails or returns no managed frame, retain the existing per-frame `own` fallback and attempt the query again on the next frame, rather than caching an invalid fallback.

## Verification

- Added Rust unit tests of the exact fixed-shape key, invalidation decision, and successful/failed cached sibling lookups. These tests exercise production helper functions.
- Selected existing Python security, benchmark, frame-interval and idle-wakeup regression suites: **36/36 passed** (`python -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare`).
- Source checks confirm ARGB matching occurs once at `X11App` creation, fixed-mask early return has a backend-validity gate, the no-shadow exit is guarded by both empty sets, and there is one `query_tree(root)` site for batch sibling selection.
- Rust compiler, `cargo test`, release build, Xephyr/DRM runtime, pixel-diff screenshots and whole-desktop PSS/CPU measurements are **not available in this environment** and remain **UNVERIFIED**. Python tests cannot substitute for Rust compilation or visual proof. The pass is source-patched, not certified for production.

## Estimated savings, conditional on usage

All estimates are analytical, not captured profiler data. No wakeup interval is changed. Idle CPU in an already-sleeping reactor is expected to stay near zero; this pass is **not expected to materially lower idle PSS** (approximately zero MiB). The added two per-instance cached fields are only tens of bytes.

- During a simultaneous refresh of `N` floating shadows: managed-root stacking-tree queries fall from **N to 1** if the first query succeeds (for 10 windows, 10 -> 1, a 90% decrease in that specific X11 query count). Does not eliminate other shadow uploads/get-image calls.
- For a fixed rounded dock or fixed ring: expensive span construction/sorting and backend shape apply checks occur only on the first valid shape, on parameter/size change, or after clip/backend invalidation; stationary repeated paints do **not** rebuild the spans. For a 1080-row ring this avoids regenerating up to approximately 1080 rows per *unchanged* paint. Not a predicted whole-desktop CPU percentage.
- ARGB visual probes during eligible paint calls fall from one per blit to **zero** after display initialization; lifetime remains the same.
- With all windows fullscreen/tiling and no eligible floating shadows, the WM shadow no-op pass now avoids list allocations, sorting and hashes entirely. How often it is called depends on real input/provider traffic.

The practical magnitude of total CPU reduction could be negligible on a quiet session; the prior strip-retention pass remains the source of the documented large RAM win. Measure before and after under the same X11 config using `tools/bench-flamewm-cpu.py`, and compare Xorg/WM CPU and PSS with profiler spans enabled and with profiling disabled.

## Minimum hardware validation before merge

```bash
cargo fmt --all -- --check
cargo test -p flamewm-render-x11 --lib static_shape_tests
cargo test -p flamewm-wm-x11 --lib reconcile_batch_tests
cargo test -p flamewm-wm-x11 --lib frame::shadow
cargo build --release --workspace
```

Use a **separate nested Xephyr display**, not the active host desktop, for the repository's `tools/bench-flamewm-cpu.py` XTEST workload. Compare idle, desktop rubber selection, one and ten floating windows moving/resizing, fullscreen, changing dock radius/taskbar size, masking and slide dismissals, icons/text shadows, and a server with no ARGB32 TrueColor visual. Verify unchanged pixels and event ordering. Restore or rollback this pass if pixel parity, stability, or latency regresses.

## Deferred

- D-Bus/logind's native Wayland 20 ms session polling: making it event-driven needs a verified file-descriptor/signal hookup; simply increasing its interval can delay VT pause acknowledgement, so it was **not** changed.
- Full X11 shape coverage cache keyed by general paint commands: interactive opacity/style changes can alter the mask; coarse revision-based invalidation could create stale silhouettes. Only exact invariant shapes were memoized.
- CPU and memory claims for the whole session: cannot be made without runtime measurements.
