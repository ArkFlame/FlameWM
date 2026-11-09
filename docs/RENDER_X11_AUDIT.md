# flamewm-render-x11 presenter audit

Base: `origin/main` 9a0dae1b. Branch `perf/render-x11-presenter-2`. Dev profile, nested session via `scripts/perf/fm.sh`.

## Measurement

Harness: `scripts/perf/fm.sh <bin-dir> all --json`, the real nested session with XTEST marquee and hover. Each build ran 3 times, interleaved. Binaries: base is `origin/main` 9a0dae1b, rebuilt from a touched source tree because shared fingerprints reused the candidate build. Cand is `70bccdd1`. Medians of 3 runs.

| Process | idle CPU% base / cand | marquee CPU% base / cand | hover CPU% base / cand | PSS MiB base / cand |
|---|---|---|---|---|
| flamewm-wayland | 5.99 / 6.10 | 96.60 / 96.24 | 81.73 / 80.35 | 94.6 / 93.6 |
| flamewm-desktop | 9.60 / 10.59 | 13.74 / 13.32 | 7.76 / 7.34 | 24.0 / 23.6 |
| flamewm-shell | 8.73 / 13.46 | 1.79 / 1.79 | 0.20 / 0.00 | 18.7 / 17.9 |
| Xwayland | 0.37 / 0.37 | 3.98 / 3.98 | 1.59 / 1.39 | n/a |
| total PSS | | | | 175.4 / 174.4 |

Reading: differences are inside the run-to-run spread. The shell idle CPU moved from 8.7 to 13.5 with no change on its path, so it is noise. Nothing here shows a win.

Attribution gap: the changes fire only on opaque cache-miss uploads through `upload_opaque_*`. No counter confirms they run in this session. The benefit is unproven.

Harness notes:
- `scripts/perf/settings_session.sh` and `fm.sh` do not set `FLAMEWM_WAYLAND_WIDTH`/`HEIGHT`. Nothing in `crates/` reads those variables.
- `scripts/perf/backdrop_cpu.py` does not exist on `origin/main`.
- The earlier `.rwr` path was wrong: the shell `.rwr` renders black and produced 0% CPU. Do not use it for UI CPU.
- Screenshots: not taken on the fixed harness.

## Findings (file:line on e77035db)

| # | file:line | issue | est. win | risk | fixed? |
|---|---|---|---|---|---|
| 1 | app/images.rs:226 (`upload_opaque_argb32_image`) | Per-pixel `rgb_to_pixel` + `XPutPixel` for every opaque cache-miss upload. Runs for external opaque blits (`external_drawable.rs:581`, `:739`). | High CPU on icon or image cache misses. Unmeasured. | Low: byte-equal to per-pixel result for standard 0xRRGGBB, 32-bit, host-order XImage. Other layouts keep the old loop. | **Yes** (commit 1). Unit tests cover equivalence and gating. Not runtime-measured. |
| 2 | app/images.rs:523 (`upload_opaque_fallback`) | Same per-pixel `XPutPixel` loop, in the opaque fallback path. | Medium CPU on fallback. | Low, same rule as #1. | **Yes** (commit 70bccdd1). Covered by the packed-word equivalence test. Not runtime-measured. |
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

1. `c0db984f` (was `cc9d2d32`): `perf(render-x11): pack opaque uploads row-wise`, finding #1. Gated on `4bdebf3c`, pushed.
2. `4bdebf3c`: audit doc, first version. Superseded by this file.
3. `70bccdd1`: `perf(render-x11): pack opaque fallback rows`, finding #2. Gate: pending at time of writing.
4. This revision of the audit doc.

Not done: finding #3 (GC per upload), #4 (damage-limited present), #13 (shrink policy), and the file splits. The `tools/architecture_guard.py` owner-list update is not needed yet.

## Verification

- `cargo test -p flamewm-render-x11 --lib images`: 12 passed on `70bccdd1`, exit 0.
- Gate `scripts/gate flamewm-render-x11` on `4bdebf3c`: `GATE_DONE all=0`, all four checks exit 0. This gate covered `c0db984f` and the doc.
- Gate on `70bccdd1`: see the gate log in the report.
- Runtime CPU/PSS change: not shown. See Measurement.

## Next steps

1. Add a counter or trace for the upload fast paths, so a run proves they fire.
2. Finding #3, GC reuse per upload, with a clear owner.
3. Finding #13, shrink policy with hysteresis, with a test.
4. Finding #4, damage-limited present, only after an expose and overlap equivalence test.
