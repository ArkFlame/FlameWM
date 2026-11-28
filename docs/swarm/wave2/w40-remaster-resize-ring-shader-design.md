# w40 remaster: resize ring shader design

Status: step 1 implemented and UNBUILT; steps 2-5 are designed only and are not in this change.
No new dependencies are introduced. Visual output is required to stay pixel-identical throughout.

## Current structure (file:line)

| Location | Role |
| --- | --- |
| `crates/flamewm-render-core/src/outline.rs:20` | `SAMPLES = 4`: every ring pixel is decided by a 4x4 sub-sample grid (16 tests). |
| `.../outline.rs:26` | `ring_alpha(opacity, coverage)`: shared rounding of one pixel's alpha. |
| `.../outline.rs:34` | `inside_rounded(px, py, w, h, radius)`: bounds test plus squared corner distance. |
| `.../outline.rs:49` | `ring_coverage(...)`: supersampled coverage oracle, 16 `inside_rounded` calls per pixel. |
| `.../outline.rs:88` | `rounded_rect_sdf(...)` (added by step 1): sign of the rounded-rectangle distance field. |
| `.../outline.rs:107` | `ring_coverage_sdf(...)` (added by step 1): pure distance-field form of `ring_coverage`. |
| `.../outline.rs:150` | `outline_rgba(...)`: rasterises a full `width * height * 4` straight-alpha buffer. |
| `.../outline.rs:184` | `outline_strips(...)`: cuts the same image into four perimeter strips. |
| `.../outline.rs:226` | `strip_rgba(...)`: per-strip raster, each pixel calling `ring_coverage`. |
| `crates/flamewm-wm-wayland/src/snap_preview.rs:18` | `PREVIEW_RADIUS = 8`, `PREVIEW_THICKNESS = 2`. |
| `.../snap_preview.rs:37` | `RingKey { w, h, rgb, radius }`: cache key; a resize always misses it. |
| `.../snap_preview.rs:44` | `SnapPreview`: owns `strips`, `last_raster`, `pending` and test counters. |
| `.../snap_preview.rs:108` | `show(...)`: paces a size change to one raster per 16 ms and defers the rest to `pending`. |
| `.../snap_preview.rs:141` | `settle(...)`: applies the deferred frame, forcing a raster. |
| `.../snap_preview.rs:149` | `ensure_ring(...)`: rebuilds all strips on a key miss. |
| `.../snap_preview.rs:169` | `elements(...)`: emits one `Elem::Memory` per cached strip. |
| `.../snap_preview.rs:197` | `ring_strips(...)`: converts core strips into `MemoryRenderBuffer`s with premultiplied bytes. |
| `crates/flamewm-wm-wayland/src/render/mod.rs:57` | `render_elements!` includes `Memory` and `Backing`; no shader element yet. |
| `.../render/mod.rs:40` | `premultiplied_argb_bytes` is re-exported from `titlebar` for the strip upload. |

## Problems counted from source

These are counts, not timing measurements; no build or benchmark was run for this design.

1. For a 1920x1080 frame with radius 12 and thickness 2, `outline_strips` computes
   `band = radius + thickness = 14`, so the four strips cover `1920*14*2 + 14*1052*2 = 83216`
   pixels. At 16 sub-samples each that is 1,331,456 `inside_rounded` decisions per raster, plus
   four `Vec<u8>` allocations and four new GL textures from `MemoryRenderBuffer::from_slice`.
2. `snap_preview.rs:302` budgets at most 70 rasters per simulated second, so a one-second resize
   still performs up to 70 strip rebuilds and texture uploads; pacing hides cost but does not
   remove it.
3. The ring is entirely determined by `(width, height, radius, thickness, colour)`. Rasterising it
   for every size re-derives the same analytic function on the CPU, while the GPU can evaluate it
   on one quad with five uniforms.

## Target design

Render the ring as one quad. A fragment shader evaluates the rounded-rectangle ring from
`size`, `radius`, `thickness` and `colour`, so a resize changes uniform values only: no CPU raster,
no strip allocation, no texture upload, no pacing state and no `RingKey` cache. The shader uses the
same 4x4 sub-sample grid and the same rounding as `ring_coverage`, so its pixels are identical by
construction rather than by tuning.

Reference precedents:

- `.vendor/smithay-master/src/backend/renderer/gles/shaders/mod.rs:117` (`texture_program`) compiles
  custom shader variants with extra `UniformName` entries and a `GlesTexProgram`. This is the
  mechanism for passing size, radius, thickness and colour to a custom element.
- `.vendor/cosmic-epoch/cosmic-comp/src/backend/renderer/shaders/clipped_surface.frag:30`
  (`rounding_alpha`) draws per-corner rounding in one fragment shader using a per-corner radius
  uniform and `smoothstep(radius - 0.5, radius + 0.5, dist)`. It confirms the single-quad SDF
  approach used here; our shader keeps the 4x4 sample count instead of `smoothstep` so pixels match.

## Steps (each step compiles on its own)

1. **Pure distance-field oracle (implemented here).** Add `rounded_rect_sdf` and
   `ring_coverage_sdf` to `flamewm-render-core/src/outline.rs` and a test that compares every pixel
   against `ring_coverage` for six sizes and radii and four thicknesses. The functions are additive
   and have no production callers, so compiling this step cannot change a pixel.
2. **Shader source.** Add a GLSL fragment shader under the Wayland crate that evaluates the same
   16 samples from uniform `size`, `radius`, `thickness`, `colour`; register it with smithay's
   `GlesTexProgram`. No callers change, so this step compiles and draws nothing on its own.
3. **Element.** Add a `RingElement` variant to `render_elements!` in `render/mod.rs` that draws one
   quad with the shader from step 2, parameterised by frame size, radius, thickness and colour.
   Nothing emits it yet, so pixels stay unchanged.
4. **Switch the preview.** Change `SnapPreview::elements` to emit one `RingElement` for the shown
   frame and remove `ring_strips`, `RingKey`, `last_raster`, `pending` and the pacing branch from
   `show`/`settle`. This is the first step that changes the draw path; it is gated on a CPU-versus-GPU
   pixel comparison test.
5. **Retire the raster path.** After runtime parity evidence exists, remove the strip raster from
   `outline.rs`, keeping `ring_coverage` and `ring_coverage_sdf` as test oracles for the shader.

## Step 1 detail

`rounded_rect_sdf` returns only the sign of the distance. It reproduces the exact bounds test and
`radial <= corner` comparison of `inside_rounded`; returning a sign instead of `radial - corner`
avoids floating-point subtraction drift, which is why the equality test is exact rather than
approximate. `ring_coverage_sdf` keeps the same early exits, clamps, rectangular case (`radius <= 1`),
4x4 sample positions and final rounding as `ring_coverage`; the outer SDF must be inside and the
inner SDF (the silhouette inset by `thickness`) must be outside. The test
`sdf_coverage_matches_the_supersampled_raster` at `outline.rs:491` walks all pixels of six shapes,
including a clamped tiny radius and an oversized thickness, and asserts equality.

## Rollback

Every step is an isolated additive change that can be reverted on its own. Step 1 has no callers,
so reverting it cannot affect pixels. Step 2 is inert shader source; step 3 adds an unused element
variant. Step 4 is the only behavioural switch: reverting it restores the cached-strip path and
the pacing state. Step 5 keeps both coverage functions as oracles, so it can be reverted by
restoring the raster helpers. `ring_coverage` is never modified, so the old pixels remain the
reference for the whole sequence.

## Dependencies and constraints

No new crate is needed: smithay is already a dependency with the `renderer_glow` feature enabled
(`flamewm-wm-wayland/Cargo.toml:60`). That crate denies `unsafe_code` at the crate level
(`Cargo.toml:96`), so steps 2-3 must use smithay's safe `GlesTexProgram` constructor or a narrowly
scoped, justified module-level allowance; step 1 is safe Rust and needs none of this.

## Open risks

- Exact GPU parity depends on the driver evaluating `radial <= corner` in high precision; the
  shader must declare `precision highp float` and must be compared on real hardware before step 4.
- The rectangular case (`radius <= 1`) has no antialiasing in the current raster; the shader must
  keep that hard edge rather than smoothing it.
- All steps are UNBUILT in this unit; the orchestrator runs builds and tests after the batch lands.
