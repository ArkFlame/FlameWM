# FlameWM Pass 20 — CPU/Memory Optimization Plan and Implemented Changes

Scope: FlameWM 0.1.2 Rust X11 product source, incrementally applied after Pass 19. This is a **changed-files-only overlay**. No README edits, measurement scripts, release thresholds, new GPU backend, or dependencies are included.

## Goal and definition

Reduce idle and active allocations and redundant CPU work while preserving the user-visible FlameWM design, ABI, and fallback behavior. The requested **17 MB or less** remains a target, **not a measured or achieved result**. RSS, PSS, combined WM + Shell RSS, and whole-desktop RAM are different quantities. Validation of those metrics is explicitly deferred to the later Claude evaluation requested by the maintainer.

## Source-grounded findings

1. `crates/flamewm-image-core/src/blur.rs`: an RGBA float work image uses **16 bytes per pixel**. The `fit_f32` helper was writing zero to its full buffer before each blur even when the call immediately overwrote all active elements. `BlurScratch` is reusable and can retain a large allocation for an otherwise idle renderer after one unusual full-resolution blur.
2. `crates/flamewm-render-x11/src/backdrop.rs`: the display thread holds a thread-local `BlurScratch`; large-radius backdrops are **already downscaled 4×**, and wallpaper canvases are cached at low resolution. Both `blurred_argb_words` and `BlurredCanvas::new` retained a full-resolution input allocation during processing after producing the downscaled copy.
3. `crates/flamewm-shell/src/async_projection.rs`: decoded icon LRU shared by `Arc` was capped at **8 MiB and 2,048 entries**. It already has a bounded LRU and safely evicts old entries. A stricter budget can reclaim memory with no change to pixels on cache hits; cache misses may incur extra asynchronous icon loads.
4. `tools/ensure_shared_surface_cargo.py`: extracted archives may have historical timestamps. The existing content-addressed release invalidation tracks the renderer and Shell but **did not track `flamewm-image-core`**, making this particular optimization susceptible to stale build artifacts.
5. Hyprland upstream `src/render/Renderer.cpp` checks when precomputed blur is queued, while `src/render/OpenGL.cpp` manages GL framebuffer resources and damaged regions. These reinforce FlameWM's existing output- and damage-aware blur caching; they do not justify importing Hyprland's GPU implementation into the independent Rust/X11 stack. Sources: https://github.com/hyprwm/Hyprland/blob/main/src/render/Renderer.cpp and https://github.com/hyprwm/Hyprland/blob/main/src/render/OpenGL.cpp. The user's local Hyprland source archive was not accessible in this session; those upstream files were the reference, not a copied vendor source.

## Ordered implementation plan and status

| Priority | Change | Status | Rationale / tradeoff |
|---|---|---|---|
| P0 | Avoid zero-initializing reused float scratch | **Implemented** | Retain safe initialization when buffers grow; preserve image output and remove redundant writes during blur |
| P0 | Release extreme blur scratch at operation completion | **Implemented** | Thread-local scratch retention capped at 4 MiB after each X11 backdrop blur; new allocation needed next time a larger blur is requested |
| P0 | Free downsampled backdrop source before applying blur | **Implemented** | Reduce concurrent transient live allocations, preserving downsample and interpolation behavior |
| P1 | Reduce decoded Shell icon-cache retention ceiling | **Implemented** | 2 MiB/256 entries instead of 8 MiB/2,048 entries, while keeping LRU semantics; potential extra cache misses for unusually large icon collections |
| P1 | Content-hash guard tracks image-core | **Implemented** | Rebuild changed image-core code reliably even with preserved archive mtimes |
| P2 | Reuse output buffers or adjust per-surface invalidation after a real workload trace | **Deferred** | Might improve throughput but changes data ownership and redraw patterns; avoid speculative regression |
| P2 | Full session workload-specific memory reductions | **Deferred** | Requires the separately planned process-level review; cannot substantiate an exact RSS goal yet |
| P3 | Optional X11 GLES/Vulkan blur backend | **Deferred** | Current X11 path has CPU/XRender and cached wallpaper blur, Wayland uses Smithay GLES; introducing another renderer without fallback parity and frame lifecycle ownership could increase RAM/CPU and complicate old-GPU support |

## Validation/compatibility contract

The following Rust tests are added (not executed in this environment): `fit_f32_preserves_overwritten_scratch_instead_of_zeroing_it`, `release_if_over_budget_preserves_output_and_small_scratch_reuse`, `default_icon_cache_is_bounded_for_an_idle_shell`, `test_image_core_blur_change_invalidates_only_image_core`. Existing source tests comparing Gaussian and frosted blur output to an oracle are retained unchanged. Pure-Python build-guard and Pass 17 regression tests were run; Rust compilation and native X11 visual confirmation remain open. No measured memory or CPU improvement is claimed.

### User workstation compilation

Extract the overlay **into the existing checkout**. It must not overwrite other source or documentation. Run `./xephyr`; its content guard should invalidate `flamewm-image-core`, `flamewm-render-x11` and `flamewm-shell` as their source changes. If diagnosing separately, `cargo check --locked -p flamewm-image-core -p flamewm-render-x11 -p flamewm-shell` checks the three affected crates, then run their unit tests. No `cargo clean` of the entire workspace is required.

### Explicitly not modified

`README.md`, `docs/PERFORMANCE.md`, benchmark tooling, user settings, titlebar/taskbar visuals, Wayland renderer, dependency manifests, and previously changed product behavior.
