# u83-xwayland log

Batch: XWayland path. Owned paths: `crates/flamewm-wm-wayland/**`. `cargo`/`git` are forbidden in
the shared checkout, so every statement here is a source read plus `rustfmt --check` (syntax and
formatting only); the orchestrator builds and commits. Perf-shaped claims are composition estimates,
not measurements.

## Composition reasoning (one second of desktop use, X11 clients present)

Within a painted second with X11 apps open, the XWayland-related share splits roughly:

1. **frame-callback wakeups in hidden clients** — largest. An X11 client commits its next buffer
   only after a frame callback, so a window on another workspace or minimised used to be woken at
   the refresh rate (60/s per window) and rasterised into buffers nobody composites. With three
   hidden X apps that is the dominant X11-side cost in the sample.
2. **per-frame scene traversal** — second. `render::build_elements` and, far more often, pointer
   motion (`input::pointer::pointer_target`) call `scene::front_to_back`; it allocated 3-4
   intermediate vectors per call (layer clones, unmanaged indices, managed ids).
3. **per-commit surface→window scans** — third. Every client commit (cursor blink, spinner, terminal
   output) ran `window_for_surface`, an O(toplevels + X11 windows) scan, only to feed the Alt-Tab
   preview cache, which is closed almost always.

Expected shift: (1) removes hidden-client rasterisation from the sample entirely, (2) removes the
allocations from a path that runs per frame and per motion event, (3) removes an O(n) scan per
commit. Nothing changes for visible clients, so visuals are identical.

## Changes

1. **Frame callbacks only for visible windows** (`render/mod.rs::send_frame_callbacks`,
   `drm/events.rs::send_frame_callbacks`). Hidden managed windows (minimised or another workspace)
   are skipped for both Wayland toplevels and X11 surfaces; their popup trees are skipped with them.
   Smithay already trusts a per-surface primary-scan-out predicate, but this compositor passes
   `Some(output)` unconditionally, and its 16 ms overdue fallback still sends callbacks to surfaces
   that are not on any output — so the only reliable gate is not calling the walk at all. Unmanaged
   X11 surfaces (docks, menus) stay: they are always drawn.
2. **Single-pass scene traversal** (`scene.rs::front_to_back`). Appends straight into the returned
   vector instead of building `layers`, `overlay_top`, `bottom_background`, `unmanaged_above`,
   `managed_items` and `unmanaged_below`. The layer map guard is taken in two short scopes so it is
   never held while sending frame callbacks (smithay deadlocks on a second guard for one output).
   `compose_front_to_back` stays as the test-only ordering reference.
3. **Borrowed visible-window order** (`model.rs::visible_top_first_iter`). The scene walk no longer
   allocates a `Vec<WindowId>` per traversal; `visible_top_first()` now collects from it.
4. **Switcher check before the commit scan** (`control/switcher.rs::switcher_open`, used in
   `handlers.rs::commit`). The O(n) `window_for_surface` lookup now runs only while an Alt-Tab
   switcher is open. Pixel gating (`commit_changes_pixels`) and coalesced redraw are unchanged, so
   commits still repaint exactly when they change pixels.

## Damage-only commits (already correct, verified)

`CompositorHandler::commit` measures `commit_changes_pixels` before `on_commit_buffer_handler` and
requests a redraw only when a buffer/damage/frame-callback was present; `request_paint`/`request_full_redraw`
coalesce into one frame (`state/redraw.rs`). Resized buffers are re-imported at the resize commit so
stale damage cannot reach the GL texture (`reimports_on_resize`). No change was needed.

## Lazy XWayland start (rejected with evidence)

"Start XWayland on the first X client" cannot be implemented from inside the compositor: XWayland is
the X *server*, so no client-connect callback exists before it runs, and children inherit `DISPLAY`
from the compositor's own environment (`main.rs` spawns it right after the Wayland socket, and
`xwayland_ready` is what fires autostart). Deferring the spawn would leave autostart programs and any
X app launched from a child shell with no `DISPLAY`, which is a feature loss. What *was* lazy and
stays lazy: `X11Wm::start_wm` runs only on `Ready`, and `Props::connect` / compositor-selection claim /
Dock strut watcher all live in `xwayland_ready`. The avoidable cost was the per-refresh hidden-client
work, which change 1 removes.

## Checks

- `rustfmt --edition 2024 --check` on `scene.rs`, `model.rs`, `render/mod.rs`, `drm/events.rs`,
  `handlers.rs`, `control/switcher.rs`: exit 0 (parse + formatting).
- No `cargo`, no `git`, no interpreter, no scratch scripts.
- Runtime/compile verification belongs to the orchestrator's batch gate: UNVERIFIED here.

Commit: pending the orchestrator's `scripts/perf-loop sync`.
