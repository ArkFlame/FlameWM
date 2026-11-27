# u16b — Cursor-only damage (Wayland nested/winit and DRM)

Status: **UNBUILT** — static source review and edits only. No `git`, `cargo`, Python, build, test or
runtime command was run in this unit; the orchestrator owns verification and the commit. Workspace
version read from `scripts/version` source (`Cargo.toml` workspace package `0.1.7`) without running
the script (it needs Python). Skills loaded before edits: `flamewm-source-first`,
`flamewm-performance-proof`, `flamewm-rust-systems`, `flamewm-parallel-execution`. Git state
inspection was forbidden by the brief. `cursor.rs` was re-read in full before deciding not to edit
it (it already carries other work).

## Determination

- **DRM: already limited.** `crates/flamewm-wm-wayland/src/drm/mod.rs:334-347` takes the cached
  scene on `SceneBuild::CursorOnly` (`state/frame_pacing.rs:46-54`) and splices only the cursor slice;
  `drm/mod.rs:494-499` hands those elements to `DrmCompositor::render_frame`, whose swapchain-slot
  age drives smithay's damage tracker (`.vendor/smithay-master/src/backend/drm/compositor/mod.rs:
  1713,1734-1738`). Cursor elements are `Kind::Cursor` (`cursor.rs:7-8,119,139`), so the hardware
  cursor plane is available. Pointer motion with nothing else changed does **not** rebuild the scene
  on DRM.
- **Nested winit, host-drawn pointer: already limited.** A named/hidden cursor never enters the
  software scene: `state/redraw.rs:21-29` (`request_cursor_paint` sets only `redraw_dirty`),
  `input/keyboard.rs:84-88` (plain motion does not request a full redraw), and
  `render/mod.rs:752-759` (the scene-dirty gate returns before a frame is painted). Pointer motion
  costs zero frames.
- **Nested winit, client cursor surface: full repaint before this unit.** `input/keyboard.rs:65-79`
  detects a changed hover or a grab, and `84-91` otherwise calls `request_cursor_paint`, which does
  not set `scene_dirty`. The gate at `render/mod.rs:752-759` passes for a surface cursor
  (`state/redraw.rs:31-38`) and `render/mod.rs:776-828` rebuilds every element. The age was forced
  to 0 by `render/gl_probe.rs:72-74,88-89` (winit latches `BUFFER_AGE_REFUSED`), and smithay turns
  age 0 into whole-output damage
  (`.vendor/smithay-master/src/backend/renderer/damage/mod.rs:742-761`), so
  `render/mod.rs:927` submitted full damage on every motion event. That is the gap this unit closes
  where the swapchain can actually take damage.

## Rows (file:line, cost, change, risk)

Costs are per painted frame and static call-path budgets, not measurements. All applied rows are
UNBUILT.

| # | File:line | Cost | Change | Risk |
| --- | --- | --- | --- | --- |
| 1 | `render/mod.rs:752-759`, `state/redraw.rs:14-29`, `input/keyboard.rs:84-91` | Zero paints for pointer-only motion when the host draws the pointer; one paint for a software cursor surface. | No change — already correct and used as the precondition for row 3. | Low: a pixel-changing path that forgot `request_paint` would be missed; all commit and hover paths set it. |
| 2 | `render/mod.rs:830-865`, `render/gl_probe.rs:11-17,72-74,88-89`; vendor `damage/mod.rs:742-761` | Full clear + full element draw + full-output damage per software-cursor motion, because winit forced `age = 0`. | No change to the latch: a driver that refuses `EGL_BUFFER_AGE` still repaints in full. Only a post-present positive age can unlock row 3. | Low: this is the always-correct fallback and keeps identical pixels on llvmpipe/nested EGL. |
| 3 | `render/gl_probe.rs:24-38` (`WINIT_DAMAGE_SWAP`, `note_winit_buffer_age`), `render/mod.rs:767-772` | One EGL buffer-age query after the first submit, then a single latched boolean; no per-frame query was added. | Probe the winit surface once *after* presentation; a positive answer proves both buffer-age and damage-swap support, a refusal or age 0 leaves full repaint latched forever. | Medium: a driver that answers positively but ignores `swap_buffers(damage)` would show stale pixels; smithay's `supports_damage()` ties the answer to both extensions (`egl/display.rs:637-653`). Unverified on a live GPU session. |
| 4 | `render/mod.rs:845-865`, `render/gl_probe.rs:50-62` (`frame_age`) | On a confirmed damage-capable nested swap, only the old and new cursor rectangles are cleared, redrawn and submitted instead of the full output; the element list is still rebuilt. | Pass `age = 1` to `OutputDamageTracker::render_output` for a frame with no scene rebuild, dump, lock or colour/zoom composite. Smithay diffs the moved element's current and previous geometry (`damage/mod.rs:545-568`) and adds no stale frames for age 1 (`742-747`). | Medium: relies on scene-dirty being set by every other pixel change (verified at `handlers.rs:126-185` and `input/pointer.rs:223-254`); a missed setter would leave a stale region. No visual diff was run. |
| 5 | `render/mod.rs:937-952` | One extra `render_output` call only when a repeated motion produces no damage at all. | If `render_output` returns no damage on a cursor-only frame it skipped the draw; repeat with `age = 0` so the full frame is drawn and submitted rather than swapping an undrawn back buffer. | Low: costs a full repaint only for a same-pixel motion event; pixels stay identical. |
| 6 | `render/mod.rs:783-787,764`, `render/gl_probe.rs:32-38` | Signature change (`scene_dirty` threaded into `render_into`) plus two new statics/helpers. | Smallest wiring that keeps the gate's consumption of `scene_dirty` and lets the damage decision use it. | Low: compile not checked; all call sites (one) updated. |
| 7 | `render/renderer_class_tests.rs:1,10-26` | Two pure truth-table tests, no renderer needed. | Assert `frame_age` chooses 1 only for cursor-only inputs with a confirmed damage swap and keeps the real/fallback age otherwise. | Low: global probe statics are deliberately not mutated by tests, so no cross-test interference. |
| 8 | `drm/mod.rs:318-347,494-499`, `state/frame_pacing.rs:46-54` | Cached cursor-only scene and compositor damage tracking already avoid a full rebuild on DRM. | No change. | Low: scene cache invalidation is covered by `SceneBuild::Full` on any `scene_dirty`. |

## Reference rows (`.vendor/**`, read-only evidence)

| Path | Semantic finding | Active decision |
| --- | --- | --- |
| `.vendor/smithay-master/src/backend/renderer/damage/mod.rs:326-371,475-568,741-761` | `render_output(renderer, framebuffer, age, elements, clear)` derives element damage from current vs previous geometry/commit; `age = 0` or too-old age replaces it with the whole output; age 1 adds no older frames. | Use the API unchanged; supply age 1 only for a scene-unchanged cursor frame. |
| `.vendor/smithay-master/src/backend/winit/mod.rs:339-387` | `buffer_age()` returns `Some(0)` when damage swaps are unsupported and `None` on query error; `submit(damage)` ignores damage unless the display supports it. | Gate the partial path on a positive post-present age; otherwise full repaint. |
| `.vendor/smithay-master/src/backend/egl/display.rs:637-653` | Damage support requires `EGL_EXT_buffer_age` plus `EGL_KHR_swap_buffers_with_damage` (or EXT). | A positive buffer-age answer is used as the damage-swap proof. |
| `.vendor/cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1115-1195,1598-1604` | cosmic keeps a separate cursor texture and damage tracker so cursor motion repaints without rebuilding the scene. | Same rule (cursor damage is separate from scene damage), but no offscreen cursor texture was introduced: smithay's moved-element diff already yields the two cursor rectangles. |
| `.vendor/smithay-master/src/backend/drm/compositor/mod.rs:1706-1738` | DRM render_frame uses swapchain-slot age and resets allow-partial-update on pending commits. | DRM needs no change; its damage path was already active. |

## Verification

Not run, by task rule (no cargo, git, Python, builds, tests or runtime session). Every applied edit
is **UNBUILT**. The orchestrator must compile `flamewm-wm-wayland`, run its unit tests, and check a
dump/PPM diff for a motion-only sequence on a nested winit session with a client cursor surface and
on a damage-capable GPU before treating the cursor-only path as safe. Where the probe latches false
(the nested llvmpipe case), behaviour is byte-for-byte the pre-existing full repaint.

PARTIAL — the determination is complete (DRM and host-drawn nested pointers already avoid the
full-scene render; software-cursor nested motion did not), and the smallest conditional fix is
applied: a confirmed damage-capable winit swap now reports only the moved cursor's old and new
rectangles via smithay's age-1 tracker semantics, with a full-repaint fallback that preserves
identical pixels everywhere else. It remains UNBUILT and its benefit is driver-dependent, so no
performance improvement is claimed here.
