# flamewm-render-x11 presenter audit

Base: `origin/main` e77035db. Branch `perf/render-x11-presenter`. Dev profile, Xvfb 1280x800x24.

## Measurement status

| Item | Status |
|---|---|
| Harness | `/tmp/claude-1000/rx11-bench/{bench.py,run.sh}` (outside repo). PSS from `smaps_rollup`, CPU from `/proc/PID/stat`, XTest hover/scroll via python-xlib. |
| Target | `.target/flamewm/flamewm-shell.rwr` only. No settings, task-manager or dock `.rwr` exists in the repo or the shared `.target`. |
| Baseline (3 runs, 20 s each) | PSS median 13.6-13.9 MiB across idle/hover/scroll. CPU 0.0% in all 9 samples. |
| Screenshot | `/tmp/claude-1000/rx11-bench/base-idle.png` is solid black. The surface is not visibly rendering in this harness. |
| Conclusion | **Baseline is not representative. No before/after claim is made.** PSS is valid as a process-size sample only. CPU and input response are not measured. |

Blockers for the acceptance criteria:
- `scripts/gate` does not exist in this checkout. The gate command in the brief cannot run.
- Settings, task manager and dock `.rwr` inputs are not built here.
- The shell `.rwr` shows black, so XTest input produces no repaint work. The hover and scroll CPU figures are therefore not discriminating.
- `docs/AGENT_PLAYBOOK.md`, `docs/PERF_AUDIT.md` and `docs/DEBT_REGISTER.md` do not exist. The audit uses `docs/PERFORMANCE.md` instead.

## Findings (file:line on e77035db)

| # | file:line | issue | est. win | risk | fixed? |
|---|---|---|---|---|---|
| 1 | app/images.rs:226 (`upload_opaque_argb32_image`) | Per-pixel `rgb_to_pixel` + `XPutPixel` for every opaque cache-miss upload. Runs for external opaque blits (`external_drawable.rs:581`, `:739`). | High CPU on icon or image cache misses. Unmeasured. | Low: byte-equal to per-pixel result for standard 0xRRGGBB, 32-bit, host-order XImage. Other layouts keep the old loop. | **Yes** (commit 1). Unit tests cover equivalence and gating. Not runtime-measured. |
| 2 | app/images.rs:523 (`upload_opaque_fallback`) | Same per-pixel `XPutPixel` loop, in the opaque fallback path. | Medium CPU on fallback. | Low, same rule as #1. | No. Next candidate. |
| 3 | app/images.rs:155, 234 | `XCreateGC`/`XFreeGC` per upload. 19 `XCreateGC` sites in the crate. | Small; GC churn per miss. | Low. | No. Needs a per-pixmap or per-display GC owner. |
| 4 | app/render/present.rs:162-185 (`present_scene`) | Full-window `XCopyArea` backbuffer to window on every present, plus `XFlush`. | Medium: copies the whole surface even for small damage. | **High**: expose and overlap correctness depends on the full copy. Needs a damage-rect proof first. | No. |
| 5 | app/render/paint_full.rs:34-66 | `paint_full` clears and repaints every retained command each frame. Region paint exists only for some paths (`region_equivalence_tests.rs`). | High, but architectural. | High. | No. |
| 6 | app/render/paint_full.rs:43 (`clear_scene`) | Full clear before repaint on the full path. | Small to medium. | Medium: transparency depends on the clear. | No. |
| 7 | surface_controller.rs:229, overlay.rs:989 | `XGetGeometry` round-trips. Both sit on popup show or reveal, not per frame. | Negligible. | Low. | No. Not hot. |
| 8 | external_drawable.rs:50 | `XGetGeometry` in drawable setup, not per frame. | Negligible. | Low. | No. |
| 9 | xcomposite.rs:362 | Strip staging `XPutImage` per capture strip, bounded by `MAX_*` constants. | Unmeasured. | Medium. | No. |
| 10 | app/events/event_loop.rs:156 | Motion coalescing exists but only for contiguous same-window `MotionNotify`. Matches the contract in `docs/PERFORMANCE.md`. | None. | n/a | Already done. |
| 11 | app/events/event_loop.rs:623 | Size reports coalesce to the latest settled size. | None. | n/a | Already done. |
| 12 | app/images.rs:15-16 | Image cache is bounded (64 entries, 48 MiB). | None. | n/a | Already bounded. |
| 13 | render-x11 (all) | No shrink-after-idle policy for retained scratch buffers. `accel/mod.rs:341 trim_heap` is out of scope and only trims the heap. | Unmeasured. Memory stays at peak. | Medium: needs hysteresis. | No. |
| 14 | xft.rs (1434 lines) | No byte-capped glyph or font cache found by grep. `MAX_MASK_BATCH_HEIGHT` at xft.rs:147 is the only size bound found. Not audited in depth. | Unknown. | Unknown. | No. |
| 15 | Per-frame `Vec`/`String`/`format!` in the six named files | 62 grep hits in events.rs, mod.rs, surface_controller.rs, overlay.rs, xft.rs, external_drawable.rs. Most are setup or error paths. Not sorted by hotness. | Unknown. | Low per site. | No. |

## Commits

1. `perf(render-x11): pack opaque uploads row-wise` (finding #1). Changes `app/images.rs` only. Generalises `copy_argb32_rows` to `copy_argb32_rows_with`. Adds `packed_rgb_stride`, which gates the fast path, and two regression tests.
2. This audit doc (`docs/RENDER_X11_AUDIT.md`).

Not done: the split of xft.rs, surface_controller.rs, app/mod.rs and app/events.rs. Not done: `tools/architecture_guard.py` owner-list updates.

## Verification

- `cargo test -p flamewm-render-x11 --lib images`: 11 passed, 0 failed (exit 0, run without the pipe mask).
- rustfmt applied to `app/images.rs` only. Hunks confined to the edited regions.
- Full crate test exit: see the report.
- Gate: not run. `scripts/gate` is absent.
- Runtime, screenshot and CPU before/after: not verified.

## Next steps

1. Fix finding #2 with the same packed path.
2. Fix the measurement gap: build or locate settings, task-manager and dock `.rwr`, then drive real hover and scroll input.
3. Design damage-limited `present_scene` (finding #4) behind an expose and overlap regression test.
4. Add an idle shrink policy with hysteresis (finding #13).
