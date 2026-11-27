# u162 GPU-side downscale of the backdrop capture (open, needs real-GPU measurement)

Area: compositor backdrop capture, `read_reduced` (render/backdrop.rs). `map` is ~12 ms of a ~35 ms capture in the nested harness.

Finding: in the software-GL harness `map_texture` is where llvmpipe finishes the queued scene render, so the 12 ms is mostly scene rendering, not transfer; it cannot be reduced by CPU-side tweaks and is not representative of a real GPU. On real hardware the readback moves w*h*4 bytes (full desktop-size capture) over PCIe and stalls.

Idea: draw the capture texture into a 1/4-size texture on the GPU and read back that (16x fewer bytes), keeping `downscale_strip` only as a fallback. Constraint: a single linear blit averages 2x2, not the 4x4 box filter used now (output would change slightly), so either blit twice (2x then 2x, each linear = exact 4x4 box) or accept and frame-check the blur result.

Acceptance: measure `copy`+`map` on a real GPU session (DRM backend), frame diff of titlebar/dock blur vs current (tolerance 2/byte), no new per-frame texture allocation (reuse the half and quarter textures).
Risk: extra render passes and two textures of VRAM; only worth it if the readback share on real hardware is large.

## Addendum (crop)
`crop_capture` (5-8 ms per capture) is at its CPU floor: columns hoisted, one vertical blend per row, 10-bit fixed point, reference-tested. What remains is writing a full-resolution bilinear upscale of the 1/4 capture. The only way to remove that is to hand the low-resolution capture to the GPU and let texture filtering do the upscale, which is the same architectural change as this unit; measure it on a real GPU together with the downscale and map phases.
