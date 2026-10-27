# FlameWM 0.1.0 — Pass 47: eliminate redundant single-output wallpaper frames

**Baseline:** Pass 46 (`FlameWM_0.1.0_PASS46_WALLPAPER_PEAK_MEMORY_REDUCTION_2026-10-20.tar.gz`, SHA-256 `6f62893a2bf93644405887b7bbdb0bfaedd325770c322c8585e2adffb3cd04b8`). This is an **integrated source overlay** for a complete FlameWM checkout, not a stand-alone release; the input overlay does not contain `assets/` and `.vendor/` source trees.

## Source-proven allocations and changes

`crates/flamewm-desktop/src/wallpaper_loader.rs::compose_per_monitor` historically treats even **one monitor filling the entire X11 root** as a multi-monitor composition: it allocates a root-sized RGBA canvas, calls the shared `wallpaper_fit::compose` to allocate another full-monitor RGBA image, and then copies the latter into the root canvas. Its parent `decode_bytes_at` also owns the decoded natural image during composition.

- `single_full_root_output` recognizes only an **exactly matching single screen** at `(0,0)` with dimensions equal to the root. Do not optimize cropped, offset, negative-coordinate, partial, or genuinely multi-monitor configurations through this path.
- `compose_decoded` reuses the canonical `wallpaper_fit::compose` directly for a single full-root monitor, the intentionally shared wallpaper mode, or the existing empty-monitors fallback. It never allocates a second root canvas on these paths. For multiple independent displays the original `compose_per_monitor` remains the only authority, unchanged.
- When a decoded wallpaper already has the final root dimensions and the layout is shared/single-output, **move** `natural` into the presentation without cloning its pixel vector. Every fit (`Fill`, `Fit`, `Stretch`, `Center`, `Tile`) maps a same-size source onto the exact same output pixels; zero-sized targets are explicitly excluded.
- The worker mailbox, latest-generation fencing, error retention, per-output clipping and persistence remain unchanged. No new dependencies, settings or caches.

At **3840×2160 RGBA8**, a full-root allocation has `3840×2160×4 = 33,177,600 bytes` = **31.64 MiB**. With a differently sized decoded image, the old single-monitor path temporarily retained **one avoidable root canvas** (~31.64 MiB), while the new path only retains the required composed frame (plus natural decode / unavoidable scale scratch). With an **already 3840×2160 decoded wallpaper**, the old path temporarily built both a cloned composed frame and the root canvas (up to **63.28 MiB** in additional logical pixel buffers), whereas the new path moves the natural frame directly. At 1920×1080 these figures are **7.91 MiB** or **15.82 MiB**, respectively. These are **avoided logical allocations during wallpaper changes, not measured RSS/PSS reductions**, and do not account for the retained renderer/X11 backing images.

## Verification performed

- New Rust tests in `wallpaper_loader.rs`: `one_full_output_matches_shared_composition_for_all_fits`, exercises all five fits and guards an off-origin monitor against inadvertent fast path; `matching_root_wallpaper_reuses_decoded_buffer_without_copy` asserts buffer-pointer retention for all five fits across single-monitor, empty-monitor, and shared-wallpaper paths, plus a zero-sized rejection case.
- New `tools/test_flamewm_pass47_wallpaper_identity_memory.py` source contract (4 tests), targeted failure proven against Pass 46 and success against Pass 47. These checks **do not replace native Rust tests**.
- 40 selected Python source, packaging, screenshot-memory, X11-panel, and earlier merge tests pass. UI guard: 0 errors and 24 pre-existing warnings. Architecture guard: 7 pre-existing violations unchanged: applications `x11rb` dependency/symbol, applications prefixed env, desktop colorkey, screenshot prefixed env, WM-session prefixed env, desktop CSS `display:none` transient.
- Native Cargo, Xephyr, RAM/PSS measurements, and graphical mixed-monitor testing were **not run** because the container lacks the Rust toolchain and full runtime assets. Treat successful Rust compilation and graphical evaluation as mandatory downstream gates.

## Apply and verify on the complete workstation checkout

Do not reapply Pass 41–46 over Pass 47, or the fixes may be reverted. `tar --touch` prevents stale integration test artifacts after overlay extraction.

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS47_SINGLE_OUTPUT_WALLPAPER_BUFFER_REUSE_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
tar --touch -xzf "$ARCHIVE" -C . && \
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-desktop -p flamewm-image-core && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-screenshot -p flamewm-integrations-linux && \
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Test each wallpaper fit at one physical monitor and mixed-size multi-monitor configurations with `shareWallpaper=false/true`; compare screen captures before/after, check peak PSS during repeated wallpaper switches via the existing `./scripts/bench-memory` capability, and verify icon, lock, taskbar and screenshot surfaces are unaffected.

**Skipped:** speculative changes to icon-cache budgets, X11 retained buffers, full multi-monitor composition algorithm and external Python session services. **Risk:** Pass 47's new Rust call path has not been natively compiled or visually exercised; physical multi-monitor regressions cannot be ruled out from source-only proof.
