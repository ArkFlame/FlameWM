# u39 — X11 retained image caches bounds audit (hotlist rank 6)

**Status: UNBUILT.** No `git`, `cargo`, Python, build, test or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Every MiB
figure below is a code budget or a figure already recorded in the hotlist, not a fresh measurement.
No pixel, fingerprint or build-output delta is claimed from this unit beyond the one source cut
described below.

**Target:** hotlist rank 6 (`docs/PERF_HOTLIST.md:20`) — renderer per-app image pixmaps, minimized
capture frames, and titlebar/desktop backdrops.

## Owned files

- `crates/flamewm-wm-x11/src/wm/window_ops.rs` — the single applied cut.
- `docs/swarm/units/u39-x11-caches.md` — this report.

Read-only evidence (inspected, never edited): `docs/PERF_HOTLIST.md`,
`docs/history/FLAMEWM_PASS48_NONWALLPAPER_CAPTURE_AND_RENDERER_MEMORY_2026-10-20.md`,
`crates/flamewm-wm-x11/src/{animation.rs,wm/mod.rs,wm/overlay.rs,wm/manage.rs,wm/transition.rs,wm/construct.rs,frame/titlebar_backdrop.rs}`,
`crates/flamewm-render-x11/src/{app/images.rs,app/mod.rs,app/surface_backdrop.rs,app/window_backdrop.rs,surface_controller.rs,surface_controller/window_state.rs,surface_controller/lifecycle.rs,xrender.rs,backdrop.rs,backdrop/canvas.rs,backdrop/capture.rs}`.
Per the brief, `wm/construct.rs` and `lib.rs` startup-stage lines were not edited.

## Route and evidence ledger

- Loaded skills: `flamewm-source-first`, `flamewm-x11-window-manager`, `flamewm-performance-proof`,
  `flamewm-parallel-execution`. Workspace `AGENTS.md`, `scripts/version` (read only — it calls
  Python, forbidden here), `.opencode/{HARNESS,ROUTES}.md` and
  `.agents/{README,CONTRACTS,JOBS}.md` were read. The handoff overrides the skills' runtime-packet
  and claim protocol: no `runtime/` ledger or claim is created and this unit is UNBUILT.
- Tools used: `read`, `grep`/`rg`, `sed`, `wc`, `edit`, `write`. No `git`, `cargo`, Python,
  build or test command was run.
- Searches/read windows: cache constants and `CaptureCache` (`animation.rs:1-135`), every
  `minimized_captures` call site, `image_pixmap`/`insert_cached_image`/`evict_node_images`
  (`images.rs:294-410`), X11App drop (`app/mod.rs:1250-1285`), xrender picture cache
  (`xrender.rs:155-220`), surface/window backdrop clear paths, surface destroy, and the titlebar
  canvas state. No baseline or candidate memory measurement exists; this is a source audit.

## Findings — authority paths and answers to the brief

**Minimized captures are bounded and freed on destroy and on animated restore.**
`animation.rs:29-31,59-103` caps the cache at 8 entries / 64 MiB and evicts oldest past either cap;
oversized single captures are rejected. The insert happens only at the end of a minimize
(`wm/transition.rs:181-186`). Animated restore consumes the entry with `take` before the overlay
opens (`wm/overlay.rs:25-30`), and unmanage/destroy calls `remove` (`wm/manage.rs:638`). The gap was
the non-animated path: `wm/window_ops.rs:225-232` calls `commit_restore` directly when
`begin_transition` does not start a flight (animations disabled, or overlay creation skipped), so
the cached frame stayed resident until the 8-entry cap or window destruction. A minimized window
cannot be recaptured after it is mapped, so this entry is dead at that point. **Applied cut:**
`wm/window_ops.rs:242` now removes it in `commit_restore`, covering both restore paths. Pixels are
identical: the entry is only ever read by `open_overlay(Effect::Restore)`; a later minimize takes a
fresh capture of the then-current window.

**Renderer pixmaps are server-side and freed with their window.**
`app/images.rs:15-16` currently sets 64 entries / 48 MiB per `X11App` (the hotlist's 16 MiB figure
is stale; the test at `images.rs:673-677` requires room for one 3840x2160 RGBA surface and asserts
the cap stays at or below 64 MiB). Entries hold server pixmaps, a source picture and an optional
mask; `xrender.rs:13,161-205` caches source pictures server-side and drops them when
`take_pixmap_invalidations()` reports the backing pixmap freed. `insert_cached_image`
(`images.rs:341-371`) evicts on both count and byte cap and frees pixmap and mask; per-node stale
images are dropped on key change (`images.rs:325-338`); all remaining images are freed in `Drop`
(`app/mod.rs:1269-1272`), which runs on `destroy_surface` (`surface_controller/lifecycle.rs:179-198`)
when the display is healthy. On a broken display the app is deliberately retained until process
teardown so no backend calls into it.

**Backdrop pixmaps are dropped for unmapped windows.**
Popup unmap clears both surface and node backdrops (`surface_controller/window_state.rs:95-96`,
`surface_controller.rs:589-590`); `app/surface_backdrop.rs:184-188,609-613` and
`app/window_backdrop.rs:570-585` free the retained pixmaps, and `finish_node_backdrops` releases
unpainted node pixmaps each frame. The titlebar canvas
(`wm/frame/titlebar_backdrop.rs:43-64,81-89`) is one global CPU-side `BlurredCanvas`, not a
per-window server cache; it is replaced on generation/layout change and stores a downscaled
(1/16) copy (`render-x11/src/backdrop/canvas.rs:6-50`). Nothing per window is left to drop here.

**No cap scales with memory pressure.** The constants above are fixed, and the only gauge consumer
found (`refresh_image_gauge`, `images.rs:295-298`) records usage rather than driving eviction.
Adding pressure-scaled caps would be a new mechanism, not a pixel-identical cut.

## Rows — file:line, MiB, change, risk

| file:line | MiB | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-wm-x11/src/animation.rs:29-31` | 64 (8 entries) | none; cap kept | Lowering it cannot be pixel-identical: a minimized window cannot be recaptured, so an evicted frame loses the restore effect; needs multi-window restore benchmarks. |
| `crates/flamewm-wm-x11/src/wm/overlay.rs:25-30` | up to 64 consumed | none; animated restore already calls `take` | None — already frees the entry. |
| `crates/flamewm-wm-x11/src/wm/manage.rs:638` | per destroyed window | none; unmanage already calls `remove` | None — already frees the entry. |
| `crates/flamewm-wm-x11/src/wm/window_ops.rs:242` | up to 64 on a bypassed restore | **applied:** `commit_restore` drops any capture left by a non-animated or failed restore | Low: `commit_restore` is also called from `finish_transition` after `take`, where `remove` is a no-op; a later minimize captures fresh pixels. |
| `crates/flamewm-render-x11/src/app/images.rs:15-16,341-371` | 48 per `X11App` (hotlist says 16) | none; 64-entry/48-MiB cap kept | A smaller cap is pixel-identical but forces repeat uploads and drops the one 4K surface the test requires; the hotlist requires a benchmark before touching it. |
| `crates/flamewm-render-x11/src/app/mod.rs:1269-1272`, `surface_controller/lifecycle.rs:179-198` | up to 48 per surface | none; `Drop` frees every cached image/pixmap | Broken-display path intentionally retains the app until process teardown; calling X free there is unsafe. |
| `crates/flamewm-render-x11/src/xrender.rs:13,161-205` | 64 source pictures | none; invalidated with freed pixmaps | Cap cut only trades server picture recreation; no measured win. |
| `crates/flamewm-render-x11/src/surface_controller/window_state.rs:95-96`, `surface_controller.rs:589-590` | per unmapped popup | none; unmap already clears surface/node backdrops | None — already frees the pixmaps. |
| `crates/flamewm-wm-x11/src/frame/titlebar_backdrop.rs:43-64,81-89`, `render-x11/src/backdrop/canvas.rs:6-50` | ~4 recorded | none; one global downscaled CPU canvas | No per-window server pixmap exists; dropping the canvas would force a full recapture and needs a visibility signal. |

PASS — the audit found that animated restore, window destruction, surface teardown, unmapped
backdrops and pixmap invalidation already release their retained images; exactly one real retention
gap remained, a minimize capture left behind when restore bypassed the animation overlay, and it is
closed at `wm/window_ops.rs:242` without changing pixels. No second cut is safe without a
memory-pressure signal or multi-window restore benchmarking, so the remaining caps are documented
and left intact; this unit is UNBUILT and awaits the orchestrator's build, tests and commit.
