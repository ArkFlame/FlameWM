# Reference: how other desktops do transparency and blur

Sources are shallow clones in `.vendor/` (read-only, git-ignored). KWin was added in this pass: `.vendor/kwin-master` (sparse: `src/plugins/blur`, `src/plugins/backgroundcontrast`, `src/effect`, `src/scene`).

## KWin (KDE Plasma), `src/plugins/blur/blur.cpp`
- **GPU dual-Kawase pyramid.** One `downsample` shader pass per iteration (each halves the resolution), then matching `upsample` passes back up. Blur strength is `iteration count` x `offset`, chosen from a table of `{minOffset, maxOffset, expandSize}` per iteration, so a stronger blur adds a pyramid level (about +33% pixels) instead of a bigger kernel. Cost is proportional to the blurred area / 4, not to the radius.
- **Per-window reusable render targets.** `renderInfo.framebuffers/textures` hold the pyramid for each window and are reallocated only when the iteration count or the background rect size changes (our `BlurScratch` is the CPU analogue).
- **Damage-clipped.** `blur()` intersects the blur shape with the device clip region (`deviceRegion`) and blurs only that `effectiveShape`, not the whole window. `setPixelsToExpandRepaintsBelowOpaqueRegions(m_expandSize)` widens repaints under the window by the kernel reach so the result stays correct.
- **Colour matrix folded into the blur's final pass.** Saturation (and contrast) are one 4x4 matrix (`colorTransformMatrix`), plus an optional noise texture pass, plus a rounded-corner variant of the on-screen pass. No separate full-frame passes.
- **Takeaway for FlameWM:** the whole capture path (map, CPU downscale, CPU blur, CPU crop, CPU upscale) is what KWin does on the GPU in a few small passes. This is the design target for u162, and it also removes the crop (the GPU samples the low-resolution level with bilinear filtering).

## scenefx / mangowm (`wlr_scene_optimized_blur`)
- Blurs the static background (wallpaper and bottom layers) once per monitor, caches it, and every blurred window samples that cache. Window moves cost nothing; the cache is rebuilt only when the background changes. The trade-off is that windows under a blurred window are not in the blur ("x-ray").
- FlameWM already has the equivalent for the desktop level: `refresh_desktop_backdrop` / the `desktop_backdrop` stage. Windows beneath still need a real capture, which is what u156/u161/u164 throttle.

## COSMIC (`cosmic-epoch`, `ext_background_effect`)
- Blur is requested by the client through the `ext_background_effect` protocol (a region the compositor may blur), not decided by the compositor per window class. FlameWM decides in the compositor (window-level pseudo-transparency), which keeps clients simple; no change proposed.

## Consequences for the loop
- u162 gains a concrete design: dual-Kawase pyramid on the GPU, per-window FBO chain reused across frames, blur restricted to damage intersected with the window shape, colour matrix in the last pass. Needs a real GPU to measure; do not claim a gain from the nested software-GL harness.
- u163 (blur padding): KWin handles edge correctness by expanding repaints by `expandSize` rather than by padding the capture; same effect as our padding, so the edge-pixel change in u163 matches the reference behaviour (blur samples real neighbouring pixels at the window edge).
