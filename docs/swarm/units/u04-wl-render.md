# u04 — Wayland compositor render loop and damage audit (smithay / cosmic / mango / kwin)

**Scope:** Wayland render/damage audit plus the smallest safe cuts in the owned paths. Shared
checkout; no `git`, no `cargo`, no Python, no sleep loops, no builds or tests, orchestrator commits.
Visuals must stay pixel-identical. Workspace version read from `Cargo.toml` (`0.1.7`).
**Applied edits are UNBUILT** — no compiler, test or runtime evidence exists for them in this unit.

**Route evidence:** `flamewm-source-first`, `flamewm-reference-mining`, `flamewm-rust-systems`,
`flamewm-performance-proof` loaded. `.vendor/**` used as read-only semantic evidence only; no vendor
code, type or dependency was imported. No performance claim below is measured: costs are static
call-path budgets from source inspection, to be confirmed by the orchestrator's build/test run.

## Report

The Wayland paint path is already close to the reference implementations: the nested winit path
gates a paint on a coalesced `scene_dirty` flag and skips it entirely when the host draws the pointer,
the tty path is vblank-driven with `FrameDecision`/`SceneBuild::CursorOnly` and never arms a periodic
timer while idle, both submit through smithay's `OutputDamageTracker` with buffer-age partial repaint,
the titlebar and backdrop/rings caches key on content generation so a static frame causes no texture
upload or readback, and element/scene buffers are appended into caller-owned vectors. No idle repaint
or full-scene recapture loop remains on the steady-state path; the residual per-painted-frame work is
an unavoidable full element rebuild in the winit path (the tty path already has a cursor-only cache)
and small allocation/hit-test overheads. Two safe cuts were applied — the occluder list is
pre-allocated to its cap in both frame builders, and the per-window hit test now reuses the single
hit result already computed at the top of the traversal — with no visual or ordering change. The
larger wins (a cross-frame element cache for cursor-only winit paints, an `into` variant of
`surface_elements`, separating a cursor damage tracker as cosmic does, and single-surface direct
scanout) are proposed below rather than applied because they change cache-invalidation, readback and
presentation-feedback semantics and cannot be validated without a build in this unit.

## Reference precedent (semantic findings, not transplanted code)

| Reference path | Semantic finding |
| --- | --- |
| `smithay-master/anvil/src/render.rs:199-209`, `smithay-master/src/backend/renderer/damage/mod.rs:326,382,571` | `OutputDamageTracker::render_output(renderer, framebuffer, age, &elements, clear)` diffs each element via `damage_since(commit)` and buffer age; a zero age means full repaint. |
| `smithay-master/anvil/src/udev.rs:795,646,1046` | Repaint is driven by `DrmEvent::VBlank`; direct scanout is an explicit opt-out flag and a dmabuf scanout tranche, not an implicit path. |
| `cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1119-1121,1157-1195,1243-1403` | A separate `cursor_damage_tracker` + reused `cursor_texture` lets cursor motion repaint without rebuilding or redrawing the scene; textures are recreated only on format/size change. |
| `kwin-master/src/compositor.cpp:78,649-671,899`, `kwin-master/src/scene/scene.cpp:56-75,449-459` | Repaints are render-loop/layer driven; each layer tracks `needsRepaint`, and the scene paints only the intersection of accumulated device damage with the render region. |
| `mangowm/src/manage/monitor.c:1382-1439` | `handle_output_frame` is a wlroots vblank callback; when no animation/grab needs more frames it skips the output commit entirely and still sends frame-done. |

## Audit log

Costs are per painted frame unless marked steady-state; `applied` rows are UNBUILT.

| Area | File:line | Cost | Change | Risk | Reference path |
| --- | --- | --- | --- | --- | --- |
| Idle repaint gate (winit) | `crates/flamewm-wm-wayland/src/render/mod.rs:750-756` | Zero paints when `scene_dirty` is clear and the host draws the pointer (steady state). | None — already correct; `request_paint` sets the flag, `request_cursor_paint` does not. | Low: a path that changes pixels without `request_paint` would be missed; existing callers set it. | `smithay-master/anvil/src/udev.rs:795`; `mangowm/src/manage/monitor.c:1382-1429` |
| Frame scheduling (tty/DRM) | `crates/flamewm-wm-wayland/src/state/frame_pacing.rs:24-34,48-54,61-77` | `plan_frame` paints only when dirty or a callback is outstanding; queued work defers to vblank; no idle timer. `SceneBuild::CursorOnly` reuses cached elements. | None — parity with references. | Low: `needs_sync` deferral is bounded by `SYNC_WAIT_BUDGET`/`SYNC_WAIT_LIMIT`. | `kwin-master/src/compositor.cpp:78,899`; `smithay-master/anvil/src/udev.rs:795,1262-1286` |
| Damage tracking + buffer age | `crates/flamewm-wm-wayland/src/render/mod.rs:829-839,917-925` | Partial repaint via `state.damage.render_output(..., age, ...)`; age 0 falls back to full repaint, deliberately latched after a driver refusal. | None — already uses smithay's tracker and age. | Low: software EGL latches to full repaint, which is the safe fallback. | `smithay-master/src/backend/renderer/damage/mod.rs:326,382`; `smithay-master/anvil/src/render.rs:199-209`; `cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1403` |
| Full-scene rebuild per painted frame | `crates/flamewm-wm-wayland/src/render/mod.rs:319-335,493-720` | Every painted frame re-walks `front_to_back` and rebuilds all outline, surface, backdrop, titlebar and solid elements; winit path has no cross-frame element cache. | **Proposed:** carry the tty `SceneBuild::CursorOnly` idea to the winit path — retain the last element vec and append only cursor elements for cursor-only paints. | Medium: cached elements must be invalidated on every commit/scroll/scale change and must not hide a screencopy or dump readback from fresh pixels. | `crates/flamewm-wm-wayland/src/state/frame_pacing.rs:38-54`; `cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1157-1195` |
| Redundant pointer hit-test | `crates/flamewm-wm-wayland/src/render/mod.rs:501,666-670` | One `model.hit(pointer)` per decorated window per painted frame, duplicating the single result already held in `hovered_button`. | **Applied (UNBUILT):** use `hovered_button == Some(hit)` for `pressed_hit`; `state` is an immutable borrow for the whole traversal, so the value cannot change. | Low: `Hit` is `Copy + PartialEq` and `hovered_button` is computed from the same pointer/model before any branch. | `kwin-master/src/scene/scene.cpp:72-75` (skip redundant per-item work) |
| Occluder-list reallocation | `crates/flamewm-wm-wayland/src/render/mod.rs:511` and `crates/flamewm-wm-wayland/src/render/dock_backdrop.rs:437` | `Vec::new()` grew 1→2→4→8→16→32→64 (about seven allocations) on every painted frame and every Dock backdrop probe. | **Applied (UNBUILT):** promote `MAX_OCCLUDERS` to `pub(super)` and `Vec::with_capacity(MAX_OCCLUDERS)` in both sites. Same contents and cap, no visual change. | Low: one 64-rect allocation up front instead of growth; `dock_backdrop.rs` already imports `super::*`. | `cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1125-1146` (reuse/retain buffers) |
| Per-surface element vectors | `crates/flamewm-wm-wayland/src/render/mod.rs:275-290` | `surface_elements` allocates a `Vec<Elem>` per surface per rebuild, including lock, layers, popups and Docks. | **Proposed:** add an `into` sibling that appends to a caller scratch vec and migrate the frame builder; keep the owned wrapper for lock/Dock callers. | Medium: lock, screencopy and Dock capture paths must get the same element order and ownership. | `smithay-master/src/backend/renderer/element/utils/wayland.rs`; `smithay-master/anvil/src/render.rs:199-209` |
| Titlebar element cache | `crates/flamewm-wm-wayland/src/render/titlebar.rs:364-432,118-121` | Cached on `(width, active, maximized, rounded, title)`; moved bar only re-places the buffer; unchanged feedback early-outs before `buffer.render()`, so no redundant upload. | None — already correct. | Low: a future input added to `chrome_pixels_over` must join the cache key. | `kwin-master/src/scene/decorationitem.cpp`; `kwin-master/src/scene/atlas.cpp` |
| Desktop/dock backdrop capture | `crates/flamewm-wm-wayland/src/render/backdrop.rs:586-648`, `crates/flamewm-wm-wayland/src/render/titlebar.rs:284-331` | Steady state: commit-counter compare only, scratch GL texture retained across captures, recapture throttled; flat glass drops the capture entirely. No readback or blur on a static frame. | None — already correct. | Low: stale capture is bounded by `DESKTOP_RECAPTURE_INTERVAL` with a one-shot wake. | `cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1157-1195`; `kwin-master/src/compositor.cpp:649-671` |
| Occlusion culling | `crates/flamewm-wm-wayland/src/render/mod.rs:384-407,506-511,575-704` | Front-to-back traversal skips off-output windows and windows covered by up to 64 opaque regions; memory/rounded elements are deliberately not occluders. | None — already correct (cap now pre-allocated). | Low: an over-broad opaque region would drop pixels; clipped chrome is excluded on purpose. | `kwin-master/src/scene/scene.cpp:449-459` |
| Per-frame solid element ids | `crates/flamewm-wm-wayland/src/render/mod.rs:264-270,710-716` | `Id::new()` for each solid element (blue-light veil, client body) on every painted frame; smithay backs an external id with an `Arc`, so this is a small allocation per solid element. | **Proposed, not applied:** a stable cached `Id` for the veil/body where identity carries no meaning. | Low; but an element id is an identity contract, so changing it needs a renderer-level check. | `smithay-master/src/backend/renderer/element/mod.rs:53-63,185`; `smithay-master/src/backend/renderer/element/solid.rs` |
| Readbacks (screencopy / dump / switcher) | `crates/flamewm-wm-wayland/src/render/mod.rs:936-1000` | Offscreen texture + readback only when a client asked; `screencopy_pending.is_empty()` short-circuits to zero GPU work. | None — already correct. | Low: dumps take a separate offscreen path to avoid losing the GL context. | `cosmic-epoch/cosmic-comp/src/backend/render/mod.rs:1115-1123` |
| Frame callbacks | `crates/flamewm-wm-wayland/src/render/mod.rs:1002-1030` | 16 ms throttle; hidden and fully off-output windows get no callback, so they cannot rasterise at refresh rate unseen. | None — already correct. | Low: a window becoming visible must still receive a callback on the first painted frame. | `mangowm/src/manage/monitor.c:1437-1439` (frame-done still sent on skipped frames) |
| Direct scanout | `crates/flamewm-wm-wayland/src/render/mod.rs:774-932` | Paint path always composites through `render_output`; no single-surface scanout fast path. DRM buffers are allocated with `SCANOUT` in `drm/outputs.rs:255`, but that is not a compositor scanout decision. | **Proposed:** detect one opaque, untransformed, full-output client surface and hand its dmabuf to the CRTC, falling back to composite for damage, overlays or presentation feedback. | High: damage rectangles, cursor plane, presentation feedback, screencopy and lock screen must all remain correct; needs >1% frame-time evidence first. | `smithay-master/anvil/src/udev.rs:646,1046`; `smithay-master/src/backend/drm/output.rs`; `cosmic-epoch/cosmic-comp/src/backend/kms/render/mod.rs` |

## Verification

No checks executed by design (no cargo/build/tests/runtime in this unit). The three edits are
**UNBUILT** and must be compiled and visually diffed by the orchestrator. Static review confirms:
`MAX_OCCLUDERS` is `pub(super)` and reachable through the existing `use super::*` in
`dock_backdrop.rs`; `Hit` is `Copy + PartialEq`, so `hovered_button` remains usable after the
`hovered` match; and both changed sites preserve their previous contents and ordering.
