# u16 — winit cursor-only paint element cache (proposal + guarded implementation)

**Scope:** nested winit paint path of `flamewm-wm-wayland`. Shared checkout; no `git`, no `cargo`,
no Python, no sleep loops, no builds or tests; the orchestrator commits. Workspace version read
from `Cargo.toml` (`0.1.7`). Pixels must stay identical.

**Applied edits are UNBUILT** — no compiler, test or runtime evidence exists for them in this unit.

**Route evidence:** `flamewm-source-first`, `flamewm-reference-mining`, `flamewm-rust-systems`
loaded. `.vendor/**` was read as semantic evidence only; no vendor code, type or dependency was
imported. Active owner of the mechanism is `crates/flamewm-wm-wayland/src/render/`; `State`,
`handlers`, `input` and `control` remain untouched.

## Problem

`render_frame` already skips a paint entirely when the host draws the pointer and no scene flag is
set. With a software-drawn client cursor (a surface), plain pointer motion calls
`State::request_cursor_paint` (`state/redraw.rs:21`) and therefore *does* paint; before this unit it
rebuilt every outline, surface, backdrop, titlebar and overlay element, re-checked both backdrop
captures and then submitted a frame whose only real change was the cursor. The tty backend had
already solved this with `frame_pacing::SceneBuild::CursorOnly` and a cached element list in
`drm/mod.rs:200-205,326-366`; the winit path had no equivalent.

## Decision: implementability

The invalidation points **are enumerable by reading**. Every pixel-changing scene mutation funnels
through `State::request_paint` (`state/redraw.rs:14-17`), which sets the single `scene_dirty` flag;
`request_full_redraw`/`request_redraw` call it, and plain pointer motion is the only caller of the
cheaper `request_cursor_paint` that does not (`input/keyboard.rs:53-91`). That gives one complete,
auditable invalidation funnel. A guarded implementation was therefore added inside the owned
render path. The guard never trusts the cache unless the generation, geometry, pointer owner and
readback state all match the cached frame; every other case takes the existing full-build path, so
a missed invalidation degrades to the old behaviour rather than stale pixels.

## Design

- **Generation counter.** `SCENE_GENERATION` in `render/mod.rs` is a monotonic `AtomicU64` owned by
  the render module (`State` is outside this worker's ownership). The frame that consumes a true
  `scene_dirty` bumps it exactly once (`render_into`), so the counter names the scene version the
  next cached frame may reuse. It starts at 0; the first paint is always a full build.
- **Cache key.** `SceneCache { generation, size, scale, host_draws_pointer, cursor_len, elements }`
  lives in a thread-local `RefCell<Option<SceneCache>>` (the winit event loop is single-threaded).
  The `Vec<Elem>` is the previous frame's complete front-to-back list; its first `cursor_len`
  entries are the topmost cursor elements, exactly as in the DRM `SceneCache`.
- **Cursor-only reuse.** `render_frame` consumes `scene_dirty` once and passes it to `render_into`.
  Cursor elements are rebuilt on every paint and placed first. If `!scene_dirty`, the generation
  and geometry guards match, there is no dump or pending screencopy, and the cache is present, the
  cursor slice is replaced in place with `Vec::splice` and the cached scene elements are reused.
  Otherwise the previous `build_elements_into` path runs unchanged. After submit, the resulting
  list is stored back for the next frame. Lock and blank frames drop the cache so a desktop scene
  cannot reappear behind a released lock.
- **Pixel identity.** Smithay's `OutputDamageTracker` keys element state by `Element::Id` and
  `last_commit` and calls `damage_since` for changed elements
  (`.vendor/smithay-master/src/backend/renderer/damage/mod.rs:321-370,555-582`). Reusing the same
  element values keeps their ids and commits, so smithay reports no damage for them; the freshly
  built cursor element produces the old and new cursor rectangles. `anvil` passes one element list
  to one `render_output` call (`anvil/src/render.rs:193-209`), which is what this path still does.
  COSMIC reuses a retained cursor texture and a separate cursor damage tracker so motion never
  rebuilds the scene (`.vendor/cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1115-1201`); this
  unit follows the same ownership rule without importing that code or a second tracker. The winit
  buffer-age/cursor-only damage rule added alongside this change stays in force: age 0 remains the
  fallback until a post-present query proves the swapchain accepts damage.
- **Fallbacks.** Missing cache, any generation/geometry/pointer-owner mismatch, a pending dump or
  screencopy, a malformed cursor slice, an error before the store, a lock screen and non-desktop
  frames all rebuild fully. No cached element is ever mutated by another path.

## Invalidation site inventory (all sites, mapped to the generation bump)

`scene_dirty` is initialised true (`state/mod.rs:406`), consumed by `take_scene_dirty`
(`state/redraw.rs:93-95`), and set only by `request_paint` (`state/redraw.rs:14-17`). The following
call sites are the complete reading-based inventory; each makes the next paint bump the generation
and take the full-build path.

**Direct `request_paint` callers**

- `state/redraw.rs:9` — `request_full_redraw`, reached by every `request_redraw` below.
- `input/pointer.rs:224` — snap ring / geometry change during a move grab.
- `input/pointer.rs:239` — snap ring hidden during a content-mode resize grab.
- `input/pointer.rs:253` — snap ring shown during an outline-mode resize grab.
- `input/buttons.rs:124` — ring hidden or window moved on button release.
- `input/buttons.rs:146` — resize release commits the final rectangle.
- `render/mod.rs` (post-paint re-arm, ~812) — a scene request made while the backend was taken.

**Window, focus, geometry and work-area changes (`request_redraw`)**

- `state/focus.rs:42,91` — focus change and focus-follows hover.
- `state/mod.rs:458` — output resize / mode change; `state/mod.rs:694` — work-area reflow.
- `handlers.rs:142` — switcher preview commit; `handlers.rs:284,627` — window state changes.
- `control/service.rs:217`, `control/apply.rs:58,71`, `control/mirror.rs:137,167,182,189,198,205,221`,
  `control/switcher.rs:141` — settings, mirror and switcher state applied to the scene.
- `input/shortcuts.rs:11`, `input/keyboard.rs:90` — shortcut/key changes that alter the scene.
- `xwayland.rs:547,599,623,697,713` — X11 window map/unmap/geometry/focus changes.
- `screencopy.rs:255`, `render/switcher_thumbs.rs:39`, `main.rs:753,757,800` — explicit repaints.

**Surface commits and overlays**

- `handlers.rs:126-185` — a visible surface commit calls `request_redraw` (`:184`); a commit with
  no new buffer, damage or frame request does not repaint (`:181-183`), so it cannot change pixels.
- `state/redraw.rs:42-51` — cursor image status change calls `request_redraw`, so switching between
  a host-drawn named cursor and a client surface invalidates the scene (`host_draws_pointer` is also
  stored in the cache key).
- `render/backdrop.rs:573`, `render/dock_backdrop.rs:486,507` — desktop/Dock recapture requests.
- `input/pointer.rs:267-281` — a decoration resize-cursor change goes through
  `refresh_cursor_status` and therefore through `request_redraw`.

**Cursor-only (does not invalidate)**

- `input/keyboard.rs:54-91` — plain motion with no grab, no focus-follows and an unchanged
  `model.hit` (so decoration hover and press feedback are unchanged) calls `request_cursor_paint`.
  This is the one winit path that reuses the cached scene.

## Residual risks / follow-ups

- A client cursor surface that commits an animated frame goes through `handlers.rs:184` and bumps
  the generation, so it still takes a full rebuild. Making that case cursor-only requires a
  cursor-surface classification in `handlers.rs`, which is outside the owned paths; it is a
  correctness-preserving follow-up, not part of this slice.
- A hidden/unmapped surface mutation that does not set `scene_dirty` is not drawn by this frame in
  any case; a later visible commit sets the flag. The geometry/pointer-owner guards are an extra
  fence against a state change reaching the renderer without a request.
- The counter and cache are process/thread-local static state rather than `State` fields. Moving
  them into `State` (owning module outside this task) would make the key explicit in the state type
  and is recommended if the render crate later gains ownership of that type.

## Verification

No checks executed by design (no cargo, build, tests or runtime in this unit). Static review only:
`scene_dirty` is consumed once in `render_frame` and passed once to `render_into`; the reuse guard
is a conjunction that falls back to the original full-build path; cursor elements remain the first
(topmost) slice; and the cached list is stored only after a complete submit. The orchestrator must
compile, run the existing rendering tests and diff a cursor-motion frame against the pre-change
output before this cache is enabled in a release.

Status: UNBUILT
