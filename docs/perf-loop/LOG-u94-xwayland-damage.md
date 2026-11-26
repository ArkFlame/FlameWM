# u94-xwayland-damage log

Batch fourteen. Owned paths: `crates/flamewm-wm-wayland/src` (`xwayland*`, `render*`, `scene*`).
`cargo`/`git` are forbidden in the shared checkout, so every statement below is a source read plus
`rustfmt --check` (syntax/formatting only); the orchestrator builds and commits. Composition claims
are estimates from the call graph, not measurements.

## Traced path: X11 commit -> damage -> render scheduling

1. **Commit.** XWayland is an ordinary Wayland client, so an X11 window's buffer lands in
   `handlers.rs::CompositorHandler::commit` (`handlers.rs:125-171`).
   `commit_changes_pixels` (`handlers.rs:211-227`) runs **before** `on_commit_buffer_handler`,
   which is what makes it see the state that matters: smithay promotes pending attributes to
   current in `PrivateSurfaceData::commit` (`smithay-0.7/src/wayland/compositor/tree.rs:281-287`)
   *before* it invokes the handler, and `on_commit_buffer_handler` then takes the buffer and damage
   out of that state. So the gate sees `buffer.is_some()` for every attach, `!damage.is_empty()`
   for a damage-only commit, and a frame-callback-only commit as *not* paintable. Every paintable
   commit calls `state.request_redraw()` (`handlers.rs:168-170`).
2. **Damage.** Nothing here is hand-computed: `render_into` feeds the element list to
   `OutputDamageTracker::render_output` (`render/mod.rs:821-824`). A commit that carries a new
   buffer but no damage rectangle still damages the whole buffer
   (`smithay-0.7/src/backend/renderer/element/surface.rs:310-315`: `damage_since` falls back to
   `Rectangle::from_size(buffer_dimensions)`), so an X11 client that repaints without explicit
   surface damage cannot lose pixels.
3. **Scheduling.** `request_redraw` -> `request_full_redraw` -> `request_paint` ->
   `request_cursor_paint` sets `scene_dirty` and asks winit for a frame
   (`state/redraw.rs:6-29`), paced to at most one host paint per `WINIT_FRAME_INTERVAL` = 16 ms
   (`state/redraw.rs:129-163`, `state/mod.rs:19`). Inside the interval the request arms a calloop
   timer instead of returning, so a coalesced request still paints.
4. **Callback back to XWayland.** `send_frame_callbacks` (`render/mod.rs:903-956`) runs at the end
   of every paint and walks `state.x11` plus every unmanaged X11 surface, skipping only windows the
   model reports as hidden (`state/model.rs:467-470`). One callback per paint is exactly the
   contract XWayland needs to submit the next buffer.

**Conclusion: the WM does not drop or throttle X11 updates.** Every paintable commit requests a
paint, every paint sends the callback that unblocks XWayland, and missing damage is treated as
whole-buffer damage. "A few render dispatches per second" therefore means *a few presented
commits per second*, and `wait=99%` is the compositor correctly parked between them: the X11 client
rasterises into its own SHM window on its own schedule (`flamewm-terminal` is X11-only via
`flamewm-ui-x11`, LOG-u93), and that work is invisible to the compositor's event loop. LOG-u93 also
measured the paints that do happen at 100-280 ms each, i.e. dominated by the blocking host
`swap_buffers` (`smithay-0.7/src/backend/winit/mod.rs:326-353`), not by scene building.

## Two real defects found on that path (both fixed here)

1. **Frame requests made during a paint were dropped.** `render_frame` takes `state.backend` for
   the whole paint (`render/mod.rs:666-679`), so `request_winit_frame` found no window and returned
   silently (`state/redraw.rs:132-135`) while `scene_dirty` was set. Work that finishes inside the
   paint - a switcher thumbnail (`render/switcher_thumbs.rs:39`), a screencopy readback
   (`screencopy.rs:255`) - then waited for an unrelated host event to be shown. Fix: after restoring
   the backend, if the scene flag was set again during the paint, ask for that frame immediately.
2. **A skipped paint left the pacing clock stale.** `note_winit_frame` was called only on a real
   paint, so after the early "host draws the pointer" return (`render/mod.rs:659-664`) the next
   burst of requests saw `elapsed >= WINIT_FRAME_INTERVAL` and bypassed the 16 ms gate on every
   event. Fix: stamp the interval when the host hands out the frame, before the scene gate.

## Expected composition shift (one second of X11-client use)

Within the compositor's second these two remove, in order: (a) the deferred `wait` penalty on
thumbnail/screencopy pixels, which is a correctness fix rather than CPU; (b) a per-motion-event
host frame request collapsing back to <= 62/s, which is the whole share of the pointer-motion wake
path on a nested window. Element building, surface upload and the composite are untouched, so
nothing visual changes.
