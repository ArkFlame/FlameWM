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

PASS — the resize-live-pacing audit is complete: all eight resize-path sites were read against current source and recorded with file:line, per-event cost, and a bounded fix. No production edits were required (the outline-ring pacing fix already landed) and no build or test was run, so every finding and proposed fix remains UNBUILT. The trailing-edge defects (#2 Wayland, #7 X11) and the unverified X11 frame path (#8) are documented with concrete next units (p95–p97) rather than patched here, keeping this unit audit-only. The `STARTED` marker and `STATUS: OPEN` replacement from the brief were already consumed by the prior run that produced this result, so they are preserved rather than duplicated.

## Follow-up: p94 X11 trailing edge (design only, UNBUILT, 0 production edits)

Status: DESIGN ONLY. The fix needs a new `Wm` field, so it exceeds the 2-edit cap and was not applied. No build, test or cargo run.

Root cause (effects.rs:69-76, 136-138): when `paced` is true, the ConfigureFrameRoot, ConfigureClientLocal, NotifyClientRoot, LayoutInput, PaintChrome and ApplyShape effects are skipped with `continue` and dropped. Nothing retains them. The event loop (event_loop.rs:119-146) blocks with no resize-specific wait cap, so a resting pointer leaves the native frame at the last configured rect until the next motion or release.

Proposed p95 fix, 4 touch points:
1. wm/mod.rs:560 add `resize_deferred: Option<(Window, Vec<FrameEffect>)>`. wm/construct.rs:371 init to `None`.
2. wm/effects.rs execute_effects: when `paced`, store the skipped paced-set effects (latest wins, since configure rects are absolute). Clear on `commit.ungrab` (next to line 72) and in abort_session_to_idle (line 398).
3. wm/effects.rs new `flush_deferred_resize(now)`: when stored and `resize_configure_due(self.resize_configure_at, now)`, take the stored effects and call `self.execute_effects(client, &PointerCommit::plan(effects))` (interaction.rs:33). New `resize_deferred_wait(wait)` mirrors `deferred_chrome_wait` (paint.rs:92-100) and `transition_wait` (transition.rs:327): cap the existing wait at `resize_configure_at + RESIZE_CONFIGURE_INTERVAL`. This adds no new timer source.
4. wm/event_loop.rs: call `wm.flush_deferred_resize(now)` after `flush_deferred_chrome` (line 113), and apply `resize_deferred_wait` after `deferred_chrome_wait` (line 125).
Release stays unthrottled because ungrab clears the store and the release commit runs unpaced.
Regression check to add: a pure helper test in effects.rs tests that a paced batch is stored and that flush is due only after 16 ms.

Row 8 (frame/*) findings, read-only:
- frame/input.rs:19-46 `frame_control_at` builds 3 fixed control rects per call. Cost is O(1) and bounded, and it is not a scene rebuild.
- No per-motion `plan_scene`, `layout_frame_children` or texture work was found on the motion path in frame/*. Motion enters at wm/pointer.rs:331 and wm/workspace.rs:109, then `reduce`, then effects.rs:430.
- UNVERIFIED: allocation-level cost of `reduce` and the plan Vec per motion was not measured.
- frame/* grep counted 149 alloc/clone/format sites across frame/*.rs, most in tests or one-time layout. Not profiled.

Next: p95-resize-trailing-ring (the 4-touch fix above; needs the user to lift the 2-edit cap), then measure the per-motion allocation count in reduce.

## Result (X11 trailing-edge resize fix, UNBUILT)

Status: IMPLEMENTED, UNBUILT. No cargo, tests, git or python run. Not image-reviewed; no frame captured.
Files (4, under crates/flamewm-wm-x11/src/wm/):
- mod.rs: field `resize_deferred: Option<(Window, Vec<FrameEffect>)>`.
- construct.rs: init `resize_deferred: None`.
- effects.rs: paced batch stored latest-wins (filter = paced-set effects); cleared on ungrab and on any unpaced live batch; new `flush_deferred_resize(now)` replays via `PointerCommit::plan` when `resize_configure_due`, drops the store if the session is no longer Resize; new `resize_deferred_wait(wait)` caps the wait at `resize_configure_at + 16 ms`.
- event_loop.rs: `flush_deferred_resize` after `flush_deferred_chrome`; `resize_deferred_wait` after `deferred_chrome_wait`.
Release: ungrab clears the store and the release commit is unpaced, so release is unthrottled.
Deviation: abort_session_to_idle not edited; the stale store is dropped by flush when the session is not Resize.
Regression test: not added. The existing `resize_configure_due` tests in effects.rs cover the due decision; a replay test needs a live Wm, so it was not cheap.
Checks: none run (by instruction). Next: build + capture the live resize frame, confirm the trailing configure lands while the pointer rests.
