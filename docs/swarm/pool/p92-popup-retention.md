# p92 popup document retention (PERF_HOTLIST row 22)
STATUS: PARTIAL

## Question
Do closed shell helper popups (audio, network, calendar, battery, weather) keep their UI documents and surfaces in memory after close?

## Answer
Yes. Each popup is created once on first open and kept until shutdown. Close only unmaps.

## Evidence (file:line)
- `crates/flamewm-shell/src/quick_controls/host/surfaces.rs:10-18`: `handles: [Option<SurfaceHandle>; 5]` documents "created on its first open and kept until shutdown: close only unmaps".
- `surfaces.rs:52-71` `ensure()`: `create_surface(...)` on first use, stores handle in `self.handles[slot]`, never cleared on close.
- `surfaces.rs:83-90` `close()`: takes `slide`, calls `close_handle()` (which `take()`s only `open`, not `handles`), then `close_helper`.
- `surfaces.rs:95-115` `shutdown()`: the only place `handles` are `take()`n and `runtime.destroy()` is called.
- `surfaces.rs:118-139` `create_surface()`: `decode_document(bytes)` then `runtime.create_surface(UiTemplate::new(document), ...)`.
- `crates/flamewm-ui-document/src/template.rs:770-774` `decode_document`: `decode_static(bytes)` borrows the embedded `.rwr` bytes (`'static`), so image pixels are not copied to heap. The retained heap cost is the compiled node tree and layout state.
- `crates/flamewm-shell/src/quick_controls/host/consts.rs:7-20`: five `include_bytes!` `.rwr` artifacts (in the binary's rodata, not heap).
- `crates/flamewm-shell/src/quick_controls/popup.rs:127-133` `close_helper` and `crates/flamewm-shell/src/popup_controller.rs:259-270` `close_popup`: `hide` when slide is armed, else `close_surface`. Neither destroys.
- `crates/flamewm-render-x11/src/surface_controller/lifecycle.rs:208-236` `close_surface`: ungrab, `XUnmapWindow`, `present_scene`, flush, clears `shown`/`slide_drawn`/`overlay`. The `SurfaceInstance` (window, app, `document`, scene and backing state) stays in `self.surfaces`.
- `lifecycle.rs:179-204` `destroy_surface`: the only path that removes the instance; it defers native library unload (or `mem::forget` when the display is broken).

## What is retained per closed popup
- The `SurfaceInstance` in `surfaces` map: X11 window id, `app` with its scene/backing resources, `document` (node tree + layout), shape mask.
- The X11 server-side window and its backing store, which are not freed on unmap.
- Up to 5 instances total (one per kind), plus any deferred native library handles.

## Size estimate (UNMEASURED)
- Document tree and layout per popup: estimate 50-300 KB heap (not measured; the `.rwr` byte size was not available, no target dir built).
- Backing/scene buffer per popup at 310x220 (see `popup.rs` test): 310*220*4 = 273 KB at 1x; about 1.1 MB at 2x scale. Only applies if the scene buffer is CPU-side and not released on unmap.
- Worst case: 5 popups, about 1.5-2 MB, excluding any server-side window memory. Treat as an upper-bound estimate until measured.

## Fix proposal (minimal, safe)
Do not destroy-and-recreate on close: row 22 notes that recreating alters first-frame pixels and native teardown.
1. Add to `flamewm-render-x11` a `release_backing(id)` (or reuse an existing shrink hook) that, after `close_surface` completes, drops the CPU scene/backing buffers and the shape mask while keeping the window, the `document` and the instance in the map. Re-allocate lazily in `present_scene`/`redraw` on the next show.
2. Expose it through `flamewm-ui-x11/src/surface_runtime.rs` (thin pass-through like `destroy` at :353).
3. In `surfaces.rs` `close()` (:83-90), after `close_helper`, call the new release for that kind's handle. Leave `shutdown()` unchanged.
4. Regression check: unit test in `surfaces.rs` tests (or `quick_controls/host/tests.rs`) that `close` then `ensure` returns the same handle and does not call `create_surface` again; plus a measured before/after RSS via the scoped perf procedure (`.agents/skills/scoped-perf-tests/SKILL.md`): open each kind once, close, sample PSS.
5. Gates: `flamewm-shell` tests and the wasm/x11 build through the normal gate; no image review needed unless first-frame pixels change; capture one popup open after a close and confirm identical pixels.

Do not drop the `document` in step 1 unless the measurement shows it is significant; it is the cheapest thing to keep and avoids re-decoding.

## Next
- Measure the scene/backing size first (grep `present_scene` backing alloc in flamewm-render-x11) before coding.
- Confirm whether the X11 window backing store is server-side (not counted in shell RSS).

## Result
- `crates/flamewm-render-x11/src/app/events/window.rs:141-153` — Finding: only grow-on-resize and settle-time shrink existed; no close-time hook released an oversized backbuffer. Change: added `release_backbuffer_on_unmap()`, swapping down to the window's own size and calling `mark_full()`; window and document are untouched. Risk: a swap failure leaves the buffer oversized (best-effort). UNBUILT.
- `crates/flamewm-render-x11/src/surface_controller/window_state.rs:94-98` — Finding: the immediate close inside `hide()` unmapped and cleared backdrops but kept the enlarged backbuffer. Change: call `release_backbuffer_on_unmap()` after the clears, propagating errors with `?`. Risk: the swap retargets Xft/XRender while the window is still live; safety rests on the retired-backbuffer lifecycle. UNBUILT.
- `crates/flamewm-render-x11/src/surface_controller.rs:589-594` — Finding: a landed slide-out (the primary quick-control close path, `popup_controller.rs:264-267` → `hide()` → `advance_motion`) unmapped at :584 but retained its backbuffer. Change: call `release_backbuffer_on_unmap()` after `clear_node_backdrops()`, discarding the error because `advance_motion` returns `()`. Risk: no reopen-repaint pixel check; `mark_full()` plus re-grow on the next show should repaint every pixel. UNBUILT.
- `crates/flamewm-render-x11/src/surface_controller/tests.rs:1083-1102` — Finding: no guard covered backbuffer release on close. Change: added source-guard test `every_close_path_releases_the_backbuffer` asserting both the slide landing and the immediate `hide()` path call the release. Risk: source-text assertion, not runtime proof. UNBUILT.
- Reference-first evidence (read-only, never a dependency): `.vendor/xfwm4-master/src/mywindow.c:334,344` frees temporary pixmaps once the draw is done; `.vendor/smithay-master/src/backend/x11/buffer.rs:53,185` frees a Pixmap when its wrapper is dropped after presentation. Precedent supports keeping the window/document and retiring the stale pixmap through the existing lifecycle rather than destroying the surface.
- Not verified: no build, no tests, no reopen-pixel comparison, no RSS measurement. The asked shell regression test (`close` then `ensure` returns the same handle without re-calling `create_surface`) was not added: `SurfaceRuntime::new()` needs a live X11 display (`crates/flamewm-ui-x11/src/surface_runtime.rs:131-136`) and the crate has no runtime mock. Retention is structural: `HelperSurfaces::ensure` short-circuits on `handle(kind)` (`surfaces.rs:58-60`) and `close()` never takes `handles` (`surfaces.rs:83-90`).

PARTIAL — all three production edits are in place and the slide-out landing gap noted by the earlier pass is closed, with a source-guard test added, but every change is UNBUILT: no cargo build, no test run, no reopen-repaint pixel check and no RSS measurement were permitted. Pixel identity relies on the retired-backbuffer lifecycle plus `mark_full()` on the next redraw; the shell close/ensure runtime test remains unwritten because there is no mock `SurfaceRuntime`.
