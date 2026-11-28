# z03: render shared buffers — remaster design

## Scope and evidence

Unit: `docs/swarm/wave5/z03-render-shared-buffers.md`. Canonical owner is
`crates/flamewm-render-core`; the typed facade is the new `SharedBuffers`
struct in `glyphs.rs`, re-exported at the crate root. No native or GPU access is
added, no gate is loosened, no dependency is added, and emitted pixels are
unchanged.

Reference-first evidence (read-only, never a dependency or runtime input):

- `.vendor/RustWebRender-0.0.9/crates/rustwebrender-x11/src/lib.rs:169-173`
  creates one retained full-frame `Pixmap` (the backbuffer) at startup and paints
  every frame into it; lines 595-617 recreate and rebind it on resize and free
  the previous one only after the replacement is installed.
- `.vendor/RustWebRender-0.0.9/RUSTWEBRENDER_0.0.9_IMPLEMENTATION_REPORT.md:17,44-48`
  states the same policy: paint the complete frame off-screen, present in one
  copy, retarget on `ConfigureNotify`, drop drawables on destruction.
- Choice: port the *retain-and-refill buffer* idea, not the X11 `Pixmap`,
  `XCopyArea`, or backend rebinding. Native drawables stay in their own owner;
  only CPU coverage bytes are pooled here, with render-core's existing
  clear-and-reuse convention (`pixels.rs:18-25,613-634`, `soft.rs:90-105`).

## Current structure (file:line)

| Path | Line | Role |
| --- | --- | --- |
| `crates/flamewm-render-core/src/glyphs.rs` | 496-563 (pre-change) | `text_mask` allocated a fresh `vec![0u8; w*h]` per text run per frame. |
| `crates/flamewm-render-core/src/glyphs.rs` | 404-418 | `shape_origins`/`caret_positions` allocate a fresh `Vec<f32>` per layout. |
| `crates/flamewm-render-core/src/text_layout.rs` | 171, 255, 305 | `lines`, ellipsis `cuts` and chunk `cuts` allocate per wrapped run. |
| `crates/flamewm-render-core/src/layout.rs` | 179, 204, 253 | `boxes` / `contents` / `z_order` allocate per layout pass (returned, revision-cached). |
| `crates/flamewm-render-core/src/paint.rs` | 109 | `commands` already reserves `z_order.len() * 2`. |
| `crates/flamewm-render-core/src/lower.rs` | 248, 261 | `ops` reserved once; clip `stack` is small and internal. |
| `crates/flamewm-render-core/src/pixels.rs` | 6-38, 613-634 | `BatchScratch`, `INPLACE_REGION`, `BACKDROP_REGION` already reused with a retention cap. |
| `crates/flamewm-render-core/src/soft.rs` | 15-24, 90-105 | `SoftPixels` retains its own frame and region buffers across paints. |

## Counted problems

1. `glyphs::text_mask` (`glyphs.rs:529` pre-change) allocates a fresh coverage
   `Vec<u8>` sized `width * height` on every text paint — the largest single
   fresh per-frame buffer in the render-core text path (a long run is tens of
   KB, repeated for every visible text node).
2. `glyphs::caret_positions` (`glyphs.rs:404-418`) allocates a fresh `Vec<f32>`
   per caret query, used by `text_layout::push_chunks` (`text_layout.rs:307`).
3. `text_layout` builds fresh `Vec<usize>` byte-offset tables in
   `push_ellipsized` (`text_layout.rs:255`) and `push_chunks`
   (`text_layout.rs:305`) on every wrapped run.
4. Reuse is scattered across private thread-locals and backend fields
   (`pixels.rs:6-38`, `soft.rs:15-24`); callers have no typed facade to keep a
   buffer across frames, so the text coverage buffer could not be reused at all.

Number of problems: 4. Problems 2 and 3 are deliberately deferred to later
steps; problem 4 is only partially closed by step 1 (the text facade exists).

## Target design

One typed facade, `SharedBuffers`, owns reusable byte buffers with a single
clear-and-refill policy and a retention cap, mirroring the vendor's
retained-backbuffer lifecycle. Every large fresh per-frame allocation gets a
`*_into` / facade method that fills a caller-owned buffer; the plain public
entry points stay unchanged and delegate to the same rasterizer, so pixels
cannot drift. Callers keep a `SharedBuffers` across frames and reclaim the
mask's buffer after lowering. No native handle, no new dependency, no gate
change.

## Steps (each compiles alone)

### Step 1 — typed shared-buffer facade and tests (implemented here, UNBUILT)

Add `SharedBuffers { coverage }` with `new`, `text_mask`, `reclaim` and
`coverage_capacity` to `glyphs.rs` (`glyphs.rs:69-111`), re-export it from
`lib.rs`, and extract `rasterize_text_mask` (`glyphs.rs:565-628`) so both the
plain `text_mask` and the facade fill the same buffer and emit the same
geometry and bytes. Add `tests/shared_buffers.rs` asserting capacity reuse and
zero allocations on a second (smaller) rasterization, plus byte-for-byte
equality with the plain entry point. Additive: no existing caller is rewired,
no pixel path output changes, no gate changes, no new dependency. This step
compiles alone.

### Step 2 — route text layout scratch tables (not implemented)

Have `text_layout` thread a `SharedBuffers` through `layout_styled` →
`push_chunks` / `push_ellipsized` so the caret and offset tables of problems 2
and 3 reuse capacity. Pure CPU-side change; compiles alone on top of step 1.

### Step 3 — presenter-owned buffers (not implemented)

Have the software presenter keep one `SharedBuffers` across frames and call the
facade's `text_mask` + `reclaim` around lowering, removing the fresh coverage
allocation from the steady-state path. Requires a visual/pixel-identical check
before acceptance; not part of this unit.

## Rollback

Revert step 1 by removing the `SharedBuffers` block from `glyphs.rs`, the
`pub use glyphs::SharedBuffers;` line from `lib.rs`, the extracted
`rasterize_text_mask` (restoring the original inline `text_mask` body), and the
`tests/shared_buffers.rs` file. No other caller references the facade, so the
rollback is clean and pixel-neutral. Steps 2 and 3, if applied later, must be
reverted in reverse order before step 1.

## Verification limits

Builds, tests, and commits are owned by the orchestrator and were not run
here. All edits in this slice are UNBUILT. No runtime or performance claim is
made; capacity reuse is asserted by source-level tests only until the
orchestrator runs them.
