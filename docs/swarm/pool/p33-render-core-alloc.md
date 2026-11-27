# Unit p33-render-core-alloc
STATUS: PARTIAL

Crate flamewm-render-core: find per-frame heap allocations in hot paths (rg 'Vec::new|to_vec|clone\(\)|format!' in raster/layout/paint); reuse buffers or borrow. Output pixels must stay identical. Max 3 edits, list the rest.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-render-core/src/glyphs.rs:355-376,478-529 | `text_mask` built an origins Vec and a glyph Vec per painted line through `shape` | replaced `shape` with `for_each_glyph`: two identical cached-run walks (bounds, then blit), no per-paint arrays | UNBUILT (no cargo); pen and advance order unchanged, second pass re-reads cached glyphs; eviction between passes can re-rasterize deterministically |
| crates/flamewm-render-core/src/pixels.rs:35-37,243-253 | `backdrop_blur_cpu` passed `&mut Vec::new()`, allocating a region buffer per stand-alone backdrop | reuse a dedicated thread-local `BACKDROP_REGION` with the existing 16 MiB retention cap | UNBUILT (no cargo); no recursive caller found; separate from `INPLACE_REGION` |

Remaining sites (not edited; need broader borrow or cached-state changes):
- crates/flamewm-render-core/src/text_layout.rs:171,255,305,318-329: lines Vec, ellipsis probe/cuts Vecs, one owned String per line.
- crates/flamewm-render-core/src/paint.rs:210: one `Arc<str>` per laid-out line; needs a cached/borrowed text source.
- crates/flamewm-render-core/src/lower.rs:261: clip-stack Vec (allocates only when scroll clips exist).
- crates/flamewm-render-core/src/layout.rs:179,204,253 and model/style/runtime.rs:320-347,370: returned boxes/contents/z_order plus z-index and style clones; public outputs or semantic state.
- crates/flamewm-render-core/src/glyphs.rs:429: `caret_positions` Vec is the public return value.

PARTIAL: two allocation sites were fixed (glyph shaping per painted line, stand-alone backdrop region) and both are marked UNBUILT because the brief forbids cargo and builds; the remaining sites above need API or cached-state changes beyond the three-edit cap, so pixel identity and the allocation counts were reasoned from source only and must be verified by the orchestrator.
