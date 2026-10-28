# FlameWM 0.1.2 — Performance Pass 6: selection-shadow buffers and X11 GC reuse

**Input:** `FlameWM_0.1.2_X11_Performance_Pass5_27_10_26.tar.gz`. Prior passes are preserved. Only the X11 desktop-selection/shadow paint hot path is changed.

## Source audit and implementation

`Wm::on_root_rubber_changed` previously allocated one pixel vector per clipped damage rectangle and one graphics context per affected floating-window shadow. A selection update can cross multiple frame shadows and affect up to four regions per frame. Passes 1–5 already reduced damage and X Shape traffic; those paths remain intact.

1. `shadow_rubber::compose_visible_region_into` now clears/refills a caller-owned `Vec<u8>` using the same pixel arithmetic. The original owned-return API still exists and delegates to it.
2. `on_root_rubber_changed` reuses one temporary pixel buffer across every damaged strip/window within an event. That buffer is dropped afterward rather than retained in the idle process.
3. A lazily created X11 GC is shared across all shadow pixmap uploads and freed on success or error. If no shadow is affected, no GC is created.
4. A fully-contained compact strip is chosen once per region rather than once per pixel. Nonstandard spanning regions retain the original lookup.

The shadow pixel arithmetic, X11 upload dimensions, clipping, ordering, taskbar, window gestures, overlays, Wayland compositor, and X Shape masks are unchanged.

## Estimated benefit (not a CPU benchmark)

| Per selection property update | Pass 5 | Pass 6 |
|---|---|---|
| GC create/free pairs for N affected shadows | N | 1 for N > 0 |
| Temporary output buffer allocations for R damaged regions | Up to R | Up to 1 |
| Compact strip selection | Per pixel | Per region (with fallback) |
| Uploaded/shaded pixel count | Unchanged | Unchanged |
| Idle retained scratch buffer | None | None |

For ten affected windows, GC create/free requests fall from 20 to two (90% fewer), independent of the region count. When there are two damaged regions per window, allocation opportunities fall from up to twenty to one. Actual allocator savings depend on capacities. The impact on CPU is expected to be limited to active selection movement. Idle CPU and steady-state PSS should remain essentially unchanged. These are derived operation counts, **not measured CPU or memory reductions**.

## Verification

- Independent randomized compact-strip index parity: **1,018,240 comparisons passed** over 8,000 random frame layouts, including negative monitor offsets.
- Existing Python regression suite: **36/36 passed**.
- New Rust unit test verifies pixel identity against the original independent full-frame reference when reusing the output allocation across focus, overlay, size and empty-region changes.

**Unavailable:** `cargo`, `rustc`, a live X11 display and graphical/CPU/PSS benchmark runtime on this host. Compilation and visual tests remain unverified. Before merge, run:

```bash
cargo fmt --all -- --check
cargo test -p flamewm-wm-x11 --lib reusable_composition_matches_owned_output_after_grow_shrink_and_style_change
cargo test -p flamewm-wm-x11 --lib
cargo build --release --workspace
python3 -m unittest -q tools.test_bench_flamewm_session_security tools.test_flamewm_bench_memory tools.test_flamewm_frame_interval tools.test_flamewm_idle_wakeups_compare
```

Compare Pass 5 versus Pass 6 on the same nested 10-second desktop-selection workload and floating-window count. Verify screenshot/pixel parity, window dragging, focus, and multi-monitor operation before production merging.

## Deferred

Full-frame X11 shadow pixmaps and synchronous initial desktop readbacks/uploads remain higher-cost candidates. Changing their topology without a native compiler and live X11 test would risk visible regressions. No speculative idle-timer, Wayland, or architecture refactor was performed.
