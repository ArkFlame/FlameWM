# Interactive resize latency: competitor research

Scope: read-only mining of `.vendor/**` (kwin-master, cosmic-epoch/cosmic-comp, mangowm,
smithay-master). Line numbers refer to those checked-out snapshots. Nothing here was built,
run, or profiled; this is source evidence plus a design comparison, not a runtime claim.
The question driving the search: how do these compositors keep pointer-controlled resize
responsive when the client cannot repaint as fast as the pointer emits motion?

## 1. KWin

**Interactive move/resize ownership.** `.vendor/kwin-master/src/input.cpp:670`
`MoveResizeFilter` sits at `InputFilterOrder::InteractiveMoveResize`. Its
`pointerMotion` (line 677), `pointerButton` (686), `keyboardKey` (704) and
`touchMotion` (729) all route to `Window::updateInteractiveMoveResize()` /
`endInteractiveMoveResize()` and swallow the event from clients. Axis events are
dropped outright (699) while a drag is active. One filter owns the whole drag, so
no second path can deliver a competing geometry update.

**Geometry update.** `.vendor/kwin-master/src/window.cpp:1132`
`Window::updateInteractiveMoveResize` stores the new anchor, computes
`nextInteractiveResizeGeometry(global)`, and calls `doInteractiveResizeSync()` only
when the geometry actually changed (1161-1207). The move branch similarly calls
`move()` only on a real change (1215-1237). The early exit at 1143 is the core
throttle: `if (isWaitingForInteractiveResizeSync()) return;`.

**Configure throttling (X11/XWayland).** `.vendor/kwin-master/src/x11window.cpp:3802`
`X11Window::isWaitingForInteractiveResizeSync` is true while a sync request is
`pending` or `acked`. `doInteractiveResizeSync` (3807) converts the frame rect to
a native *buffer* rect, returns early when that equals the client's current
buffer geometry (3813), and otherwise either calls `moveResize(rect)` directly or
does `setMoveResizeGeometry(frameRect)` + `sendSyncRequest()` + `configure(bufferRect)`.
The frame can therefore track the pointer while the client buffer lags. `sendSyncRequest`
(1919) refuses to send a second request while one is outstanding, increments a
serial, calls `setAllowCommits(false)` (writes `XWAYLAND_ALLOW_COMMITS` in
`setAllowCommits`, 3070), and arms a 10 s timeout. `ackSync` (2257) clears `pending`,
sets `acked` and re-enables commits; `handleCommitted` (3061) calls `finishSync`
(2284) only after the client actually commits; `ackSyncTimeout` (2275) temporarily
disables XSync and releases so a stuck client cannot freeze the drag. A special
case: while waiting, motion is dropped; on timeout during resize `finishSync`
forces one final `moveResize(moveResizeGeometry())` (2286-2290).

**Configure requests and constraint work.** `X11Window::configureRequest` (3098)
and `X11Window::resizeWithChecks` (3225) validate client-originated resizes against
rules, min/max hints, gravity and work area before `moveResize`, so the expensive
constraint pass runs once per accepted request, not per pointer sample.

**Outline.** KWin's `Outline` (`.vendor/kwin-master/src/outline.cpp:37` `show`,
`:51` `hide`, `:76` `setGeometry`) is a cheap QML overlay; `setGeometry` returns
early on an unchanged rect. In this snapshot it is used by `window.cpp:1248` and
`:3539` for quick-tile/maximize preview, **not** as a live-resize outline: pointer
resize is live with sync gating. Do not claim KWin has a resize outline here.

**Skip redraw of unchanged surfaces.** `WorkspaceScene::collectDamage`
(`.vendor/kwin-master/src/scene/workspacescene.cpp:664`) accumulates per-window
damage and then runs an occlusion cull pass (684-695) using `shouldRenderItem` /
`shouldRenderHole`, removing damage hidden by opaque windows before `endFrame`.
Unchanged or fully covered surfaces cost no redraw.

## 2. cosmic-epoch (cosmic-comp)

**Grab and geometry.** `.vendor/cosmic-epoch/cosmic-comp/src/shell/layout/floating/grabs/resize.rs:67`
`ResizeSurfaceGrab`; `update_location` (82) computes min/max-clamped, edge-snapped
size and stores `last_window_size`, calls `set_resizing(true)` and `set_geometry`,
then sends a configure **only** if `window.latest_size_committed()` (177-179).
`ResizeState::{Resizing,WaitingForCommit}` (60) is the explicit two-phase state.
`PointerGrab::motion` (202) forwards `handle.motion(data, None, event)` so no client
gets pointer focus during the grab; `frame` (258) forwards the frame callback;
`button` (228) releases the grab when buttons are empty.

**Configure serial ack gate.** `.vendor/cosmic-epoch/cosmic-comp/src/shell/element/surface.rs:630`
`CosmicSurface::latest_size_committed` compares `XdgToplevelSurfaceData::current_server_state().size`
with `last_acked.state.size`; the comment at 657-662 states the reason verbatim:
clients do not batch size requests, which misbehaves with a 1000 Hz mouse, and
throttling preserves visual consistency. `serial_acked` (598) and `serial_past`
(616) both test `last_acked.serial >= serial`. `send_configure` (681) delegates to
Smithay's `send_pending_configure`, so a no-change configure is not sent.
`is_resizing`/`set_resizing` (414/425) toggle `ToplevelState::Resizing`; for X11
surfaces the gate is `pending_configure().is_none()` (421, 667), i.e. one configure
in flight. `CosmicMapped::latest_size_committed` and `configure` are the fan-out for
stacks (`.vendor/cosmic-epoch/cosmic-comp/src/shell/element/mod.rs:492`, `:500`).

**Outline/indicator and scaled buffers.** `Shell::resize_mode` and the
`ResizeIndicator` (`.vendor/cosmic-epoch/cosmic-comp/src/shell/mod.rs:2541`, `:2555`)
draw keyboard-resize feedback only; `output_elements`
(`.vendor/cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:622`) passes it into
layout rendering. `RescaleRenderElement` in that file (593-606) is used for the
zoom/menu move grab, **not** for resize: no scaled-last-buffer resize path exists
in this corpus.

## 3. mangowm

**Coalesce pointer motion per output frame.** `pointer_process_motion`
(`.vendor/mangowm/src/input/pointer.c:1173`) branches at 1362-1374: floating
resize calls `pointer_resize_floating_window` only when
`m->last_resize_frame != m->frame_serial`; tiled resize calls `resize_tile_client`.
`resize_tile_client` (`.vendor/mangowm/src/layout/arrange.c:1008`) opens with an
explicit "Coalesce keyboard repeats and pointer motion to once per output frame"
guard (1017-1020). `handle_output_frame`
(`.vendor/mangowm/src/manage/monitor.c:1382`) increments `frame_serial` (1399).

**Skip redraw while waiting for the client.** The same frame handler sets
`resizing_on_this_mon` when a client has a pending `configure_serial` (1420-1422)
and jumps to `skip` via `monitor_check_skip_frame_timeout` (1425-1428, timer 100 ms
at 1378) when no animation/grab frame is needed. The compositor keeps accepting
motion but does not repaint until the client catches up or the timeout fires.

**Configure serial ack.** `.vendor/mangowm/src/manage/client.c:2392`
`client_xdg_size_pending` compares the client's current toplevel size with the
target. `handle_client_commit` (2399) clears `c->configure_serial` once
`configure_serial <= surface.xdg->current.configure_serial` (2454-2456), then sets
`dirty` from the pending-size and window-geometry-origin checks and returns early
for the grab client or when not dirty (2458-2475). `client_set_size` (610) drops
identical repeat configures unless `force_configure`, for both X11 geometry and
xdg size (638-659). `resize` (`.vendor/mangowm/src/animation/client.c:1578`) records
the returned serial and increments `resizing_count_pending` (1660-1666).

**Live geometry with stale buffer.** In `resize`, when `c == server.grab_client`
(1668-1695), mango snaps the scene node to the target rect immediately, sets
`current = pending = geom` (no animation), redraws border/clip/shadow/Tabbar and
returns. Decorations and clip follow the pointer while the client buffer is still
the old size; `client_set_surface_clip` hides the unrendered strip. This is the
closest analogue here to a "scaled/snapshot last buffer" trick, but it is a clip,
not a stretched texture.

## 4. smithay (protocol/library + example compositors)

**xdg_toplevel configure model.** `.vendor/smithay-master/src/wayland/shell/xdg/mod.rs:200`
defines `pending_configures` and `last_acked`. `ack_configure` (221) finds the
matching serial, stores it as `last_acked` and retains only configures with a
greater serial (isolate old serials). `current_server_state` (253) folds the last
pending configure (or `last_acked`) into the accumulated state so no change is
lost between configures. `has_pending_changes` (278) and `get_pending_state` (1444)
make configure sending idempotent; `send_pending_configure` (1472) returns `None`
when nothing changed, while `send_configure` (1489) always sends and registers the
serial for later ack. Ack dispatch is in
`.vendor/smithay-master/src/wayland/shell/xdg/handlers/surface.rs:302-334`
(`XdgShellHandler::ack_configure`).

**Frame-callback gating / skip unchanged surfaces.**
`.vendor/smithay-master/src/desktop/wayland/utils.rs:221` `send_frames_surface_tree`
sends queued `frame_callbacks` only when the surface is on its primary scan-out
output or the frame is overdue (259-280). It skips unmapped surfaces entirely
(237-247), and the doc comment at 219 says surfaces with no buffer never receive
frame callbacks; `surface_is_unmapped` is at 507. `WindowElement::send_frame`
(`.vendor/smithay-master/src/desktop/wayland/window.rs:323`) applies this to
toplevels. This is the mechanism that stops a resize from forcing needless client
repaints.

**Example resize grab.** `.vendor/smithay-master/anvil/src/shell/grabs.rs:344`
`PointerResizeSurfaceGrab`: `motion` (345) forwards to `handle.motion(data, None,
event)`, computes the clamped size, sets `State::Resizing` plus size and calls
`xdg.send_pending_configure()` (407-411); for X11 it uses `configure_with_sync`
(416). `button` (432) clears the Resizing state, sends the final configure (454)
and moves state to `WaitingForFinalAck` (477-478) / `WaitingForCommit` (515).
`.vendor/smithay-master/anvil/src/shell/xdg.rs:201` `ack_configure` advances to
`WaitingForCommit` only when `configure.serial >= serial` and the previously acked
state still had `State::Resizing` (222-244); `ensure_initial_configure`
(`.vendor/smithay-master/anvil/src/shell/mod.rs:294`) finishes it on commit (337-339).
`smallvil/src/grabs/resize_grab.rs:135` is the minimal version of the same gate.

## Ranked techniques and exact applicability to FlameWM

Ranked by expected latency benefit at 1000 Hz with a slow client, then by how
closely each fits the existing code.

1. **Ack-gated configure (highest value).** KWin `sendSyncRequest`/`ackSync`,
   cosmic `latest_size_committed`, mango `configure_serial`, Smithay `last_acked`
   all refuse to queue a second size while the previous serial is outstanding.
   *FlameWM:* `crates/flamewm-wm-wayland/src/state/mod.rs:717` `apply_geometry`
   currently calls `surface.send_pending_configure()` unconditionally, and
   `pointer.rs:246` just overwrites `pending_configure`. Add a per-window
   `inflight_serial: Option<Serial>` plus `queued_rect`; in
   `crates/flamewm-wm-wayland/src/handlers.rs:126` `commit`, read
   `XdgToplevelSurfaceData::last_acked` and, when `serial >= inflight_serial`, send
   the queued rect (and clear it). X11: `crates/flamewm-wm-x11/src/wm/effects.rs:100`
   `ConfigureFrameRoot` should track an XSync counter per client like KWin's sync
   request instead of firing every due batch.

2. **One configure per frame / per pointer batch.** mango `frame_serial` gate;
   KWin's `isWaitingForInteractiveResizeSync` return; cosmic/anvil's
   `send_pending_configure`.
   *FlameWM:* already largely present — `pointer.rs:151` gates on
   `grab_motion_needs_work`, `pointer.rs:243` skips unchanged rects, and
   `state/mod.rs:710` `flush_pending_configure` emits once per paint. The gap is
   interaction with (1): while a serial is in flight, motion must update only the
   queued rect and must not create a second configure. X11 already paces at 16 ms
   in `wm/effects.rs:7` `RESIZE_CONFIGURE_INTERVAL` and `:12`
   `resize_configure_due`; making that deadline ack-driven (not just time-driven)
   is the incremental change.

3. **Skip redraw of unchanged surfaces.** KWin `collectDamage` occlusion cull;
   Smithay `send_frames_surface_tree` output/overdue gating and no-buffer skip;
   mango `monitor_check_skip_frame_timeout`.
   *FlameWM:* `handlers.rs:226` `commit_changes_pixels` already checks buffer,
   damage and frame callbacks before `request_redraw`. Extend the frame-callback
   side to Smithay's primary-scanout-or-overdue rule so a resize commit does not
   wake a client whose surface is not visible; keep `handlers.rs:216`
   `reimports_on_resize` as the only texture reimport trigger. X11: the existing
   `wm/effects.rs` pacing already avoids redundant configure batches; add a
   matching damage skip in `crates/flamewm-wm-x11/src/wm/overlay.rs` if repaint
   still follows every motion.

4. **Outline mode as the cheap path.** KWin/Smithay/cosmic have no pointer-resize
   outline; mango's snapped scene node is the nearest live analogue, and cosmic's
   `ResizeIndicator` covers keyboard resize only. FlameWM is unusual in already
   having both modes via `show_content_while_dragging`.
   *FlameWM:* `pointer.rs:248-255` correctly does no configure and no scene
   rebuild in outline mode, and `buttons.rs:133-143` commits the final rect once
   at release, clearing `pending_configure` so paint does not duplicate it. Keep
   this path intact; consider making outline the automatic fallback while an
   ack is outstanding (combine with (1)).

5. **Snapshot / scaled-last-buffer during resize (lowest confidence).** No vendor
   in this corpus stretches the previous texture during resize. Closest evidence:
   KWin `setMoveResizeGeometry` (x11window.cpp:3820) keeps the frame at the pointer
   while the client buffer is stale; mango `resize` (animation/client.c:1668-1695)
   snaps the scene node and clips away unrendered area; cosmic's
   `RescaleRenderElement` is used for move/zoom, not resize.
   *FlameWM:* `handlers.rs:146` `import_resized_buffer` reimports at the resize
   commit, which is a correctness fix, not a stretched preview. A scaled-last-buffer
   element would be a new scene element under `crates/flamewm-wm-wayland/src/scene.rs`
   and should be treated as experimental, not copied from a proven precedent.

## Evidence limits

No runtime timings, no build, no tests; all findings are static reads of the four
vendor snapshots. Line numbers may drift if those snapshots are refreshed. The
"scaled-last-buffer" item has no direct upstream precedent in the corpus, so it is
reported as a gap rather than as a competitor technique.

PARTIAL — the four corpora were mined and all requested mechanisms were located except a true snapshot/scaled-last-buffer resize path, for which only adjacent analogues (KWin's frame/buffer split, mango's scene-node snap plus clip, cosmic's move-grab rescale) exist; the ranked FlameWM applicability is grounded in the cited current source but every performance claim remains unverified because no runtime measurement was in scope.
