# FlameWM × COSMIC Epoch: Native Rendering, Blur, Window Resize, Selection and Start Performance

**EXECUTION-READY COMMANDCODE / CLAUDE CODE HANDOFF — 2026-10-23**  
**Mission:** Investigate and implement the highest-value COSMIC-derived *mechanisms* into FlameWM's **existing Rust/X11 architecture**, iterating until FlameWM remains visually/functionally equivalent but uses dramatically less CPU, X server work, allocations and resident memory during transparency, blur, window dragging/resizing, Desktop/Explorer selection, and Start open/close. **Do not port slower COSMIC behavior.** If an end-to-end refactor is justified, perform it incrementally and retire the bypassed path in the same change. The production objective is near-zero **idle** work; interactive operations cannot literally consume zero CPU. Never assert a percentage without measured before/after workload evidence.

> **Direct instruction to agent:** This Markdown is an immediately actionable research supplement to the currently running performance pass. **Resume from the live checkout, not the supplied archive.** Determine the present HEAD, dirty state and which concurrent agents own files; reconcile their work instead of resetting it or duplicating changes. Explore `.vendor` COSMIC references side-by-side and keep searching for *better* source-level patterns after completing the listed work. Iterate through independent optimizations, scoped counter/tests, short native measurements and visual checks. Do not stop after the first successful crate test. Do not edit system KWallet/profile/data, do not overwrite uncommitted work, do not abandon a failed verification gate, and do not pretend an unavailable live test passed.

## 0. Source provenance, confidence and freshness

The snapshot analyzed here is **`1657_23_10_26.tar.gz`**, SHA-256 `548bcf80893e4cb8b4b086d7fda7339b98f158feba94cb61e9400907b6d858dc`. The supplied **`cosmic-epoch.tar.gz`**, SHA-256 `cb5a8633f2fb8b1924cf3964b850896cc9e4dffdcb46a991539bb94b7cf6673b`, contains `cosmic-comp`, `cosmic-panel`, `cosmic-files`, `cosmic-launcher`, `cosmic-term` and related source. The accompanying October 23 Claude log (`Pasted text(20261023-210157).txt`) records earlier optimization agents and **a successful Debian package build**, but **does not establish 95% CPU savings**. The live checkout at `/run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender` is newer/authoritative. Actual `.vendor` layout may differ from the archive; locate equivalents using `rg --files .vendor | rg 'cosmic-comp|cosmic-panel|cosmic-files'`. References below are *verified against the supplied snapshots*, not claimed identical to your live source.

**Evidence levels:** `[F]` direct source inspection; `[L]` prior agent log, possibly superseded; `[H]` hypothesis requiring profiling; `[U]` proven upstream architectural mechanism, **not** a FlameWM runtime measurement. Line numbers are approximate anchors for those exact snapshots; always relocate with `rg`.

**Latest previous agent progress [L], do not re-implement blindly:** desktop rubber-band now freezes projected item rectangles on press, skips identical motion, diffs membership and removes stale selected IDs; 127–128 desktop tests reportedly passed. Terminal uses dirty rows and shared `Arc<[Run]>`; up to 127 tests reported. Render X11 added a 4 MiB text A8-mask cache, centralized pixmap invalidations/XRender picture caching, reused backdrop pixmaps and removed some pixel clones; 312 render-X11 tests reportedly passed. Renderer has `layout_revision` and compares paint revision separately; recent archive shows render-X11 **already consumes layout_revision** (so old transcript note saying it did not is now obsolete). Explorer skip-on-Hover, blocking inotify and listing caches were worked on; update from live tree. WM motion coalescing, duplicate position early-out, frame-shape and input-layout comparisons were worked on. Shell drift checking has revision-key caching. A warm `tools/fast-bench.sh` ran in about 4 seconds (cold compilation was far slower), but these are work counters, not whole-session CPU savings. Xephyr canary was run, *not* interactive full-path verification. Another user report cites up to ~80% CPU during rubber-band resizing and ~40% for terminal typing/htop on the production laptop. That makes native-event and X server attribution urgent.

## 1. Direct source comparison: what COSMIC really does, and what not to copy

| Mechanism | COSMIC evidence | FlameWM 1657 evidence | Transfer judgement |
|---|---|---|---|
| Event-queued rendering | `cosmic-comp/src/backend/x11.rs` `schedule_render`: sets `dirty`, pings only when `!pending`; `backend/kms/surface/mod.rs` `queue_redraw`: queued/vblank state prevents duplicate renders | Render X11 `app/render.rs::redraw_if_dirty` already skips when nothing dirty; shell/desktop have native event loops | **ADAPT** coalescing/no redundant wakeups; don't introduce unconditional 60 Hz animation or Wayland semantics |
| Damage-aware composition | `cosmic-comp/src/backend/render/mod.rs` `OutputDamageTracker::render_output` and `damage_output`; COSMIC panel `space/render.rs` uses damage tracker only if `is_dirty && has_frame` | FlameWM `app/render.rs::redraw` does a **full clear, full command paint loop, full `XCopyArea`** after passing its dirty gate | **TOP PRIORITY**: make actual X11 region damage flow through producer → scene → clear/paint → present; no X11 `OutputDamageTracker` port |
| Blur | `cosmic-comp/src/backend/render/wayland/blur_effect.rs`: bounded geometry + blur support radius, per-surface state/commit, cached renderer texture, multi-pass GLES shader | FlameWM `backdrop.rs::blurred_argb_words` already downsamples by 4 for sufficiently large radii and reuses scratch; `app/window_backdrop.rs` maintains wallpaper/root snapshots and reuses uploads | **ADAPT** region/support bounds, generation keys and retained backdrop; **DO NOT** copy GLES shaders/GL renderer into the XRender/CPU backend by default |
| Window resize | COSMIC schedules output render on actual surface state change and damage; smithay tracks moved-element old+new geometry | `flamewm-wm-x11` uses position-only move, incremental geometry update; shape/input cache added; root captures and shape operations still require scrutiny | **ADAPT** state-delta and bounded dirty regions, no new compositor or frame pacing that changes input latency |
| Selection rectangles | `cosmic-files/src/tab.rs::select_rect` scans **every item** on each rectangle update and checks overlap | FlameWM Desktop uses **frozen item rects plus differential toggles**; Explorer `main.rs` still projects from broad `App::project()` and holds a `rubber` rectangle | **REJECT** full COSMIC Files loop for Desktop; **IMPROVE** Explorer updates and common overlay geometry selectively |
| Popup lifecycle | `cosmic-panel/cosmic-panel-bin/src/space/render.rs` tracks `is_dirty && has_frame`; per-popup rendering/damage; `cosmic-launcher/src/app.rs` message→state→view | FlameWM shell `runtime.rs::toggle_start` uses native transient surfaces and retained layout; static analysis reports one Start projection per open | **ADAPT** dirty-before-draw, retain icon/row content, popup-specific draw; **DON'T** copy COSMIC/iced widget tree wholesale |
| Memory | COSMIC uses GL textures/outputs/render-element buffers, with cached textures and temporary offscreen blur | FlameWM has ARGB32 backbuffers, XRender pixmaps, temporary snapshots/caches | **ADAPT** resource lifetime and budget accounting; copying extra COSMIC caches could **increase** memory |

**Critical limitations:** COSMIC is a Wayland compositor with scene access and GPU renderer. FlameWM is a **non-compositing X11 window manager** with X11/XRender-native retained UI; it cannot always read obscured pixels of other windows. A root screenshot, XComposite redirect or a new full-screen compositor is not a free substitute. Do **not** promise true live behind-window translucency as a byproduct of a damage patch. Preserve FlameWM's accepted transparent/blurred wallpaper/backdrop compromise, corner shapes, high-DPI rules, pixel parity and low-latency drag. COSMIC Files' selection loop and COSMIC blur GPU pipeline are **negative controls**: reject them if slower/incompatible. Never introduce `smithay`, `cosmic-*`, `iced`, GTK, Qt, GL context or Wayland dependencies into FlameWM's X11 hot path solely because COSMIC uses them.

Upstream references to consult in addition to vendored source:
- Smithay `OutputDamageTracker` rationale: https://docs.rs/smithay/latest/smithay/backend/renderer/damage/index.html ; also https://android.googlesource.com/platform/external/rust/android-crates-io/+/fd55dc012125592ab72ea3a66c592999c1cf0c0e/crates/smithay/src/backend/renderer/damage/mod.rs
- Smithay damage-aware `RenderElement` contract: https://docs.rs/smithay/latest/smithay/backend/renderer/element/index.html
- Smithay renderer frame clip/damage API: https://docs.rs/smithay/latest/smithay/backend/renderer/trait.Frame.html
- COSMIC maintained source/reference locations (read **the vendored commit first**): https://github.com/pop-os/cosmic-comp ; https://github.com/pop-os/cosmic-panel ; https://github.com/pop-os/cosmic-files

## 2. The currently most expensive architectural gap [F]

`crates/flamewm-render-x11/src/app/render.rs` (1657): `redraw_if_dirty` merges `SurfaceDamage` and skips truly empty frames, but `redraw()` always calls `refresh_window_backdrop`, `clear_scene`, full `paint_surface_backdrop`, iterates every retained paint command, `present_scene` copies the entire `self.width × self.height` backbuffer via XCopyArea, binds background and rebuilds/refreshes shape coverage. The retained `layout_revision` successfully avoids *layout* when styling only changes. **What it does not yet avoid is full pixel work for a single small paint change.** `RuntimeDocument` `touch_paint` can avoid layout, but still marks `SurfaceDamage::Full`. Neither a small repaint producer nor a consuming clip exists end-to-end. This is the main **cross-crate** target; isolated tweaks to callers or `XCopyArea` are insufficient.

`crates/flamewm-render-core/src/model/document.rs` stores `revision`, `layout_revision`, `pending_damage`; `touch()` bumps both and sets Full; `touch_paint()` bumps paint revision only, but also sets Full. `crates/flamewm-ui-x11/src/surface_runtime.rs` passes surface dirtiness to the renderer. The correct fix is a **retained layout bounds → paint damage regions → clip-aware renderer**, not a second ad hoc per-feature redraw engine.

### 2.1 Proposed canonical contract (original Rust pseudocode, adapt to actual code)

Use **one** damage authority and **one** coordinate conversion policy. Names below are illustrative; do not paste types over existing equivalents.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect { pub x: i32, pub y: i32, pub w: u32, pub h: u32 }

#[derive(Clone, Debug)]
pub enum Damage {
    None,
    Regions(SmallVec<[PxRect; 8]>), // use Vec if smallvec isn't already in Cargo.lock
    Full,
}

fn invalidate_transition(out: &mut Damage, old: Option<PxRect>, new: Option<PxRect>,
                         paint_halo: u32, viewport: PxRect) {
    // Old+new, never only new: moving/hiding leaves pixels at the old location.
    for r in old.into_iter().chain(new) {
        let expanded = inflate_and_clip_safely(r, paint_halo, viewport);
        out.add(expanded); // merge overlaps; promote to Full above measured thresholds
    }
}
```

`paint_halo` must account for blur filter support, rounded anti-aliasing, shadows, borders, focus rings and text shadow. Use physical pixels after DPI conversion and *ceil* outward (no fractional 1px seams). Damage should be a bounded vector (e.g. up to 8–32 regions) and promote to Full if covered area becomes too large or high rectangle count is slower. **Thresholds are benchmark results, not fixed architecture axioms.** When a node changes layout, capture its old bounds from previous retained layout and new bounds from the new layout; don't calculate damage from only the new tree. When visibility, opacity, z-order, clipping, theme, font, scale, transform, root size, scroll, and output change, invalidate all affected dependent bounds or conservatively Full.

Potential staged Rust API:

```rust
struct Revisions { layout: u64, paint: u64 }
struct LayoutCache { key: LayoutKey, nodes: Vec<MeasuredNode> }
struct PaintCache { key: PaintKey, commands: Vec<PaintCommand> }
struct SurfaceState { revisions: Revisions, damage: Damage, layout: LayoutCache,
                      paint: PaintCache, backbuffer: Pixmap }

fn record_paint_change(&mut self, node: NodeId, reason: PaintReason) {
    self.revisions.paint = self.revisions.paint.wrapping_add(1);
    self.dirty_nodes.insert(node);
    self.pending_reasons.insert(reason);
    // The layout-to-pixel stage, which knows OLD/NEW geometry, resolves these into regions.
}
```

**Required dependency chain:** semantic mutator→dirty node/reason→retained old/new layout and paint extents→surface-aware physical region→XRender/Xlib clips→backbuffer paint in that clip→only matching XCopyArea regions→correct expose and shape. It is NOT complete until real Desktop/Explorer/Terminal/Start consumers exercise it and bypasses are retired. Use a single `repaint_region`/`damage_context` authority in existing render-core/render-X11, rather than feature classes clearing and painting their own surfaces.

### 2.2 Clip-safe incremental clear/paint/present

**Prototype only after region producers and deterministic equivalence tests exist**. Xlib clip calls shown are schematic: real code must honor Xlib's GC/XRender-picture clip separately and never reuse a clip across unrelated drawables.

```rust
// Painter owns the saved backbuffer content. For each coalesced physical damage rect:
set_gc_clip_to_rect(backbuffer_gc, rect);
clear_argb_rectangle(backbuffer, rect);  // zero alpha, not opaque black
paint_surface_backdrop_clipped(rect);     // crop from VALID retained backdrop
for command in spatial_commands_touching(rect) {
    paint_command_with_canonical_clip(command, rect);
}
reset_gc_clip(); // guarantee cleanup even on early error; guard/RAII recommended
copy_backbuffer_rect_to_window(rect);
```

Do **not** independently change XCopyArea to a small region while `clear_scene` and XRender drawings ignore clips: XRender does **not** necessarily honor a GC's clip. XRender Picture clip and Xft drawing clips require their own handling. Reuse existing retained commands but avoid scanning all commands on each tiny region after proving this is costly; optional spatial index per layout revision (`Vec` of paint command bounds or row/bucket bins) must be profiled against a simple filtered scan first. Xlib immediate drawing hazards: GC clip state, XRender `Picture` clip, Xft drawing, transparent clears, AA edges, shape masks, retained backbuffer background binding, Expose, parent/child clipping and selection overlay layering.

**Incremental equivalence harness (mandatory before enabling by default):** render the *same deterministic scene/event stream* with `Full` and `Region` backends to separate Xvfb/Xephyr displays, capture the output after each event, compare pixel hash/diff and screenshots. Scenarios: 1px position shift, toggle visibility, hover, selected/unselected, range selection, partial text update, opacity, blur and window shadow, inherited parent opacity, rounded clip, z-order switch, overlapping translucent children, resizing, offscreen/negative x, 100–150–200% scale, theme/icon/font change, Start slide reversed mid-close, desktop with transparent wallpaper. Alpha correctness matters, not just RGB. A bad region mode MUST fall back to full repaint safely, not leave stale pixels.

**Counters:** `render.frames`, `render.surfaces_skipped`, `render.layout_passes`, `render.paint_command_visits`, `render.clip_area_px`, `render.copy_area_px`, `render.shape_calls`, `render.gc_clip_changes`, `render.xrender_composites`, `render.get_image_bytes`, `render.put_image_bytes`, `render.flushes`, `render.client_cpu_ns`. Assert a single selected row updates only necessary region and does not force relayout or full clear; compare output with full mode. If box-shadow and blur require giant dirty extents, explicitly report real saved area.

## 3. Transparency: preserve pixels and don't invent a compositor

**Current [F]:** `app/window_backdrop.rs` maintains captured snapshots and a wallpaper fallback; the root blur cache is effectively a root-sized image. `render.rs` clears the ARGB scene to transparent before painting. Its present binds the last backbuffer as window background so Expose can reuse it. Earlier tests showed users expect Explorer/Terminal transparency and a correctly shaped shadow; a background image of obscured windows under X11 is not generally obtainable on demand without composition/rearranging windows.

**COSMIC transfer:** track per-element opacity/dirty region and persistent render resources, not the Wayland scene implementation. Confirm effective alpha is applied **once** at the canonical compositing layer, never repeated per nested surface. Avoid `GetImage` on pointer motion, moving/unmapping ancestors to refresh a backdrop, root-sized read for a tiny popup, or repeating pixmap uploads when source pixels/size have not changed.

Recommended bounded strategy:
1. Differentiate **alpha-only** (no source recapture) from **backdrop-source** (wallpaper/theme/root composition) invalidation.
2. Track `BackdropKey {source_revision, root_dimensions, region, blur_radius, scale, appearance_revision}`. A position-only move should change source crop coordinates **without re-blurring the same source**. Prefer a borrowed view or a cached region over cloning full-root words. Existing code already reuses some captures, so measure before revisiting.
3. If the current root/wallpaper fallback is acceptable, keep it. If more current behind-window content is required, file a **separate explicit compositor design decision** with CPU/GPU cost, interop, and correctness tests; it is *not* authorized merely by this performance handoff.
4. Ensure force-clear on full repaint, rebind same-size backbuffer without allocating a new pixmap, clip consistently through shape, and verify that transparent areas do not become black/opaque or show stale wallpaper when moving/resizing.
5. Trigger updates from explicit wallpaper/theme/output/window-source generations rather than elapsed-time polling.

## 4. Blur: constrain the affected pixels and bound state/memory

**Current [F]:** `crates/flamewm-render-x11/src/backdrop.rs::blurred_argb_words` uses `BLUR_DOWNSCALE=4` for radius >= 8px, with reusable scratch; `flamewm-image-core/src/blur.rs` already has downsample and exact-output tests. **Do not replace it with COSMIC's GPU multi-pass renderer or an unbenchmarked SIMD rewrite.** Under current X11 architecture, source readback and upload often dominate the blur kernel. Focus first on **avoiding capture, blur and upload**, not making blur inner loops marginally faster.

COSMIC blur sources: `cosmic-comp/src/backend/render/wayland/blur_effect.rs::{BlurState,BlurElement::internal}`. The blur element expands its bounds by shader support radius, marks a commit when renderer/params/region/size change and retains a compatible GL texture; `capture_framebuffer` still allocates a temporary offscreen texture, so COSMIC is *not* a zero-allocation blueprint. Translate the **generation-key and support-region contract**:

```rust
struct BlurKey {
    backdrop_generation: u64,
    sample_rect: PxRect,    // root coordinates INCLUDING radius halo
    radius_bits: u32,
    downscale: u8,
    scale_bits: u32,
    alpha_bits: u32,
}
struct BlurEntry { key: BlurKey, width: u32, height: u32,
                   bytes: usize, last_used_frame: u64, pixmap: Pixmap }

fn needs_blur(old: Option<&BlurEntry>, new_key: &BlurKey) -> bool {
    old.is_none_or(|entry| &entry.key != new_key)
}
```

Use only when source genuinely immutable under the key; a wall/window change behind an active blur invalidates source even if geometry stayed constant. Cropping requires a **halo** at least the actual downsampled blur support, otherwise seams/different edge pixels result. A too-small crop with `crop_clamped` mirror extension is NOT visually equivalent. Use a safe full-size fallback when provenance/validity cannot be proven. Cache in bytes with explicit cap and eviction; don't simultaneously keep decoded original wallpaper, full blurred root, unblurred root, cropped buffers, uploaded pixmaps and old generations without measuring PSS. Expose `blur.calls`, `blur.cache_hits`, `blur.input_pixels`, `blur.output_pixels`, `blur.bytes_allocated`, `backdrop.get_image_bytes`, `backdrop.uploads`, `backdrop.recaptures`.

**Profile-first candidates:** root-to-window crop vs whole-root snapshot; source-scoped blur with halo; same-size X pixmap reuse; wallpaper source downscale/revision caches; lazy blur on Start *open* and no recompute on *close*; owner-preserving memory reuse. Any cache that improves CPU by wasting tens of MiB or introducing stale results is a regression and must be rejected.

## 5. Window moving/resizing: separate gesture state from final commitment

COSMIC transfer is **damage of old+new element geometry, deduplicated rendering requests, explicit frame lifecycle**. FlameWM's existing WM already sends efficient position-only ConfigureWindow requests for move and has motion coalescing. Do NOT copy compositor redraw on each X11 pointer event; no full scene snapshot per motion. In snapshot, `crates/flamewm-wm-x11/src/wm/pointer.rs::handle_motion` has a repeat-motion early-out but still evaluates edge-drag dwell. `wm/client_state.rs::clear_frame_shape` and `wm/paint.rs::layout_input_children` have caches but need shape-change correctness tests.

Required investigation:
- For each 1-second drag or resize, count X request classes via a private Xephyr/Xvfb/XTrace-like test or local wrappers: ConfigureWindow, CopyArea, Composite, FillRectangle, Shape, QueryTree, GetGeometry, GetImage, PutImage, Sync/Flush, Expose and native events. Distinguish **WM CPU, shell/desktop CPU, Xephyr/Xorg CPU**, and kernel/user time. A slow Xorg can make a low-CPU FlameWM process look misleadingly optimized.
- Collapse bursts of motion to latest position **per event-pump dispatch** while processing release, drag dwell and snap transitions correctly. Never throttle input to a timer in a way that increases drag latency; coalescing events already queued is different from delaying them.
- Move path: configure client/frame and cheap retained shadow position. Do not recapture/reshape/repaint text/chrome/root unless actual dependency changed. Respect ICCCM ConfigureNotify needed by clients.
- Resize path: skip identical sizes/shapes/child placements; compute decoration geometry once per change; limit root backdrop refresh during gesture; update terminal/grid size only when cell dimensions or client resize actually changes. Carefully distinguish interaction preview and settled commit; never defer the client's real resize indefinitely.
- Backdrop: `wm/paint.rs::begin_drag_backdrop` root-sized read at drag start was previously identified; **only** adopt a region capture if padding and origin mapping reproduce blur exactly. It is a one-time read per drag, not automatically the largest steady-state cost.
- Fix the **client SHAPE cache invalidation** risk: an independently shaped client changing its X SHAPE after management may now bypass a needed re-INTERSECT. Add shape notification/invalidation or keep the safe old path for that client. Require live rounded-corner, maximize/minimize/restore, snap, move, resize and shadow-stacking comparison.

Example state logic, adapt to existing controller:

```rust
fn on_motion(&mut self, latest: RootPoint, now: Instant) {
    self.evaluate_edge_dwell(latest, now); // always, even if position unchanged
    let target = self.snap_target(latest);
    if self.last_applied == Some((latest, target)) { return; }
    self.last_applied = Some((latest, target));
    self.apply_move_and_shadow_delta(latest, target); // WM native owner only
}
```

Full refactor only if event/paint separation is still entangled: keep `ControllerState` gesture/intent authority, have one `FrameEffects` result containing changed geometry/shape/decoration/damage flags, then one WM X11 executor that sends only necessary requests. Migrate **move first**, then resize, then snap/restore; remove duplicate emission in touched call sites. No new generic framework unless all real consumers migrate in the same wave.

## 6. Desktop + Explorer selection rectangle: transient overlay, not whole-document mutation

**DO NOT PORT** `cosmic-files/src/tab.rs::select_rect` directly: it loops all items on *each* drag update; FlameWM Desktop's current diff algorithm is more efficient when selections change sparsely. COSMIC's *semantic lesson* is that selection is temporary state outside permanent filesystem mutations.

**Desktop:** preserve `rubber_snapshot`, `diff_rubber_hits`, no-op motion detection, changed-item-only visual updates, and `drop_missing_selected`. Verify grid creation once at drag start and again only on a real layout-generation change. Selection rectangle should be drawn as a transient retained **single overlay node**, with old+new rectangle damage (possibly four thin changed bands), not per-icon full reproject. Ensure keyboard modifiers, right-click, dragged icon hiding, multi-output DPI, scroll and item removal during drag remain correct.

**Explorer:** Source 1657 `main.rs` has `App::rubber` with a position-only update on `UiActionPhase::Motion`; `project()` repositions/resizes `selection-rectangle` and also walks toolbar, rows, scrollbar and status unless projection is more specifically gated. Earlier agent skipped `Hover` reprojections and cached listing data; **Motion during rubber selection may still trigger broad projection**. Introduce a **semantic overlay-update fast path** in the existing Explorer controller → retained UI document, not a second standalone X11 overlay; `update_rubber_overlay(old, new)` should update only the node's semantic bounds, and old+new damage should be derived by canonical renderer. Keep selection commit semantics (in list view `select_vertical_band` on release) and real filesystem selection unchanged. Do NOT change Explorer's vertical band into arbitrary full-list rectangle logic just because COSMIC Files does; inspect mode and behavior first.

Minimal deterministic tests:
- 100 repeated identical motions: zero revised nodes, zero paint regions, zero X work beyond required input dispatch.
- 100 alternating small/large rectangle changes: union of old/new overlay damages only; cached row values and icons unchanged; selections identical to slow canonical reference.
- Selection across 10 / 100 / 1000 / 10,000 items: **O(changed membership)** incremental update after O(N) snapshot, with bounded memory; if extreme cases are hot, evaluate spatial bins/row interval indexing only if baseline shows O(N) scanning dominates.
- Pixel parity for XOR-ish edges, translucent fill, rounded selection highlight, list scroll changes, and 1px mouse steps.

## 7. Start menu: show/hide existing surface, don't rebuild the desktop

**COSMIC:** panel `space/render.rs` allows rendering only when `is_dirty && has_frame`; separate popup damage trackers. **FlameWM:** `flamewm-shell/src/runtime.rs::toggle_start`, `open_start_group_measured`, `close_transients` already manage a parent-anchored native Start surface. Transcript static analysis says `refresh_dynamic` exits before mutation on settled ticks, and Start projection happens once on open. Thus **do not perform a sweeping Start rewrite without an X event trace**.

Examine exact sequence: panel click→transient ownership/focus and pointer grab→Start surface initialization (warm/cold)→projection/icons→X11 map/unmap/slide animation→shape mask, blur, shadow, full present, Expose and taskbar/desktop wake. Benchmark after initialization and separately first-ever open. A Start toggle can trigger high Xorg even when shell's semantic projection is cheap: X server BackingStore, ARGB upload, animation pixmap operations or Expose fallout.

Candidates, conditional on proof:
1. Persist Start's compiled template/layout and precomputed static rows/icons between opens; rebuild only when app catalog/theme/scale/search/query/output placement genuinely changes.
2. Warm the native surface without leaving it mapped or stealing focus (if lifecycle already does so; otherwise preserve lazy allocation when total memory better). Reopen should map the retained valid backbuffer.
3. For animation, compare **move/opacity of retained bitmap** vs repaint-per-position; do not lower 60Hz responsiveness just to reduce reported CPU. Use event-driven damage and no post-close tick chain.
4. On close: release grab once, stop only relevant animation; one clean unmap/hide and correct focus restore. Avoid rerendering desktop and relayouting unrelated panels.
5. Verify frame-interval, old/new popup geometry across outputs, hover/search input, nested categories, no flicker and click-through after close.

Do not copy COSMIC launcher/iced architecture. Borrow dirty/queued semantics and independently retained popup resources where they outperform FlameWM's current path.

## 8. Conditional end-to-end refactor (AUTHORIZED ONLY IF IT IS THE SMALLEST COMPLETE FIX)

The largest savings likely need an end-to-end **damage ownership correction**, not more per-feature caches. Agent MAY refactor materially **only if** it first proves the existing API cannot carry accurate region damage or the number of separate bypasses makes smaller fixes incomplete. Work within the **existing** canonical crates; no separate compositor/UI framework.

**Candidate target layering, preserving existing crates:**

```text
flamewm-desktop / flamewm-explorer / flamewm-terminal / flamewm-shell
     semantic state/actions; no raw X operations; transient overlay intent
              |
flamewm-ui-core + flamewm-render-core
     stable node IDs, layout revision, paint revision, node dirty reasons,
     previous/new bounds, retained scene and damage set
              |
flamewm-ui-x11
     surface lifecycle, event-to-intent, output scaling and scheduler,
     no divergent per-feature relayout/paint
              |
flamewm-render-x11
     one clipped clear + paint + XCopyArea/present, pixmap lifetime,
     XRender/Xft clip, alpha/blur support area, shape, resource budgets
              |
flamewm-wm-x11
     native frame/client geometry, configure, shape and stacking
```

**Strict migration:** (R0) freeze current baseline (commit, dirty diff, build/package, golden screenshot/measurements); (R1) implement `PaintDamage` and dirty-node mutation in render-core with tests; (R2) join retained layout/paint bbox to damage, old+new regions, and physical scale; (R3) attach renderer region-mode behind a debug feature/config that defaults to Full until parity passes, preserving one backbuffer owner; (R4) move **one real consumer** (Explorer rubber or Terminal single dirty row) through this path end-to-end, prove region is non-Full and screenshot matches, **delete its obsolete full-mark bypass**; (R5) migrate Desktop transient selection, Start, remaining text styles; (R6) promote region mode to default only after live tests across scaled, transparent, shaped windows; (R7) remove temporary debug scaffolding except permanent regression/metrics. A new abstraction without migrated consumers is a failed migration.

**When NOT to refactor:** if XRender/Xft clips force repaint of the whole scene or blur dependency expansion covers most of the surface most of the time, benchmark first; choose a targeted cached backdrop or optimized full repaint rather than adding a 1,000-line damage framework with negative ROI. If a native child surface can be moved cheaply but a separate X11 window for selection creates stacking/input/focus problems, retain the current surface and fix the canonical renderer instead. Strong negative evidence overrides aesthetic preference for a cleaner abstraction.

**Memory constraints:** one owner per ARGB resource. Budget **bytes** for full-root buffers (4 × W × H each, so ~7.9 MiB per 1920×1080 copy and ~31.6 MiB per 3840×2160 copy, before extra vectors/pixmaps). Track resident/PSS, Rust heap and X server pixmaps; a “saved CPU” cache can simply move pressure to Xephyr/Xorg. Explicit cache invalidation on XID reuse, texture/pixmap free, root resize, theme change, font change, scale and shutdown; use the existing centralized free path, do not reintroduce a second one.

## 9. Workload baselines and fast measurement protocol

**Never conflate CPU percentage units.** `%CPU` shown by top/htop is usually relative to one logical core; read `/proc/<pid>/stat` `utime+stime` deltas and `/proc/stat` clock ticks, include FlameWM WM/Desktop/Shell/Explorer/Terminal and **Xorg or Xephyr**. Report total core-milliseconds for each fixed-duration scripted workload, median of 3–5 quiet repetitions, p95 input-to-visible-change where measurable, allocation bytes, peak/PSS and X request/pixel counters. A fast scoped regression/counter loop can run often, but do one short native test every coherent batch. Do not compile during CPU sampling. Set a stable performance CPU governor only with explicit authorization; avoid host-wide changes.

Required workloads:

| Workload | Scripted behavior | Main quality invariants |
|---|---|---|
| Idle | Desktop settled, no pointer activity, no background fetch for 10s | No wake/render loop, clock/network integration still works, normal periodic tasks isolated |
| Transparency | Explorer + Terminal idle and moving behind static wallpaper/other windows | Same alpha/corners; no black region, no root recapture on motion |
| Blur | Start/Explorer show; 0/50/100% blur and 100/150/200% UI scale | Identical supported visual quality; bounded blur calls and memory |
| Move | 60 Hz synthetic pointer motions, move window 400px and back, repeats | Smooth frame motion, snap dwell, input responsiveness, titlebar/shadow stability |
| Resize | 60 Hz resize varying width/height and return, include maximized restore | No stale corners, correct child geometry, no frame flicker |
| Desktop selection | 60Hz small→large→small rubber rectangle; varying icon populations | Exact hit selection, no ghost overlay, correct icon selection |
| Explorer selection | 60Hz band/rect drag across rows, with scroll | Correct native list/selection result, no full row relabeling |
| Start | Open/close 30× after warmup, and cold first open separately | Correct focus, animations, icons, menu response, no lingering work |

**Example runner commands** (verify script flags with `--help`, do not guess an unsupported invocation):

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender
pwd; git status --short; git rev-parse HEAD
rg --files .vendor | rg 'cosmic-(comp|panel|files)' | head -30
bash -n tools/fast-bench.sh && tools/fast-bench.sh
python3 tools/bench-flamewm-cpu.py --help
./scripts/xephyr --help
cargo test -p flamewm-render-core -p flamewm-ui-x11 --locked
cargo test -p flamewm-render-x11 --locked
cargo test -p flamewm-desktop -p flamewm-explorer -p flamewm-wm-x11 --locked
cargo check --workspace --all-targets --locked
```

**Native evidence:** use the project's `scripts/xephyr` and private per-display D-Bus test harness; choose an available display (the prior log used `:207`; do not assume it is free), never send automated XTEST to host `:0`, do not alter production profile/wallet. For Xvfb gated tests use `xvfb-run -a` where available. Take screenshot from within the isolated display and inspect manually. A nested Xephyr measurement captures Xephyr overhead, not production Xorg GPU driver behavior. Also run a short read-only production measurement with user authorization for true Xorg/GPU-driver attribution; **do not hide high Xorg usage by reporting only FlameWM PID CPU**. A headless test proving correct counters does not prove animation is smooth or low-latency.

**Acceptance:** set an aspirational ≥90–95% aggregate CPU drop for expensive workflows where possible, but never turn it into a fabricated gate. At minimum, show **measured** reduction vs the same current-HEAD baseline after the batch, no statistically significant regression in latency/visuals, zero extra idle repaint beyond legitimate scheduled tasks, and no large PSS/X server memory increase. If 95% is physically impossible under this X11/CPU rendering policy, report the measured attainable Pareto frontier and explain why (e.g., unavoidable full-window operations or X server compositing work). P95 frame latency and responsiveness trump a cosmetic CPU headline.

## 10. Autonomous agents, READY frontier and ownership (MANDATORY)

**Coordinator is control plane only while async/subagents exist.** First invoke a read-only `explore` subagent if the harness supports it; delegate repository operations, discovery, edits, compilation, Xephyr, profiling and commits. Coordinator holds concise source-fact capsules, dependency/ownership graph, READY frontier, verification status and blockers, not extensive pasted source. Use asynchronous background agents and *saturate all disjoint READY lanes*, target up to six if available, **but do not launch six Cargo compiles**. All builders must own exclusive files and return `PATCH_APPLIED_NEEDS_VERIFY`/`BLOCKED`/`FAILED`; **only a fresh independent verifier** may return PASS. Never reuse an agent's self-reported test success as final certification.

**Initial parallel read-only wave:**
- Scout A `cosmic-comp`/Smithay output damage/scheduler/blur region vs render-X11; detailed exact source anchors and tradeoffs.
- Scout B `cosmic-panel`/`cosmic-launcher` native popup caches vs FlameWM Start/animation/renderer, including X event timing.
- Scout C COSMIC Files rectangle selection vs FlameWM Desktop/Explorer; prove whether Cosmic is slower on each workload; report reject/adapt outcomes.
- Scout D FlameWM WM/X11 move/resize, readback and round-trip graph; no edits.
- Scout E render-core/ui-x11 revision/damage producer/consumer/bypass audit, dependency order for complete migration.
- Scout F native benchmarking, visual test/packaging readiness, current agent ownership and source drift; no edits.

Compute entire READY frontier and then **launch all disjoint implementation lanes concurrently** with exact ownership, e.g. Wave 1: (1) damage-model + tests in `flamewm-render-core`; (2) WM gesture/shape metadata in `flamewm-wm-x11`; (3) Explorer controller/project selective overlay in `flamewm-explorer`; (4) Start popup lifecycle/instrumentation in `flamewm-shell`; (5) benchmarking/X11 counters under `tools/` (6) backdrop A/B tests in image-core only, **if performance proof suggests a new algorithm**. Do NOT have render-X11 renderer implementation modify damage-model files while another builder owns them. After model verification, Wave 2: single render-X11 owner consumes region damage, other disjoint builders tackle Desktop/Explorer/Start feature wiring as ready. One exclusive Cargo/build owner manages a shared target dir and checks; other builders run source static checks/counter-only tests while the build owner is busy. One exclusive X server / Xephyr owner. Avoid OOM and disk thrash from per-agent target directories and parallel release builds; reuse a warm scoped target as governed by repo instructions.

**Dependency DAG:** Source freeze→read-only scouts→[canonical damage model, benchmark harness, WM independent changes, Explorer lightweight semantic changes, Start instrumentation]→independent focused verifier→[render-X11 region consumer + bound/correctness tests]→[real consumer migration and removal of Full bypass]→native Xephyr screenshot/performance tests→full workspace build/tests→Debian packaging→installed artifact inspection→final independent verifier. On failure, open a disjoint repair lane for the first failing canonical owner, preserve other successful patches and re-verify. Recompute and fill the READY frontier after every wave; no artificial idle agents. If a bug survives, write an exact deterministic regression before any second attempted repair. Every second retry on same hypothesis requires a new discriminating observation. Agents must not ask the user to relaunch the task merely because one subprocess/profiler took too long; switch to bounded scoped probes and continue other tasks. An explicit safety/tool-denial is a real BLOCKED constraint, never bypass it.

**No slop refactoring:** prioritize fewer lines and simpler canonical owners only when actual consumers use them and redundant paths disappear. A source reorganization with no measured gain or maintainability win is out of scope. No whole-workspace `cargo clean`, lockfile churn, unrelated OS dependencies, unapproved root/system changes, cache-invalidating unscoped formatter, and no modifications of `.vendor` upstream sources. Preserve current other agents' uncommitted work and their X11/Cargo ownership; coordinate rather than overwrite or cherry-pick blindly.

## 11. Required tests / zero-regression contract

**Render/damage:** identical mutation no revision/damage; paint-only change no layout; geometry change invalidates old+new; alpha/blur spreads halo; clip/out-of-bounds; theme/font/icon/scale generation; XID-free invalidation; image equality with full renderer; no blank/black transparent regions; damage promotion to Full at threshold; expose on partially obscured native windows; concurrent hover + popup close.

**WM:** move with edge dwell unchanged; resize forward/back and rounding; client SHAPE Notify invalidation; maximize→minimize→restore still maximized; snap transitions; stable shadow below managed titlebars; no drag flicker; frame and child shapes correct at all corners and DPI; no needless root GetImage on each pointer motion; ICCCM/EWMH correct.

**Selection:** Desktop (sparse and dense layouts, item removal while dragging, shift/control); Explorer (list band + scroll + context menu and row dragging); motion noop; visually identical overlay; old location erased; no per-motion full list/text/icon projection.

**Start:** correct anchored position in every output, nested menu, keyboard search, tabs, focus/grabs, icon theme, opacity and animation; correct immediate reopen mid-close; no per-idle frame after settling; Start close doesn't make desktop repaint the wallpaper unnecessarily.

**Cross-domain:** Terminal typing/htop still correct with row sharing; Dolphin, Brave and KWallet launch/session behavior not changed; taskbar, Explorer, Settings, dynamic appearance, multi-monitor (as testable) unchanged; `.deb` includes Terminal. Build scripts and manifests validate correctly. For externally unavailable hardware tests, mark `UNVERIFIED_EXTERNAL` not PASS.

## 12. Final agent deliverable format and continuous execution

Keep working autonomously through the READY frontier and repair waves. Don't stop at `cargo check` or counter-only `PASS`, and don't spend hours on duplicate full release baseline builds. At each batch report a terse ledger:

```
Baseline: HEAD + dirty state; exact workload/CPU scope
Item: [source owner] [old behavior -> new behavior]
Tests: deterministic gate + exit code
Runtime: screenshots / CPU core-ms / Xorg+FlameWM / p95 latency / PSS
Delta: speedup and memory change; noise/uncertainty
Decision: ADOPT / REVERT / FURTHER MEASURE / BLOCKED_EXTERNAL
Next READY wave: N disjoint agents
```

Before claiming done: exact source commit(s) if authorized, all open regressions/tests, short quiet native Xephyr before/after, screenshot/visual review, complete build/test/package, any remaining external hardware verification explicitly listed. Never claim almost-zero CPU for moving/blurred windows merely because a microbenchmark skipped their pixel work. End with what was skipped/not checked and remaining risks. If a deep redesign proves necessary, implement it end-to-end under §8; do **not** declare a half-migrated architecture complete.

## 13. Extra side-by-side research questions: agent must continue beyond this report

1. COSMIC `OutputDamageTracker` uses opaque-region culling, element commit counters and old/new geometry. Which of these improves FlameWM's **actual** XRender/ARGB scene and which requires full compositor visibility? Implement only measurable improvements.
2. COSMIC panel `has_frame` and per-popup dirty state avoid unnecessary EGL swaps. Does FlameWM flush/present repeatedly on Start close or panel drift despite no semantic changes? Gather real X event trace first.
3. COSMIC's blur state records renderer context, dimensions, blur params and region. Which FlameWM backdrop cache keys are currently missing invalidation on wallpaper, DPI or geometry changes? Avoid stale cache regression.
4. COSMIC Files `select_rect` is O(N) per update. Can FlameWM Explorer use its current ordered rows for **O(changed row interval)** selection without copying COSMIC's slower loop? Prove against high item count and compare GUI output.
5. Does FlameWM's shared renderer reissue an XRender composite per element after `set_gc_clip`, and would a correctly cached XRender Picture clip eliminate wasted work? Scope and validate all backend primitives.
6. Can opaque row/background regions be reused or skipped while preserving exact alpha, rounded clipping and popup overlapped content? Apply only if correctness/perf proved.
7. Is CPU really consumed by Rust or by Xorg (pixmap transfers/round trips) on the production laptop? Use paired per-PID core-ms, X request counters and bounded Xephyr/Xorg tests; don't optimize the wrong process.
8. Can several transient UI elements (selection overlay, drag preview, snap guides) reuse one canonical retained overlay primitive without making native window count/focus/grabs worse? Migrate one real consumer before any generalized framework.
9. Measure tail memory usage of cached text masks, roots/backdrops/pixmaps and last-scene retained commands. Reject caches whose cross-surface duplication exceeds saved CPU value.
10. Check more current COSMIC upstream **after** comparing vendored source, but treat changes as proposals until verified; do not automatically import an upstream regression or slower algorithm.

**End-state criterion:** Every adopted technique must do *less real work* than FlameWM's existing counterpart in a matched test, use no materially worse memory or latency, maintain pixel and feature parity, and follow the canonical native Rust/X11 owner. Other COSMIC ideas are documented as **REJECTED** with reason rather than ported.
