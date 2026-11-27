# u166 GPU upscale of the blurred backdrop (next after u162)
Area: `dock_backdrop.rs` after `blur_reduced`: `crop_capture` (~8 ms) writes a full-resolution bilinear upscale on the CPU, then the store loop (~6 ms) applies the silhouette alpha and premultiplies into a `MemoryRenderBuffer` that is uploaded again.
Share after u162 (real Intel GPU, per capture of 26.5 ms): crop 30%, store 23%, blur 12%, gpu_wait 13%, map 10%.
Idea: upload only the blurred low-res image (sw x sh, ~1/16 bytes) as a texture, draw it with `render_texture_from_to` (linear filter does the crop+upscale) with the silhouette as a rounded-corner shader or a small cached alpha mask texture keyed on (w,h,radii), and hold the result as a GPU backing instead of a MemoryRenderBuffer.
Constraint: premultiplied alpha and the exact crop offset/scale must match `crop_capture` (reference test exists); frame-diff titlebar and dock blur, tolerance 2/byte; reuse textures, no per-frame allocation.
Risk: medium (new draw path through DockBacking); measure with the drag probe (FLAMEWM_BACKDROP_PROFILE=1).
