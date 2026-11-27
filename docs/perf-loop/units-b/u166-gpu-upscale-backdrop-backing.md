# u166 GPU upscale of the blurred backdrop (next after u162)
Area: `dock_backdrop.rs` after `blur_reduced`: `crop_capture` (~8 ms) writes a full-resolution bilinear upscale on the CPU, then the store loop (~6 ms) applies the silhouette alpha and premultiplies into a `MemoryRenderBuffer` that is uploaded again.
Share after u162 (real Intel GPU, per capture of 26.5 ms): crop 30%, store 23%, blur 12%, gpu_wait 13%, map 10%.
Idea: upload only the blurred low-res image (sw x sh, ~1/16 bytes) as a texture, draw it with `render_texture_from_to` (linear filter does the crop+upscale) with the silhouette as a rounded-corner shader or a small cached alpha mask texture keyed on (w,h,radii), and hold the result as a GPU backing instead of a MemoryRenderBuffer.
Constraint: premultiplied alpha and the exact crop offset/scale must match `crop_capture` (reference test exists); frame-diff titlebar and dock blur, tolerance 2/byte; reuse textures, no per-frame allocation.
Risk: medium (new draw path through DockBacking); measure with the drag probe (FLAMEWM_BACKDROP_PROFILE=1).

## Scoping note (after reading the consumer)
The backing is consumed as `Elem::Memory(MemoryRenderBufferRenderElement)` (`dock_backdrop.rs` `backing_element`, `titlebar.rs` parts/strip). A GPU-upscaled backing needs a new `Elem` variant that draws a low-res texture with a rounded-corner alpha mask (shader or cached mask texture), damage/commit handling for it, and a frame-diff against the CPU result. That is a multi-file change touching the element enum, so it is not a safe single slice. Split: (a) u166a add a texture element with mask behind a flag and diff it; (b) u166b switch dock/titlebar to it and delete crop/store. Only worth it during moves/drags (the capture only runs then), so rank it below idle-wakeup and RAM work unless drag latency is the complaint.

## Building blocks found (for whoever implements u166a)
- `MemoryRenderBufferRenderElement::from_buffer(renderer, loc, buffer, alpha, src, size, kind)` takes `src` and `size`: a low-res (sw x sh) blurred buffer drawn with `size = (w, h)` is upscaled by the GPU filter, so `crop_capture` and the full-size store disappear.
- Silhouette: `clip.rs` `RoundedSurfaceElement` already owns a `GlesTexProgram` with a `corner_radius` uniform, but it wraps `WaylandSurfaceRenderElement`. u166a = the same wrapper around the memory element (or a generalised element), with the dock's outline (`rounded_coverage`) and the window mask (`client_mask_coverage`) expressed as uniforms. Premultiply must match `premultiply_pixel`.

## Attempt 1 (worker luna, rejected)
First worker draft of the rounded memory element did not compile (src typed `Rectangle<f64, Buffer>` where `from_buffer` wants `Rectangle<f64, Logical>`, an unhandled `Elem::Backing` arm in `mod.rs`, an ambiguous `borrow_mut`), and its shader used only `radius.x`, ignoring per-corner window radii and the dock outline. Backed out; not committed. A retry needs: per-corner radius uniform (match `client_mask_coverage`), compile pass in the brief (workers may not run cargo, so the orchestrator must fix type errors), and the `Elem` match in `mod.rs` handled.
