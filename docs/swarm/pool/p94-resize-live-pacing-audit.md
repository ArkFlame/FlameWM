# Unit p94-resize-live-pacing-audit
STATUS: DONE (audit only, 0 production edits, UNBUILT: no build or test run)

User report: resizing with the resize rectangle froze the machine. Outline ring pacing landed (snap_preview.rs, UNBUILT until slot). Audit the REST of the resize path for per-event unbounded work on Wayland (crates/flamewm-wm-wayland: input/pointer.rs, input/buttons.rs, render/mod.rs, state/mod.rs) and X11 (crates/flamewm-wm-x11 wm/effects.rs, frame/*): per-motion allocations, full-scene rebuilds, repeated output lookups, client configure storms, texture uploads. For each, give file:line, cost per event, and a bounded fix. At most 2 production edits, UNBUILT; prefer pacing by frame interval.

## Result

No production edits. Checks not run (no build/test per instruction). UNBUILT.

| # | file:line | path | cost per pointer event | verdict / bounded fix |
| --- | --- | --- | --- | --- |
| 1 | crates/flamewm-wm-wayland/src/snap_preview.rs:93-99 | outline resize ring | before: 1 raster (strip alloc + texture upload) per motion (~1 kHz); now at most 1 per 16 ms, translation free | OK, landed |
| 2 | crates/flamewm-wm-wayland/src/snap_preview.rs:100-101 and input/pointer.rs:251-253 | outline ring trailing edge | O(1) while paced; returns false, so no paint | DEFECT: last paced motion is never drawn while the pointer rests (stale ring until next motion or release). Fix next unit: SnapPreview `deferred` flag set on pacing skip, flushed with request_paint once 16 ms elapsed, checked from the per-vblank path (drm/mod.rs:307 area). |
| 3 | crates/flamewm-wm-wayland/src/input/pointer.rs:235-247 | live content resize | O(1): set_rect + pending_configure | OK. One configure per vblank via flush_pending_configure (drm/mod.rs:307, render/mod.rs:797). |
| 4 | crates/flamewm-wm-wayland/src/input/pointer.rs:209-214 and state/outputs.rs:145-157 | move: home output change | O(windows) scan with String and WlSurface clones on each cross-output step | Gated on a boundary crossing, but a pointer jittering on a boundary triggers it every event. Fix: cache home output name per window, skip when unchanged. |
| 5 | crates/flamewm-wm-wayland/src/input/pointer.rs:181-190 | move with snapPreview on | snap_target_at + snap_client_geometry + pointer_work_area per event, unpaced | Not measured; allocation not confirmed. Fix: reuse the zone while the pointer stays in the same snap band. |
| 6 | crates/flamewm-wm-wayland/src/snap_preview.rs:136-160 (called from render/mod.rs:358) | ring elements per paint | up to 4 MemoryRenderBufferRenderElement per paint | Cheap, not a freeze cause. |
| 7 | crates/flamewm-wm-x11/src/wm/effects.rs:3-8, 69-76 | X11 live resize configure | paced at 16 ms (RESIZE_CONFIGURE_INTERVAL) | OK. Same trailing-edge defect as #2: skipped batch is superseded only by the next motion or release. Fix: deferred configure from the event loop timer. |
| 8 | crates/flamewm-wm-x11/src/frame/* | X11 frame | not read in depth; grep found no per-motion scene rebuild | UNVERIFIED. |

Next units: p95-resize-trailing-ring (fix #2 and #7 together), p96-outputs-home-cache (fix #4), p97-snap-zone-reuse (fix #5, after measurement).
